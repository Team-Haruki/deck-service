mod bridge;
mod error;
mod ffi;
mod handlers;
mod models;
mod state;

use std::env;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use parking_lot::Mutex;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use bridge::DeckRecommend;
use state::{AppState, DebugConfig};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("deck_service=info".parse().unwrap()),
        )
        .init();

    // Initialize static data path (defaults to _cpp_src/data relative to the executable)
    let data_dir = env::var("DECK_DATA_DIR").unwrap_or_else(|_| {
        let exe = env::current_exe().expect("cannot determine executable path");
        let base = exe.parent().unwrap().parent().unwrap().parent().unwrap();
        base.join("_cpp_src/data").to_string_lossy().into_owned()
    });

    tracing::info!("Initializing data path: {data_dir}");
    DeckRecommend::init_data_path(&data_dir).expect("Failed to init data path");

    let lock_warn_threshold = env_duration_ms("DECK_LOCK_WARN_MS", 1_000);
    let lock_timeout = env_duration_ms("DECK_LOCK_TIMEOUT_MS", 30_000);
    let engine_warn_threshold = env_duration_ms("DECK_ENGINE_WARN_MS", 10_000);
    let default_recommend_timeout_ms = env_optional_i32("DECK_RECOMMEND_TIMEOUT_MS");

    let engine = DeckRecommend::new().expect("Failed to create DeckRecommend engine");
    let state = Arc::new(AppState {
        engine: Mutex::new(engine),
        next_op_id: std::sync::atomic::AtomicU64::new(0),
        debug: DebugConfig {
            lock_warn_threshold,
            lock_timeout,
            engine_warn_threshold,
            default_recommend_timeout_ms,
        },
    });

    tracing::info!(
        lock_warn_ms = lock_warn_threshold.as_millis() as u64,
        lock_timeout_ms = lock_timeout.as_millis() as u64,
        engine_warn_ms = engine_warn_threshold.as_millis() as u64,
        default_recommend_timeout_ms = default_recommend_timeout_ms.unwrap_or_default(),
        "Initialized deck-service debug thresholds"
    );

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/cache_userdata", post(handlers::cache_userdata))
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

fn env_duration_ms(name: &str, default_ms: u64) -> Duration {
    match env::var(name) {
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(err) => {
                tracing::warn!(
                    env_var = name,
                    value = %raw,
                    error = %err,
                    fallback_ms = default_ms,
                    "Invalid duration env var, using default"
                );
                Duration::from_millis(default_ms)
            }
        },
        Err(_) => Duration::from_millis(default_ms),
    }
}

fn env_optional_i32(name: &str) -> Option<i32> {
    match env::var(name) {
        Ok(raw) => match raw.trim().parse::<i32>() {
            Ok(value) if value > 0 => Some(value),
            Ok(_) => {
                tracing::warn!(
                    env_var = name,
                    value = %raw,
                    "Ignoring non-positive timeout env var"
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    env_var = name,
                    value = %raw,
                    error = %err,
                    "Ignoring invalid timeout env var"
                );
                None
            }
        },
        Err(_) => None,
    }
}
