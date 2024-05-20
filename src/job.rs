use std::{
    fmt,
    fmt::{Formatter, Write},
    path::PathBuf,
    time::Duration,
};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use skyscraper::{
    xpath,
    xpath::{
        grammar::{data_model::Node, NonTreeXpathNode},
        xpath_item_set::XpathItemSet,
        Xpath, XpathItemTree,
    },
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

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unknown => f.write_str("?"),
            Value::String(value) => f.write_str(value),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub enum Continuation {
    Ref(ParsedXPath),
}

impl Continuation {
    pub fn evaluate(&self, tree: &XpathItemTree) -> Vec<String> {
        match self {
            Continuation::Ref(path) => {
                let Ok(items) = path.to_xpath().apply(tree) else {
                    return vec![];
                };

                items
                    .iter()
                    .filter_map(|item| {
                        item.as_node()
                            .and_then(Node::as_non_tree_node)
                            .and_then(NonTreeXpathNode::as_attribute_node)
                            .map(|node| node.value.clone())
                            .ok()
                    })
                    .collect()
            }
        }
    }
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
