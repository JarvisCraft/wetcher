use std::{path::PathBuf, time::Duration};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use skyscraper::{
    xpath,
    xpath::{grammar::XpathItemTreeNode, xpath_item_set::XpathItemSet, Xpath},
};
use url::Url;

/// A resource which should be polled for info.
#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    /// The scraped resource
    pub resource: Resource,
    /// Period at which the resource is polled
    pub period: Duration,
    /// Targets to be queried
    pub targets: Targets,
    /// The path which should be visited next
    pub continuation: Continuation,
}

#[derive(Debug, Clone, Deserialize)]
pub enum Resource {
    Url(Url),
    Path(PathBuf),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Targets(pub IndexMap<String, Target>);

#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub path: ParsedXPath,
    pub then: Then,
}

#[derive(Debug, Clone, Deserialize)]
pub enum Then {
    Get(Targets),
    Extract(ValueExtractor),
}

#[derive(Debug, Clone, Deserialize)]
pub enum ValueExtractor {
    Text,
}

impl ValueExtractor {
    pub fn extract<'tree>(&self, items: XpathItemSet<'tree>) -> Vec<Value<'tree>> {
        use skyscraper::xpath::grammar::data_model::*;
        match self {
            Self::Text => items
                .iter()
                .map(|item| {
                    item.as_node()
                        .and_then(Node::as_tree_node)
                        .and_then(|tree| tree.data.as_text_node())
                        .map(|item| Value::String(&item.content))
                        .unwrap_or(Value::Unknown)
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value<'tree> {
    Unknown,
    String(&'tree str),
}

#[derive(Debug, Clone, Deserialize)]
pub enum Continuation {
    Ref(ParsedXPath),
}

/// [`XPath`] internally stored as a [`String`].
#[derive(Debug, Clone)]
pub struct ParsedXPath(String);

impl ParsedXPath {
    pub fn to_xpath(&self) -> Xpath {
        xpath::parse(&self.0).expect("Path should have been parsed")
    }
}

impl<'de> Deserialize<'de> for ParsedXPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        let raw = String::deserialize(deserializer)?;
        xpath::parse(&raw)
            .map_err(|error| Error::custom(format_args!("failed to parse XPath: {error}")))
            .map(|_| Self(raw))
    }
}
