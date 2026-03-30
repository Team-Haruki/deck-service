mod bridge;
mod error;
mod ffi;
mod handlers;
mod models;
mod state;

use std::env;
use std::sync::{Arc, Mutex};

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use bridge::DeckRecommend;
use state::AppState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("deck_service=info".parse().unwrap()))
        .init();

    // Initialize static data path (defaults to _cpp_src/data relative to the executable)
    let data_dir = env::var("DECK_DATA_DIR").unwrap_or_else(|_| {
        let exe = env::current_exe().expect("cannot determine executable path");
        let base = exe.parent().unwrap().parent().unwrap().parent().unwrap();
        base.join("_cpp_src/data").to_string_lossy().into_owned()
    });

    tracing::info!("Initializing data path: {data_dir}");
    DeckRecommend::init_data_path(&data_dir).expect("Failed to init data path");

    let engine = DeckRecommend::new().expect("Failed to create DeckRecommend engine");
    let state = Arc::new(AppState {
        engine: Mutex::new(engine),
    });

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/recommend", post(handlers::recommend))
        .route("/update/masterdata", post(handlers::update_masterdata))
        .route(
            "/update/masterdata/json",
            post(handlers::update_masterdata_from_json),
        )
        .route("/update/musicmetas", post(handlers::update_musicmetas))
        .route(
            "/update/musicmetas/string",
            post(handlers::update_musicmetas_from_string),
        )
        .layer(DefaultBodyLimit::max(1000 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state);

    let bind = env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".into());
    tracing::info!("Listening on {bind}");
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .expect("Failed to bind");
    axum::serve(listener, app).await.unwrap();
}
