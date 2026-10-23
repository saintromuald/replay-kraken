//! Replay Kraken (fork) — self-hosted Cosmonarchy replay archive.
//!
//! Phase 1: upload + filters (player / race / result / matchup / date) + following +
//! Discord result webhook. Prepared for VPS deployment (Dockerfile + compose + env
//! config). Discord OAuth login and the richer web UI come next.

mod api;
mod db;
mod discord;
mod model;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Router;
use rusqlite::Connection;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

pub struct AppState {
    pub db: Mutex<Connection>,
    /// Bearer token -> account name (config `RK_TOKENS`, `name:token` pairs).
    pub tokens: HashMap<String, String>,
    pub data_dir: PathBuf,
    pub webhook_url: Option<String>,
    pub base_url: String,
    pub public_read: bool,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind = env_or("RK_BIND", "127.0.0.1:8787");
    let data_dir = PathBuf::from(env_or("RK_DATA_DIR", "./data"));
    let webhook_url = std::env::var("RK_WEBHOOK_URL").ok().filter(|s| !s.is_empty());
    let base_url = env_or("RK_BASE_URL", "http://127.0.0.1:8787");
    let public_read = env_or("RK_PUBLIC_READ", "1") == "1";

    // RK_TOKENS="admin:secret,friend:othersecret"
    let tokens: HashMap<String, String> = std::env::var("RK_TOKENS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .filter_map(|pair| {
            let (name, token) = pair.split_once(':')?;
            Some((token.trim().to_string(), name.trim().to_string()))
        })
        .collect();

    std::fs::create_dir_all(data_dir.join("replays"))?;
    let db_path = data_dir.join("kraken.db");
    let conn = Connection::open(&db_path)?;
    db::init(&conn)?;

    // Seed account -> Discord id links from config (RK_DISCORD_IDS="name:id,...").
    // Discord OAuth replaces this source in phase 2.
    for pair in std::env::var("RK_DISCORD_IDS").unwrap_or_default().split(',') {
        if let Some((name, id)) = pair.split_once(':') {
            let _ = db::set_account_discord(&conn, name.trim(), id.trim());
        }
    }

    let state = Arc::new(AppState {
        db: Mutex::new(conn),
        tokens,
        data_dir,
        webhook_url,
        base_url,
        public_read,
    });

    let app = Router::new()
        .merge(api::router(state.clone()))
        .fallback_service(ServeDir::new("static"))
        .layer(CorsLayer::permissive());

    println!("replay-kraken listening on http://{bind} (data: {})", state.data_dir.display());
    let listener = tokio::net::TcpListener::bind(&bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

/// How the uploaded replay/desync/randoms bytes map onto the response the client
/// gets back from the API (the gptp ReplayKraken protocol).
pub const REPLAY_FILE_EXT: &str = "rep";
pub const DESYNC_FILE_EXT: &str = "desync";
pub const RANDOMS_FILE_EXT: &str = "randoms";
