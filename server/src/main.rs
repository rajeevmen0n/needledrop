//! Needledrop server: config load, shared state, router, static file serving, tracing.

mod config;
mod daily;
mod deezer;
mod game;
mod mp3;
mod routes;

use anyhow::Context;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

/// Address the server listens on unless `GTS_BIND` says otherwise.
const DEFAULT_BIND: &str = "127.0.0.1:4810";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let bind = std::env::var("GTS_BIND").unwrap_or_else(|_| DEFAULT_BIND.to_owned());

    let app = routes::router().layer(TraceLayer::new_for_http());

    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);

    axum::serve(listener, app).await.context("serving")
}
