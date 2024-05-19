use std::time::Duration;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer};
use sxd_xpath::XPath;
use url::Url;

/// A resource which should be polled for info.
#[derive(Debug, Clone, Deserialize)]
pub struct Job {
    /// URL to the resource
    pub url: Url,
    /// Period at which the resource is polled
    pub period: Duration,
    /// Targets to be queried
    pub targets: Targets,
    /// The path which should be visited next
    pub continuation: Continuation,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Targets(pub IndexMap<String, Target>);

#[derive(Debug, Clone, Deserialize)]
pub enum Target {
    Single {
        path: RawXPath,
        then: Option<Box<Target>>,
    },
    /// Iterate over all current children (by their IDs).
    Each(Targets),
}

#[derive(Debug, Clone, Deserialize)]
pub enum Continuation {
    Ref(RawXPath),
}

/// [`XPath`] internally stored as a [`String`].
#[derive(Debug, Clone)]
pub struct RawXPath(String);

impl RawXPath {
    pub fn as_xpath(&self) -> XPath {
        sxd_xpath::Factory::new()
            .build(&self.0)
            .expect("Path should have been parsed")
            .expect("Path should be non-empty")
    }
}

impl<'de> Deserialize<'de> for RawXPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        let raw = String::deserialize(deserializer)?;
        sxd_xpath::Factory::new()
            .build(&raw)
            .map_err(|error| Error::custom(format_args!("failed to parse XPath: {error}")))
            .and_then(|xpath| {
                xpath
                    .ok_or_else(|| Error::custom("no XPath was specified"))
                    .map(|_| Self(raw))
            })
    }
}
