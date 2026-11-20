//! Starts everything: opens the database, starts the recorder, and lists the API routes.

mod db;
mod garden;
mod sensors;

use std::sync::{Arc, Mutex};

use axum::Json;
use axum::http::StatusCode;
use rusqlite::Connection;
use serde_json::Value;
use tower_http::services::ServeDir;

use db::{get_history, migrate, open};
use garden::*;
use sensors::{Sensors, get_now, record_every_minute};

pub type ApiError = (StatusCode, Json<Value>);
pub type Error = Box<dyn std::error::Error + Send + Sync>;

#[derive(Clone)]
pub struct App {
    pub db: Arc<Mutex<Connection>>,
    pub sensors: Arc<tokio::sync::Mutex<Sensors>>,
    pub admins: Arc<Vec<String>>,
}

async fn shutdown() {
    let ctrl_c = async { tokio::signal::ctrl_c().await.expect("Ctrl-C handler") };
    #[cfg(unix)]
    let term = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = term => {} }
}

#[tokio::main]
async fn main() {
    let mut conn = open("garden.db").expect("open garden.db");
    migrate(&mut conn).expect("run migrations");

    let state = App {
        db: Arc::new(Mutex::new(conn)),
        sensors: Default::default(),
        admins: Arc::new(parse_admins(
            &std::env::var("GARDEN_ADMINS").unwrap_or_default(),
        )),
    };
    eprintln!("admins: {}", state.admins.len());
    tokio::spawn(record_every_minute(state.clone()));

    let app = axum::Router::new()
        .route("/api/history", axum::routing::get(get_history))
        .route("/api/now", axum::routing::get(get_now))
        .route("/api/me", axum::routing::get(get_me))
        .route("/api/beds", axum::routing::get(get_beds).post(post_bed))
        .route(
            "/api/beds/{id}",
            axum::routing::patch(patch_bed).delete(delete_bed),
        )
        .route(
            "/api/plantings",
            axum::routing::get(get_plantings).post(post_planting),
        )
        .route(
            "/api/plantings/{id}",
            axum::routing::patch(patch_planting).delete(delete_planting),
        )
        .fallback_service(ServeDir::new("web/dist"))
        .with_state(state.clone());
    // Localhost only: tailscale serve is the sole access control (ADR 0002).
    let listener = tokio::net::TcpListener::bind("127.0.0.1:8080")
        .await
        .expect("bind 127.0.0.1:8080");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await
        .expect("server error");

    eprintln!("shutting down");
    // Waits for a snapshot or save in progress; kept until exit so no new snapshot starts Python again.
    let mut sensors = state.sensors.lock().await;
    sensors.python = None; // kill_on_drop stops it
    eprintln!("stopped");
}
