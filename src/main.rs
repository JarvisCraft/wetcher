mod cmd;
mod job;

use std::{borrow::Cow, io, path::PathBuf, process::ExitCode};

use clap::Parser;
use config::{Config, ConfigError};
use indexmap::IndexMap;
use job::Job;
use serde::Deserialize;
use skyscraper::{
    html,
    xpath::{xpath_item_set::XpathItemSet, ExpressionApplyError, XpathItemTree},
};
use thiserror::__private::AsDisplay;
use tokio::{fs, signal::ctrl_c};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;

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

// #[tracing::instrument(skip(client))]
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
            .unwrap(),
        targets,
    );
    info!("Found: {result:#?}");

    match continuation {
        job::Continuation::Ref(path) => match path.to_xpath().apply(&tree) {
            Ok(element) => match element.iter().next() {
                Some(item) => match item.as_node() {
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
                },
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

fn process_targets<'tree>(
    tree: &'tree XpathItemTree,
    items: XpathItemSet<'tree>,
    targets: &'tree job::Targets,
) -> ProcessingResult<'tree> {
    ProcessingResult::Group(
        items
            .iter()
            .enumerate()
            .map(|(id, item)| {
                info!("Scanning: {item}");
                info!("Item: {item:?}");
                let group = ProcessingResult::Group(
                    targets
                        .0
                        .iter()
                        .map(|(name, job::Target { path, then })| {
                            (
                                Cow::Borrowed(name.as_str()),
                                match path.to_xpath().apply_to_item(tree, item.clone()) {
                                    Ok(items) => {
                                        if let Some(then) = then {
                                            process_targets(tree, items, then)
                                        } else {
                                            ProcessingResult::Leaf(items)
                                        }
                                    }
                                    Err(error) => ProcessingResult::Unknown(error),
                                },
                            )
                        })
                        .collect(),
                );
                (Cow::Owned(format!("[{id}]")), group)
            })
            .collect(),
    )
}

#[derive(Debug)]
enum ProcessingResult<'tree> {
    Group(IndexMap<Cow<'tree, str>, ProcessingResult<'tree>>),
    Leaf(XpathItemSet<'tree>),
    Unknown(ExpressionApplyError),
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
        //                    /html/body/div[1]/div/div[5]/div/div[2]/div[3]/div[3]/div[3]/div[2]/div[3]/div/div/div[2]/div[2]/div/a/h3
        const XPATH0: &str =
            "//html/body/div[1]/div/div[5]/div/div[2]/div[3]/div[3]/div[3]/div[2]/div";
        //                     /html/body/div[1]/div/div[6]/div/div[2]/div[3]/div[3]/div[3]/div[2]/div
        let xpath0 = xpath::parse(XPATH0).unwrap();

        let document = html::parse(&std::fs::read_to_string("./avito.html").unwrap()).unwrap();

        let tree = XpathItemTree::from(&document);

        let items = xpath0.apply(&tree).unwrap();
        print_items(&items);

        let xpath = xpath::parse("/div/div/div[2]/div[2]/div/a/h3/").unwrap();
        for item in &items {
            let items = xpath.apply_to_item(&tree, item.clone()).unwrap();
            print_items(&items);
        }
    }
}
