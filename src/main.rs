mod cmd;
mod job;

use std::{borrow::Cow, fs::File, io, io::Write, path::PathBuf, process::ExitCode};

use clap::Parser;
use config::{Config, ConfigError};
use indexmap::IndexMap;
use job::Job;
use serde::Deserialize;
use skyscraper::{
    html,
    xpath::{
        grammar::{data_model::XpathItem, NonTreeXpathNode},
        xpath_item_set::XpathItemSet,
        ExpressionApplyError, XpathItemTree,
    },
};
use thiserror::__private::AsDisplay;
use tokio::{fs, signal::ctrl_c};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

use crate::{cmd::CmdArgs, job::Resource};

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// Resources to be queried
    resources: Vec<Job>,
}

/// An error which may occur while loading [config][`AppConfig`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigLoadError {
    #[error("path {0:?} is not a valid UTF-8 path")]
    NonUtf8Path(PathBuf),
    #[error(transparent)]
    LoadError(#[from] ConfigError),
}

fn main() -> ExitCode {
    let config = cmd::CmdArgs::parse();

    #[cfg(feature = "tokio-console")]
    console_subscriber::init();
    #[cfg(not(feature = "tokio-console"))]
    {
        if let Err(error) = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_env("WETCHER_LOG"))
            .try_init()
        {
            error!("Failed to initialize fmt tracing subscriber: {error}");
            return ExitCode::FAILURE;
        }
    }

    let config = match load_config(config) {
        Ok(config) => {
            info!("Loaded config: {config:?}");
            config
        }
        Err(error) => {
            error!("Failed to load configuration: {error}");
            return ExitCode::FAILURE;
        }
    };

    info!("Running app..");

    match start(config) {
        Ok(()) => {
            info!("Received CTRL-C signal, shutting down");
            ExitCode::SUCCESS
        }
        Err(error) => {
            error!("Failed to await for CTRL-C signal: {error}");
            ExitCode::FAILURE
        }
    }
}

fn load_config(CmdArgs { config }: CmdArgs) -> Result<AppConfig, ConfigLoadError> {
    let Some(config) = config.to_str() else {
        return Err(ConfigLoadError::NonUtf8Path(config));
    };

    let config: AppConfig = Config::builder()
        .add_source(config::Environment::with_prefix("WETCHER").separator("_"))
        .add_source(config::File::with_name(config).required(false))
        .build()
        .and_then(Config::try_deserialize)?;

    Ok(config)
}

#[tokio::main]
async fn start(config: AppConfig) -> io::Result<()> {
    for Job {
        resource,
        period,
        targets,
        continuation,
    } in config.resources
    {
        let client = reqwest::Client::new();
        let mut period = tokio::time::interval(period);
        let resource = resource.clone();
        tokio::spawn(async move {
            loop {
                period.tick().await;
                let result = handle(&client, resource.clone(), &targets, &continuation).await;
                match result {
                    Ok(()) => {
                        info!("Awaiting again...");
                    }
                    Err(e) => {
                        error!("Failed to handle: {e}")
                    }
                }
            }
        });
    }

    ctrl_c().await
}

#[derive(Debug, thiserror::Error)]
enum HandleError {
    #[error("failed to execute request")]
    Send(#[from] reqwest::Error),
    #[error(transparent)]
    InvalidHtml(#[from] html::parse::ParseError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[tracing::instrument]
async fn handle(
    client: &reqwest::Client,
    resource: job::Resource,
    targets: &job::Targets,
    continuation: &job::Continuation,
) -> Result<(), HandleError> {
    info!("Performing request");
    let document = match resource {
        Resource::Url(url) => client.get(url).send().await?.text().await?,
        Resource::Path(path) => fs::read_to_string(path).await?,
    };
    debug!("Received document body: {document:?}");

    let document = html::parse(&document)?;

    let tree = XpathItemTree::from(&document);
    let result = process_targets(
        &tree,
        skyscraper::xpath::parse("//")
            .unwrap()
            .apply(&tree)
            .unwrap()[0]
            .clone(),
        targets,
    );
    info!("Found: {result:#?}");

    match continuation {
        job::Continuation::Ref(path) => match path.to_xpath().apply(&tree) {
            Ok(element) => match element.iter().next() {
                Some(item) => {
                    info!("item: !!!{item}!!!");
                    match item.as_node() {
                        Ok(node) => match node.as_non_tree_node() {
                            Ok(node) => match node.as_attribute_node() {
                                Ok(attribute) => {
                                    info!("Should continue from: {:?}", attribute.as_display());
                                }
                                Err(error) => {
                                    error!("Continuation item is not an attribute node: {error}");
                                }
                            },
                            Err(error) => {
                                error!("Continuation item is not a tree node: {error}");
                            }
                        },
                        Err(error) => {
                            error!("Continuation item is not a node: {error}");
                        }
                    }
                }
                None => {
                    error!("No available continuations");
                }
            },
            Err(error) => {
                warn!("Failed to find continuation: {error}");
            }
        },
    }

    Ok(())
}

#[tracing::instrument(skip(tree, item))]
fn process_targets<'tree>(
    tree: &'tree XpathItemTree,
    item: XpathItem<'tree>,
    targets: &'tree job::Targets,
) -> ProcessingResult<'tree> {
    info!("Scanning: {item}");
    ProcessingResult::Node(
        targets
            .0
            .iter()
            .filter_map(|(name, target)| {
                let value = match target {
                    job::Target::Single { path, then } => {
                        let items = match path.to_xpath().apply_to_item(tree, item.clone()) {
                            Ok(value) => value,
                            Err(error) => {
                                warn!("Failed to process: {error}");
                                return None;
                            }
                        };

                        info!("Found: {items}");
                        if let Some(_then) = then {
                            // FIXME
                            ProcessingResult::Node(IndexMap::new())
                        } else {
                            // ProcessingResult::Leaf(value)
                            ProcessingResult::Leaf(items)
                        }
                    }
                    // job::Target::Each(targets) => ProcessingResult::Node(
                    //     item.iter()
                    //         .map(|child| {
                    //             (
                    //                 Cow::Owned(child.to_string()),
                    //                 // process_targets(child, targets),
                    //                 ProcessingResult::Node(Default::default()),
                    //             )
                    //         })
                    //         .collect(),
                    // ),
                    job::Target::Each(targets) => ProcessingResult::Node(Default::default()),
                };
                Some((Cow::Borrowed(name.as_str()), value))
            })
            .collect(),
    )
}

#[derive(Debug)]
enum ProcessingResult<'tree> {
    Node(IndexMap<Cow<'tree, str>, ProcessingResult<'tree>>),
    Leaf(XpathItemSet<'tree>),
}

#[cfg(test)]
mod tests {
    use skyscraper::xpath;

    use super::*;

    fn print_items(items: &XpathItemSet<'_>) {
        println!("{} items:", items.len());
        for item in items {
            println!("-> {item:?}");
        }
    }

    #[test]
    fn test_path() {
        const XPATH0: &str =
            "/html/body/div[1]/div/div[5]/div/div[2]/div[3]/div[3]/div[4]/nav/ul/li[9]/a";
        let xpath0 = xpath::parse(XPATH0).unwrap();

        let document = html::parse(&std::fs::read_to_string("./avito.html").unwrap()).unwrap();

        let tree = XpathItemTree::from(&document);

        let items = xpath0.apply(&tree).unwrap();
        print_items(&items);

        let xpath =
            xpath::parse("/html/body/div[1]/div/div[6]/div/div[2]/div[3]/div[3]/div[3]/div[2]/div");
        let items = xpath0.apply_to_item(&tree, items[0].clone()).unwrap();
        print_items(&items);
    }
}
