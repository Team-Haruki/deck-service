use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use deck_service::bridge::DeckRecommend;
use deck_service::handlers;
use deck_service::masterdata::{MasterdataSignature, masterdata_signature};
use deck_service::state::{AppState, DebugConfig, EnginePool, UserdataCache};

mod logging;

#[tokio::main]
async fn main() {
    logging::init();
    tracing::info!("===== Deck Service v{} =====", env!("CARGO_PKG_VERSION"));
    tracing::info!("Powered by Haruki Dev Team");

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
    let engine_pool_size =
        env_usize_at_least_one("DECK_ENGINE_POOL_SIZE").unwrap_or_else(default_engine_pool_size);

    let engines = EnginePool::new(engine_pool_size).expect("Failed to create DeckRecommend pool");
    let state = Arc::new(AppState {
        engines,
        next_op_id: std::sync::atomic::AtomicU64::new(0),
        debug: DebugConfig {
            lock_warn_threshold,
            lock_timeout,
            engine_warn_threshold,
            default_recommend_timeout_ms,
        },
        userdata_cache: UserdataCache::default(),
    });

    preload_masterdata(state.as_ref());
    preload_musicmetas(state.as_ref());
    start_masterdata_refresh(Arc::clone(&state));

    tracing::info!(
        lock_warn_ms = lock_warn_threshold.as_millis() as u64,
        lock_timeout_ms = lock_timeout.as_millis() as u64,
        engine_warn_ms = engine_warn_threshold.as_millis() as u64,
        engine_pool_size = state.engines.size(),
        default_recommend_timeout_ms = default_recommend_timeout_ms.unwrap_or_default(),
        "Initialized deck-service debug thresholds"
    );

    let app = Router::new()
        .route("/health", get(handlers::health))
        .route("/cache_userdata", post(handlers::cache_userdata))
        .route("/recommend", post(handlers::recommend))
        .route("/calculate", post(handlers::calculate))
        .route(
            "/world_bloom/support_cards",
            post(handlers::world_bloom_support_cards),
        )
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

fn env_usize_at_least_one(name: &str) -> Option<usize> {
    match env::var(name) {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(value) if value > 0 => Some(value),
            Ok(_) => {
                tracing::warn!(
                    env_var = name,
                    value = %raw,
                    "Ignoring non-positive engine pool size"
                );
                None
            }
            Err(err) => {
                tracing::warn!(
                    env_var = name,
                    value = %raw,
                    error = %err,
                    "Ignoring invalid engine pool size"
                );
                None
            }
        },
        Err(_) => None,
    }
}

fn default_engine_pool_size() -> usize {
    std::thread::available_parallelism()
        .map(|value| value.get().min(4))
        .unwrap_or(1)
}

fn preload_masterdata(state: &AppState) {
    let requested_base_dir = env::var("DECK_MASTERDATA_DIR")
        .or_else(|_| env::var("DECK_MASTERDATA_BASE_DIR"))
        .unwrap_or_default();
    let regions = env_csv("DECK_MASTERDATA_REGIONS", &["jp", "en", "cn", "tw", "kr"]);

    for region in regions {
        update_masterdata_region(state, &requested_base_dir, &region, "preload");
    }
}

fn start_masterdata_refresh(state: Arc<AppState>) {
    let interval = env_duration_ms("DECK_MASTERDATA_REFRESH_MS", 300_000);
    if interval.is_zero() {
        tracing::info!("Deck-service masterdata refresh loop disabled");
        return;
    }

    let requested_base_dir = env::var("DECK_MASTERDATA_DIR")
        .or_else(|_| env::var("DECK_MASTERDATA_BASE_DIR"))
        .unwrap_or_default();
    let regions = env_csv("DECK_MASTERDATA_REGIONS", &["jp", "en", "cn", "tw", "kr"]);
    let mut known = HashMap::new();
    for region in &regions {
        if let Ok(Some(signature)) = masterdata_signature(&requested_base_dir, region) {
            known.insert(region.clone(), signature);
        }
    }

    tracing::info!(
        refresh_ms = interval.as_millis() as u64,
        regions = %regions.join(","),
        "Deck-service masterdata refresh loop started"
    );

    std::thread::spawn(move || {
        loop {
            std::thread::sleep(interval);
            refresh_masterdata_regions(state.as_ref(), &requested_base_dir, &regions, &mut known);
        }
    });
}

fn refresh_masterdata_regions(
    state: &AppState,
    requested_base_dir: &str,
    regions: &[String],
    known: &mut HashMap<String, MasterdataSignature>,
) {
    for region in regions {
        let signature = match masterdata_signature(requested_base_dir, region) {
            Ok(Some(signature)) => signature,
            Ok(None) => {
                tracing::warn!(
                    region = %region,
                    requested_base_dir = %requested_base_dir,
                    "Skipping masterdata refresh because no directory was resolved"
                );
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    region = %region,
                    requested_base_dir = %requested_base_dir,
                    error = %err,
                    "Failed to check masterdata refresh signature"
                );
                continue;
            }
        };

        if known
            .get(region)
            .is_some_and(|previous| previous.hash == signature.hash)
        {
            continue;
        }

        tracing::info!(
            region = %region,
            resolved_base_dir = %signature.base_dir,
            file_count = signature.file_count,
            "Detected deck-service masterdata update"
        );
        if let Some(applied) =
            update_masterdata_region(state, requested_base_dir, region, "refresh")
        {
            known.insert(region.clone(), applied);
        }
    }
}

fn update_masterdata_region(
    state: &AppState,
    requested_base_dir: &str,
    region: &str,
    reason: &'static str,
) -> Option<MasterdataSignature> {
    let signature = match masterdata_signature(requested_base_dir, region) {
        Ok(Some(signature)) => signature,
        Ok(None) => {
            tracing::warn!(
                region = %region,
                requested_base_dir = %requested_base_dir,
                reason,
                "Skipping masterdata update because no directory was resolved"
            );
            return None;
        }
        Err(err) => {
            tracing::warn!(
                region = %region,
                requested_base_dir = %requested_base_dir,
                reason,
                error = %err,
                "Skipping masterdata update because signature check failed"
            );
            return None;
        }
    };

    tracing::info!(
        region = %region,
        requested_base_dir = %requested_base_dir,
        resolved_base_dir = %signature.base_dir,
        reason,
        "Updating deck-service masterdata"
    );

    let mut engines = match state.engines.checkout_all(state.debug.lock_timeout) {
        Ok(engines) => engines,
        Err(err) => {
            tracing::error!(
                region = %region,
                requested_base_dir = %requested_base_dir,
                resolved_base_dir = %signature.base_dir,
                reason,
                error = %err.timeout_message(),
                "Failed to lock engine pool for masterdata update"
            );
            return None;
        }
    };

    let Some(engine) = engines.iter().next() else {
        tracing::error!(
            region = %region,
            reason,
            "Engine pool is empty; cannot update masterdata"
        );
        return None;
    };
    if let Err(err) = engine.update_masterdata(&signature.base_dir, region) {
        tracing::error!(
            region = %region,
            resolved_base_dir = %signature.base_dir,
            reason,
            error = %err,
            "Failed to update deck-service masterdata"
        );
        return None;
    }

    engines.clear_userdata_hashes();
    state.userdata_cache.clear();
    tracing::info!(
        region = %region,
        resolved_base_dir = %signature.base_dir,
        engine_count = engines.len(),
        file_count = signature.file_count,
        reason,
        "Updated deck-service masterdata"
    );
    Some(signature)
}

fn preload_musicmetas(state: &AppState) {
    let requested_base_dir = env::var("DECK_MUSICMETAS_DIR")
        .or_else(|_| env::var("DECK_MUSICMETAS_BASE_DIR"))
        .or_else(|_| env::var("DECK_MASTERDATA_DIR"))
        .or_else(|_| env::var("DECK_MASTERDATA_BASE_DIR"))
        .unwrap_or_else(|_| "/app/data".into());
    let regions = env_csv("DECK_MUSICMETAS_REGIONS", &["jp", "en", "cn", "tw", "kr"]);

    for region in regions {
        let env_file_name = format!("DECK_MUSICMETAS_FILE_{}", region.to_ascii_uppercase());
        let resolved_file_path = env::var(&env_file_name)
            .unwrap_or_else(|_| resolve_musicmetas_file_path(&requested_base_dir, &region));
        if resolved_file_path.trim().is_empty() {
            tracing::warn!(
                region = %region,
                requested_base_dir = %requested_base_dir,
                "Skipping music metas preload because no file was resolved"
            );
            continue;
        }

        if !Path::new(&resolved_file_path).is_file() {
            tracing::warn!(
                region = %region,
                requested_base_dir = %requested_base_dir,
                resolved_file_path = %resolved_file_path,
                "Skipping music metas preload because the file does not exist"
            );
            continue;
        }

        tracing::info!(
            region = %region,
            requested_base_dir = %requested_base_dir,
            resolved_file_path = %resolved_file_path,
            "Preloading deck-service music metas"
        );

        let engines = match state.engines.checkout_all(state.debug.lock_timeout) {
            Ok(engines) => engines,
            Err(err) => {
                tracing::error!(
                    region = %region,
                    requested_base_dir = %requested_base_dir,
                    resolved_file_path = %resolved_file_path,
                    error = %err.timeout_message(),
                    "Failed to lock engine pool for music metas preload"
                );
                continue;
            }
        };

        let Some(engine) = engines.iter().next() else {
            tracing::error!(
                region = %region,
                "Engine pool is empty; cannot preload music metas"
            );
            continue;
        };
        if let Err(err) = engine.update_musicmetas(&resolved_file_path, &region) {
            tracing::error!(
                region = %region,
                resolved_file_path = %resolved_file_path,
                error = %err,
                "Failed to preload deck-service music metas"
            );
            continue;
        }

        tracing::info!(
            region = %region,
            resolved_file_path = %resolved_file_path,
            engine_count = engines.len(),
            "Preloaded deck-service music metas"
        );
    }
}

fn resolve_musicmetas_file_path(base_dir: &str, region: &str) -> String {
    let trimmed = base_dir.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let base_path = Path::new(trimmed);
    if base_path.is_file() {
        return trimmed.to_string();
    }

    let filename = musicmetas_filename(region);
    let candidates = [
        base_path.join(filename),
        base_path.join("data").join(filename),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    candidates[0].to_string_lossy().into_owned()
}

fn musicmetas_filename(region: &str) -> &'static str {
    match region {
        "cn" => "music_metas-cn.json",
        "tw" => "music_metas-tc.json",
        "en" => "music_metas-en.json",
        "kr" => "music_metas-kr.json",
        _ => "music_metas.json",
    }
}

fn env_csv(name: &str, default: &[&str]) -> Vec<String> {
    match env::var(name) {
        Ok(raw) => {
            let values = raw
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| item.to_ascii_lowercase())
                .collect::<Vec<_>>();
            if values.is_empty() {
                default.iter().map(|item| (*item).to_string()).collect()
            } else {
                values
            }
        }
        Err(_) => default.iter().map(|item| (*item).to_string()).collect(),
    }
}
