//! HTTP handlers and the encrypted-cookie game session.

use axum::{Json, Router, routing::get};
use serde_json::{Value, json};

/// Every route the server exposes, all under `/api`.
pub fn router() -> Router {
    Router::new().route("/api/health", get(health))
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true }))
}
