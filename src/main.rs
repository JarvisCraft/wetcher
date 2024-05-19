mod cmd;
mod job;

use std::{borrow::Cow, path::PathBuf, process::ExitCode};

use clap::Parser;
use config::{Config, ConfigError};
use indexmap::IndexMap;
use job::Job;
use nodeset::Node;
use serde::Deserialize;
use sxd_xpath::{nodeset, Context, Value};
use tokio::{select, signal::ctrl_c};
use tracing::{debug, error, info, warn};
use tracing_subscriber::EnvFilter;
use url::Url;

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

    run(config);
    ExitCode::FAILURE
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
async fn run(config: AppConfig) {
    for Job {
        url,
        period,
        targets,
        continuation,
    } in config.resources
    {
        let client = reqwest::Client::new();
        let mut period = tokio::time::interval(period);
        let url = url.clone();
        tokio::spawn(async move {
            loop {
                period.tick().await;
                let result = handle(&client, url.clone(), &targets, &continuation).await;
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

    ctrl_c().await;
    info!("Received CTRL-C signal, shutting down");
}

#[derive(Debug, thiserror::Error)]
enum HandleError {
    #[error("failed to execute request")]
    Send(#[from] reqwest::Error),
    #[error("failed to find element by XPath")]
    XPathError(#[from] sxd_xpath::ExecutionError),
}

#[tracing::instrument]
async fn handle(
    client: &reqwest::Client,
    url: Url,
    targets: &job::Targets,
    continuation: &job::Continuation,
) -> Result<(), HandleError> {
    info!("Performing request");
    let response_body = client.get(url).send().await?.text().await?;
    debug!("Received document body: {response_body:?}");
    let response_body = {
        let (response_body, html_errors) = sxd_html::parse_html_with_errors(&response_body);
        if !html_errors.is_empty() {
            warn!("There are HTML errors: {html_errors:?}");
        }
        response_body
    };

    let dom = response_body.as_document();

    let context = Context::new();
    process_targets(&context, dom.root().into(), targets);

    match continuation {
        job::Continuation::Ref(path) => {
            let continuation = path.as_xpath().evaluate(&context, dom.root())?;
            info!("Should continue at: {continuation:?}");
        }
    }

    Ok(())
}

#[tracing::instrument(skip(context))]
fn process_targets<'d>(
    context: &'d Context,
    node: Node<'d>,
    targets: &'d job::Targets,
) -> ProcessingResult<'d> {
    ProcessingResult::Node(
        targets
            .0
            .iter()
            .filter_map(|(name, target)| {
                let value = match target {
                    job::Target::Single { path, then } => {
                        let value = match path.as_xpath().evaluate(context, node) {
                            Ok(value) => value,
                            Err(error) => {
                                warn!("Failed to process: {error}");
                                return None;
                            }
                        };

                        if let Some(_then) = then {
                            match value {
                                Value::Nodeset(nodeset) => {
                                    // FIXME:
                                    info!("Nodes: {nodeset:?}");
                                    // We need to go deeper.
                                    // process_targets(context);
                                }
                                Value::Boolean(_) => {}
                                Value::Number(_) => {}
                                Value::String(_) => {}
                            };
                            // FIMXE
                            ProcessingResult::Node(IndexMap::new())
                        } else {
                            ProcessingResult::Leaf(value)
                        }
                    }
                    job::Target::Each(targets) => ProcessingResult::Node(
                        node.children()
                            .into_iter()
                            .map(|child| {
                                (
                                    Cow::<str>::Owned(child.string_value()),
                                    process_targets(context, child, targets),
                                )
                            })
                            .collect(),
                    ),
                };
                Some((Cow::Borrowed(name.as_str()), value))
            })
            .collect(),
    )
}

#[derive(Debug)]
enum ProcessingResult<'d> {
    Node(IndexMap<Cow<'d, str>, ProcessingResult<'d>>),
    Leaf(Value<'d>),
}
