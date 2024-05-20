mod cmd;
mod job;

use std::{borrow::Cow, collections::VecDeque, io, path::PathBuf, process::ExitCode};

use clap::Parser;
use config::{Config, ConfigError};
use indexmap::IndexMap;
use job::Job;
use serde::Deserialize;
use skyscraper::{
    html,
    xpath::{xpath_item_set::XpathItemSet, ExpressionApplyError, XpathItemTree},
};
use tokio::{fs, signal::ctrl_c};
use tracing::{debug, error, info, span, warn, Level};
use tracing_subscriber::EnvFilter;

use crate::cmd::CmdArgs;

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
        let _span = span!(Level::INFO, "job", resource = ?&resource).entered();
        let client = reqwest::Client::new();
        let mut period = tokio::time::interval(period);
        let base_resource = resource.clone();
        tokio::spawn(async move {
            loop {
                period.tick().await;
                let mut resource_queue = VecDeque::new();
                resource_queue.push_back(base_resource.clone());
                while let Some(resource) = resource_queue.pop_front() {
                    match handle(&client, resource.clone(), &targets, &continuation).await {
                        Ok(continuations) => {
                            info!("Found continuations: {continuations:?}");
                            match resource {
                                job::Resource::Url(url) => {
                                    resource_queue.extend(continuations.into_iter().map(
                                        |continuation| {
                                            let mut url = url.clone();
                                            url.set_path(
                                                if continuation.as_bytes().first() == Some(&b'/') {
                                                    &continuation[1..]
                                                } else {
                                                    &continuation
                                                },
                                            );
                                            job::Resource::Url(url)
                                        },
                                    ));
                                }
                                job::Resource::Path(_) => {
                                    warn!("Path resource does not support continuation yet");
                                }
                            }
                        }
                        Err(e) => {
                            error!("Failed to handle: {e}");
                        }
                    }
                }
                info!("Awaiting again...");
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

#[tracing::instrument(skip(client), fields(resource = %resource))]
async fn handle(
    client: &reqwest::Client,
    resource: job::Resource,
    targets: &job::Targets,
    continuation: &job::Continuation,
) -> Result<Vec<String>, HandleError> {
    info!("Performing request");
    let document = match resource {
        job::Resource::Url(url) => client.get(url).send().await?.text().await?,
        job::Resource::Path(path) => fs::read_to_string(path).await?,
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

    Ok(continuation.evaluate(&tree))
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
                let group = ProcessingResult::Group(
                    targets
                        .0
                        .iter()
                        .map(|(name, job::Target { path, then })| {
                            (
                                Cow::Borrowed(name.as_str()),
                                match path.to_xpath().apply_to_item(tree, item.clone()) {
                                    Ok(items) => match then {
                                        job::Then::Get(next_targets) => {
                                            process_targets(tree, items, next_targets)
                                        }
                                        job::Then::Extract(extractor) => {
                                            ProcessingResult::Values(extractor.extract(items))
                                        }
                                    },
                                    Err(error) => ProcessingResult::Error(error),
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

#[allow(dead_code)] // Ony used for `Debug`.
#[derive(Debug)]
enum ProcessingResult<'tree> {
    Group(IndexMap<Cow<'tree, str>, ProcessingResult<'tree>>),
    Values(Vec<job::Value<'tree>>),
    Error(ExpressionApplyError),
}
