//! Spike connector for Localify public URLs (ADR 0003).
//!
//! Holds one WebSocket to the endpoint's Durable Object and serves the
//! multiplexed public requests against a local upstream. The product
//! connector would live inside the daemon; this binary stands alone so the
//! transport can be measured without touching the user's running daemon.

mod protocol;
mod session;
mod upstream;
mod websocket;

use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::warn;
use tracing_subscriber::EnvFilter;

use crate::upstream::Upstream;

const MIN_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
/// A session that lived this long resets the reconnect backoff.
const STABLE_SESSION: Duration = Duration::from_secs(60);

pub struct Config {
    pub edge_url: String,
    pub slug: String,
    pub token: String,
    pub local_domain: String,
    pub upstream: String,
}

impl Config {
    fn from_env() -> Self {
        let var =
            |name: &str, default: &str| std::env::var(name).unwrap_or_else(|_| default.into());
        Self {
            edge_url: var("LOCALIFY_EDGE_URL", "ws://127.0.0.1:8787"),
            slug: var("LOCALIFY_SLUG", "app-a8f3"),
            token: var("LOCALIFY_CONNECTOR_TOKEN", "spike-token"),
            local_domain: var("LOCALIFY_LOCAL_DOMAIN", "app.localify"),
            upstream: var("LOCALIFY_UPSTREAM", "127.0.0.1:5199"),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let config = Config::from_env();
    let upstream = Upstream::new(config.upstream.clone(), config.local_domain.clone());
    let mut backoff = MIN_BACKOFF;

    loop {
        let started = Instant::now();
        match session::run(&config, &upstream).await {
            Ok(()) => warn!("edge closed the connector session"),
            Err(error) => warn!(error = format!("{error:#}"), "connector session failed"),
        }
        if started.elapsed() >= STABLE_SESSION {
            backoff = MIN_BACKOFF;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}
