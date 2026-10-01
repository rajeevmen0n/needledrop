//! Needledrop server: config load, shared state, router, tracing.

mod admin;
mod config;
mod daily;
mod deezer;
mod game;
mod mp3;
mod pick;
mod player;
mod routes;
mod stats;
mod store;
#[cfg(test)]
mod testutil;

use std::sync::Arc;

use anyhow::Context;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{
    config::{Config, StoreKind},
    daily::{Clock, Daily},
    deezer::Deezer,
    routes::AppState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,tower_http=debug")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::load()?;
    tracing::info!(?config, "configuration loaded");

    let key = player::session_key(&config.data_dir)?;
    let deezer = Deezer::new(config.public_url.as_deref()).context("building the Deezer client")?;

    // A database that cannot be used stops the server here, like a bad config,
    // rather than on the first request that needs it.
    let store =
        store::open(StoreKind::Sqlite, &config.store_path()).context("opening the store")?;
    let seeded = store::seed_if_empty(store.as_ref())
        .await
        .context("seeding the song pool")?;
    if seeded > 0 {
        tracing::info!(
            songs = seeded,
            "the song pool was empty; added the seed songs"
        );
    }

    let daily = Daily::new(
        deezer.clone(),
        Arc::clone(&store),
        &config.data_dir,
        Clock::utc(),
    );
    let state = AppState::new(deezer, daily, store, config.launch_date, key);

    let listener = TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);

    // Pick and load today's songs now rather than on the first player's
    // request. A failure is not fatal: it is logged where it happens and the
    // next request tries again, so a Deezer hiccup at startup does not need a
    // restart.
    let warm_up = state.clone();
    tokio::spawn(async move { warm_up.warm_up().await });

    let app = routes::router(state).layer(TraceLayer::new_for_http());
    axum::serve(listener, app).await.context("serving")
}
