use std::{path::PathBuf, time::Duration};

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use skyscraper::{xpath, xpath::Xpath};
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
    pub then: Option<Targets>,
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
