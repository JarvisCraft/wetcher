use std::time::Duration;

use config::{Config, ConfigError};
use serde::{Deserialize, Deserializer};
use sxd_xpath::{Context, XPath};
use teloxide::Bot;
use tokio::{
    select,
    signal::{
        ctrl_c,
        unix::{signal, SignalKind},
    },
};
use tracing::{error, info};
use url::Url;

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// Telegram token
    token: String,
    /// Resources to be queried
    resources: Vec<Resource>,
}

#[derive(Debug, Deserialize)]
pub struct Resource {
    /// URL to the resource
    url: Url,
    /// Period at which the resource is polled
    period: Duration,
    /// Targets to be queried
    targets: Vec<Target>,
}

#[derive(Debug, Deserialize)]
pub struct Target {
    /// Path to the targeted element
    path: RawXPath,
}

/// An error which may occur while loading [config][`AppConfig`].
#[derive(Debug, thiserror::Error)]
#[error("failed to load configuration")]
pub struct ConfigLoadError(#[from] ConfigError);

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt().init();

    let config: AppConfig = Config::builder()
        .add_source(config::Environment::with_prefix("WETCHER").separator("_"))
        .add_source(config::File::with_name("config").required(false))
        .build()
        .and_then(Config::try_deserialize)
        .map_err(ConfigLoadError)?;

    info!("Loaded config: {config:?}");

    let bot = Bot::with_client(
        config.token,
        teloxide::net::default_reqwest_settings().build()?,
    );

    run(bot, config.resources)?;

    Ok(())
}

#[derive(Debug)]
struct RawXPath(String);

impl RawXPath {
    fn to_xpath(&self) -> XPath {
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
            .map_err(|error| Error::custom("failed to parse XPath: {error}"))
            .and_then(|xpath| {
                xpath
                    .ok_or_else(|| Error::custom("no XPath was specified"))
                    .map(|_| Self(raw))
            })
    }
}

#[tokio::main]
async fn run(bot: Bot, resources: Vec<Resource>) -> color_eyre::Result<()> {
    for Resource {
        url,
        period,
        targets,
    } in resources
    {
        for Target { path } in targets {
            let mut period = tokio::time::interval(period);
            let url = url.clone();
            tokio::spawn(async move {
                loop {
                    // TODO: pass to TG
                    let result = handle(url.clone(), &path).await;
                    match result {
                        Ok(()) => {
                            info!("Awaiting again...");
                        }
                        Err(e) => {
                            error!("Failed to handle: {e}")
                        }
                    }
                    period.tick().await;
                }
            });
        }
    }

    let mut hangup_stream = signal(SignalKind::hangup())?;
    loop {
        select! {
            _ = hangup_stream.recv() => {
                info!("Reloading configuration...");
                // TODO: reload
            }
            _ = ctrl_c() => {
                break;
            }
        }
    }
    info!("Shutting down");

    Ok(())
}

async fn handle(url: Url, path: &RawXPath) -> color_eyre::Result<()> {
    let response_body = reqwest::Client::new().get(url).send().await?.text().await?;
    let response_body = sxd_html::parse_html(&response_body);
    let dom = response_body.as_document();

    let context = Context::new();
    let result = path.to_xpath().evaluate(&context, dom.root())?;
    info!("Received: {}", result.string());

    Ok(())
}
