//! Needledrop server: config load, shared state, router, tracing.

mod admin;
mod config;
mod daily;
mod deezer;
mod game;
mod mp3;
mod routes;
mod store;
#[cfg(test)]
mod testutil;

use anyhow::Context;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

use crate::{config::Config, daily::Daily, deezer::Deezer, routes::AppState};

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

    let key = routes::session_key(config.secret.as_deref(), &config.data_dir)?;
    let deezer = Deezer::new().context("building the Deezer client")?;
    let daily = Daily::new(deezer.clone(), &config.data_dir, config.track_id);

    // A database that cannot be used stops the server here, like a bad config,
    // rather than on the first request that needs it.
    let store = store::open(config.store, &config.data_dir).context("opening the store")?;
    let seeded = store::seed_if_empty(store.as_ref())
        .await
        .context("seeding the song pool")?;
    if seeded > 0 {
        tracing::info!(
            songs = seeded,
            "the song pool was empty; added the seed songs"
        );
    }

    let state = AppState::new(deezer, daily, store, config.launch_date, key);

    let listener = TcpListener::bind(&config.bind)
        .await
        .with_context(|| format!("binding {}", config.bind))?;
    tracing::info!("listening on http://{}", listener.local_addr()?);

    // Load the song now rather than on the first player's request. A failure
    // is not fatal: it is logged where it happens and the next request tries
    // again, so a Deezer hiccup at startup does not need a restart.
    let warm_up = state.clone();
    tokio::spawn(async move {
        let _ = warm_up.song(daily::today_utc()).await;
    });

    let app = routes::router(state).layer(TraceLayer::new_for_http());
    axum::serve(listener, app).await.context("serving")
}
