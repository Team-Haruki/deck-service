use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use sonic_rs::{JsonValueTrait, json};

use crate::error::AppError;
use crate::masterdata::resolve_masterdata_base_dir;
use crate::models::{
    BatchRecommendRequest, BatchRecommendResponseItem, CacheUserdataResponse, DeckRecommendOptions,
    DeckRecommendResult, UpdateMasterdataFromJsonRequest, UpdateMasterdataRequest,
    UpdateMusicmetasFromStringRequest, UpdateMusicmetasRequest,
};
use crate::state::{AppState, EngineLease};

pub async fn health() -> &'static str {
    "ok"
}

pub async fn cache_userdata(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    expect_octet_stream_content_type(&headers)?;
    tracing::info!(
        op_id,
        op = "cache_userdata",
        content_type = %request_content_type(&headers),
        compressed_bytes = body.len(),
        "Request accepted"
    );

    let segments = extract_decompressed_segments(body.as_ref())?;
    if segments.len() != 1 {
        return Err(AppError::BadRequest(
            "cache_userdata expects exactly one payload segment".into(),
        ));
    }

    let userdata = String::from_utf8(segments.into_iter().next().unwrap())
        .map_err(|_| AppError::BadRequest("userdata payload must be valid UTF-8 JSON".into()))?;
    tracing::debug!(
        op_id,
        op = "cache_userdata",
        userdata_bytes = userdata.len(),
        "Userdata payload parsed"
    );

    let userdata_hash = tokio::task::block_in_place(|| {
        run_engine_op(state.as_ref(), op_id, "cache_userdata", |engine| {
            let userdata_hash = engine.cache_userdata(&userdata)?;
            engine.remember_userdata_hash(&userdata_hash);
            Ok(userdata_hash)
        })
    })?;
    state.userdata_cache.remember(&userdata_hash, &userdata);

    tracing::info!(
        op_id,
        op = "cache_userdata",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        hash_prefix = %truncate_head(&userdata_hash, 8),
        "Request completed"
    );

    Ok(Json(CacheUserdataResponse { userdata_hash }).into_response())
}

pub async fn recommend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let op_id = state.next_op_id();
    let content_type = request_content_type(&headers);
    tracing::info!(
        op_id,
        op = "recommend",
        content_type = %content_type,
        body_bytes = body.len(),
        "Dispatching recommend request"
    );
    if is_octet_stream_content_type(&content_type) {
        return recommend_batch(state, body, op_id).await;
    }
    if is_json_content_type(&content_type) {
        return recommend_legacy(state, body, op_id).await;
    }

    Err(AppError::UnsupportedMediaType(format!(
        "unsupported content type for /recommend: {content_type}"
    )))
}

pub async fn update_masterdata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    let resolved_base_dir = resolve_masterdata_base_dir(&req.base_dir, &req.region);
    tracing::info!(
        op_id,
        op = "update_masterdata",
        region = %req.region,
        requested_base_dir = %req.base_dir,
        resolved_base_dir = %resolved_base_dir,
        "Request accepted"
    );
    if resolved_base_dir != req.base_dir {
        tracing::info!(
            op_id,
            requested_base_dir = %req.base_dir,
            resolved_base_dir = %resolved_base_dir,
            region = %req.region,
            "Resolved masterdata path for deck-service"
        );
    }
    tokio::task::block_in_place(|| {
        run_engine_broadcast_op(state.as_ref(), op_id, "update_masterdata", true, |engine| {
            engine.update_masterdata(&resolved_base_dir, &req.region)
        })
    })?;
    tracing::info!(
        op_id,
        op = "update_masterdata",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Request completed"
    );
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_masterdata_from_json(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataFromJsonRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    tracing::info!(
        op_id,
        op = "update_masterdata_from_json",
        region = %req.region,
        file_count = req.data.len(),
        "Request accepted"
    );
    tokio::task::block_in_place(|| {
        run_engine_broadcast_op(
            state.as_ref(),
            op_id,
            "update_masterdata_from_json",
            true,
            |engine| engine.update_masterdata_from_json(&req.data, &req.region),
        )
    })?;
    tracing::info!(
        op_id,
        op = "update_masterdata_from_json",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Request completed"
    );
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_musicmetas(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    tracing::info!(
        op_id,
        op = "update_musicmetas",
        region = %req.region,
        file_path = %req.file_path,
        "Request accepted"
    );
    tokio::task::block_in_place(|| {
        run_engine_broadcast_op(state.as_ref(), op_id, "update_musicmetas", true, |engine| {
            engine.update_musicmetas(&req.file_path, &req.region)
        })
    })?;
    tracing::info!(
        op_id,
        op = "update_musicmetas",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Request completed"
    );
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_musicmetas_from_string(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasFromStringRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    tracing::info!(
        op_id,
        op = "update_musicmetas_from_string",
        region = %req.region,
        data_bytes = req.data.len(),
        "Request accepted"
    );
    tokio::task::block_in_place(|| {
        run_engine_broadcast_op(
            state.as_ref(),
            op_id,
            "update_musicmetas_from_string",
            true,
            |engine| engine.update_musicmetas_from_string(&req.data, &req.region),
        )
    })?;
    tracing::info!(
        op_id,
        op = "update_musicmetas_from_string",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Request completed"
    );
    Ok(Json(json!({ "status": "ok" })))
}

async fn recommend_legacy(
    state: Arc<AppState>,
    body: Bytes,
    op_id: u64,
) -> Result<Response, AppError> {
    let request_started = Instant::now();
    let mut options: DeckRecommendOptions = sonic_rs::from_slice(body.as_ref())
        .map_err(|e| AppError::BadRequest(format!("invalid recommend payload: {e}")))?;
    inject_default_recommend_timeout(&mut options, state.as_ref());
    let userdata_hash = normalize_userdata_hash(options.userdata_hash.as_deref());
    let userdata_payload = resolve_userdata_payload(state.as_ref(), userdata_hash.as_deref())?;
    tracing::info!(
        op_id,
        op = "recommend_legacy",
        region = %options.region,
        live_type = %options.live_type,
        music_id = options.music_id,
        music_diff = %options.music_diff,
        algorithm = options.algorithm.as_deref().unwrap_or(""),
        target = options.target.as_deref().unwrap_or(""),
        timeout_ms = options.timeout_ms.unwrap_or_default(),
        "Legacy recommend request parsed"
    );

    let result: DeckRecommendResult = tokio::task::block_in_place(|| {
        run_engine_op(state.as_ref(), op_id, "recommend_legacy", |engine| {
            if let (Some(userdata_hash), Some(userdata_payload)) =
                (userdata_hash.as_deref(), userdata_payload.as_deref())
            {
                ensure_userdata_hash(engine, userdata_hash, userdata_payload)?;
            }
            engine.recommend(&options)
        })
    })?;

    tracing::info!(
        op_id,
        op = "recommend_legacy",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        deck_count = result.decks.len(),
        "Legacy recommend request completed"
    );

    Ok(Json(result).into_response())
}

async fn recommend_batch(
    state: Arc<AppState>,
    body: Bytes,
    op_id: u64,
) -> Result<Response, AppError> {
    let request_started = Instant::now();
    let req = parse_batch_recommend_request(body.as_ref())?;

    if req.batch_options.is_empty() {
        return Err(AppError::BadRequest(
            "batch_options must contain at least one recommend option".into(),
        ));
    }
    if req.userdata_hash.trim().is_empty() {
        return Err(AppError::BadRequest("userdata_hash is required".into()));
    }

    tracing::info!(
        op_id,
        op = "recommend_batch",
        region = %req.region,
        batch_size = req.batch_options.len(),
        userdata_hash_prefix = %truncate_head(&req.userdata_hash, 8),
        "Batch recommend request parsed"
    );

    let BatchRecommendRequest {
        region,
        batch_options,
        userdata_hash,
    } = req;
    let userdata_payload = resolve_userdata_payload(state.as_ref(), Some(userdata_hash.as_str()))?
        .expect("batch recommend requires userdata payload");
    let mut handles = Vec::with_capacity(batch_options.len());

    for (index, mut option) in batch_options.into_iter().enumerate() {
        inject_default_batch_timeout(&mut option, state.as_ref());
        let alg = option
            .get("algorithm")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned);
        let timeout_ms = option
            .get("timeout_ms")
            .and_then(|value| value.as_i64())
            .unwrap_or_default();

        let state = Arc::clone(&state);
        let region = region.clone();
        let userdata_hash = userdata_hash.clone();
        let userdata_payload = Arc::clone(&userdata_payload);
        handles.push(tokio::task::spawn_blocking(move || {
            option.insert("region".into(), json!(region.as_str()));
            option.insert("userdata_hash".into(), json!(userdata_hash.as_str()));

            tracing::debug!(
                op_id,
                op = "recommend_batch_item",
                item_index = index,
                region = %region,
                algorithm = alg.as_deref().unwrap_or(""),
                timeout_ms,
                "Starting batch recommendation item"
            );

            match run_engine_op_with_stats(
                state.as_ref(),
                op_id,
                "recommend_batch_item",
                |engine| {
                    ensure_userdata_hash(engine, &userdata_hash, userdata_payload.as_ref())?;
                    engine.recommend_value(&option)
                },
            ) {
                Ok(stats) => {
                    let support_deck_debug = summarize_support_deck_debug(&stats.value);
                    tracing::info!(
                        op_id,
                        op = "recommend_batch_item",
                        item_index = index,
                        region = %region,
                        algorithm = alg.as_deref().unwrap_or(""),
                        timeout_ms,
                        wait_ms = elapsed_ms(stats.lock_elapsed),
                        elapsed_ms = elapsed_ms(stats.engine_elapsed),
                        deck_count = stats.value.decks.len(),
                        support_deck = %support_deck_debug,
                        "Batch recommendation item completed"
                    );
                    (
                        index,
                        BatchRecommendResponseItem {
                            alg,
                            cost_time: stats.engine_elapsed.as_secs_f64(),
                            wait_time: stats.lock_elapsed.as_secs_f64(),
                            result: Some(stats.value),
                            error: None,
                        },
                    )
                }
                Err(AppError::Engine(err)) => {
                    tracing::warn!(
                        op_id,
                        op = "recommend_batch_item",
                        item_index = index,
                        region = %region,
                        algorithm = alg.as_deref().unwrap_or(""),
                        timeout_ms,
                        error = %err,
                        "Batch deck recommendation failed"
                    );
                    (
                        index,
                        BatchRecommendResponseItem {
                            alg,
                            cost_time: 0.0,
                            wait_time: 0.0,
                            result: None,
                            error: Some(err),
                        },
                    )
                }
                Err(AppError::Timeout(err)) => {
                    tracing::warn!(
                        op_id,
                        op = "recommend_batch_item",
                        item_index = index,
                        region = %region,
                        algorithm = alg.as_deref().unwrap_or(""),
                        timeout_ms,
                        error = %err,
                        "Batch deck recommendation timed out"
                    );
                    (
                        index,
                        BatchRecommendResponseItem {
                            alg,
                            cost_time: 0.0,
                            wait_time: 0.0,
                            result: None,
                            error: Some(err),
                        },
                    )
                }
                Err(err) => (
                    index,
                    BatchRecommendResponseItem {
                        alg,
                        cost_time: 0.0,
                        wait_time: 0.0,
                        result: None,
                        error: Some(err.to_string()),
                    },
                ),
            }
        }));
    }

    let mut results = std::iter::repeat_with(|| None)
        .take(handles.len())
        .collect::<Vec<Option<BatchRecommendResponseItem>>>();
    for handle in handles {
        let (index, item) = handle
            .await
            .map_err(|err| AppError::Engine(format!("batch recommend worker join error: {err}")))?;
        results[index] = Some(item);
    }
    let results = results
        .into_iter()
        .map(|item| item.expect("batch recommend worker did not return a response"))
        .collect::<Vec<_>>();

    tracing::info!(
        op_id,
        op = "recommend_batch",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        item_count = results.len(),
        "Batch recommend request completed"
    );

    Ok(Json(results).into_response())
}

fn request_content_type(headers: &HeaderMap) -> String {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_ascii_lowercase()
}

fn is_json_content_type(content_type: &str) -> bool {
    content_type.is_empty() || content_type.starts_with("application/json")
}

fn is_octet_stream_content_type(content_type: &str) -> bool {
    content_type.starts_with("application/octet-stream")
}

fn expect_octet_stream_content_type(headers: &HeaderMap) -> Result<(), AppError> {
    let content_type = request_content_type(headers);
    if is_octet_stream_content_type(&content_type) {
        return Ok(());
    }
    Err(AppError::UnsupportedMediaType(format!(
        "expected application/octet-stream, got {content_type}"
    )))
}

fn parse_batch_recommend_request(body: &[u8]) -> Result<BatchRecommendRequest, AppError> {
    let segments = extract_decompressed_segments(body)?;
    if segments.len() != 1 {
        return Err(AppError::BadRequest(
            "batch recommend payload expects exactly one JSON segment".into(),
        ));
    }

    sonic_rs::from_slice(&segments[0])
        .map_err(|e| AppError::BadRequest(format!("invalid batch recommend payload: {e}")))
}

fn extract_decompressed_segments(body: &[u8]) -> Result<Vec<Vec<u8>>, AppError> {
    let mut decoder = ruzstd::decoding::StreamingDecoder::new(Cursor::new(body))
        .map_err(|e| AppError::BadRequest(format!("failed to decode zstd payload: {e}")))?;
    let mut payload = Vec::new();
    decoder
        .read_to_end(&mut payload)
        .map_err(|e| AppError::BadRequest(format!("failed to decode zstd payload: {e}")))?;

    let mut segments = Vec::new();
    let mut index = 0usize;
    while index < payload.len() {
        if index + 4 > payload.len() {
            return Err(AppError::BadRequest(
                "invalid payload framing: truncated segment length".into(),
            ));
        }

        let segment_len = u32::from_be_bytes([
            payload[index],
            payload[index + 1],
            payload[index + 2],
            payload[index + 3],
        ]) as usize;
        index += 4;

        if index + segment_len > payload.len() {
            return Err(AppError::BadRequest(
                "invalid payload framing: truncated segment body".into(),
            ));
        }

        segments.push(payload[index..index + segment_len].to_vec());
        index += segment_len;
    }

    if segments.is_empty() {
        return Err(AppError::BadRequest(
            "payload does not contain any segments".into(),
        ));
    }

    Ok(segments)
}

fn inject_default_recommend_timeout(options: &mut DeckRecommendOptions, state: &AppState) {
    if options.timeout_ms.is_some() {
        return;
    }
    if let Some(timeout_ms) = state.debug.default_recommend_timeout_ms {
        options.timeout_ms = Some(timeout_ms);
        tracing::debug!(
            default_timeout_ms = timeout_ms,
            "Injected default recommend timeout"
        );
    }
}

fn inject_default_batch_timeout(
    option: &mut crate::models::BatchRecommendOption,
    state: &AppState,
) {
    if option.get("timeout_ms").is_some() {
        return;
    }
    if let Some(timeout_ms) = state.debug.default_recommend_timeout_ms {
        option.insert("timeout_ms".into(), json!(timeout_ms));
        tracing::debug!(
            default_timeout_ms = timeout_ms,
            "Injected default batch recommend timeout"
        );
    }
}

fn normalize_userdata_hash(userdata_hash: Option<&str>) -> Option<String> {
    let hash = userdata_hash?.trim();
    if hash.is_empty() {
        return None;
    }
    Some(hash.to_string())
}

fn resolve_userdata_payload(
    state: &AppState,
    userdata_hash: Option<&str>,
) -> Result<Option<Arc<str>>, AppError> {
    let Some(userdata_hash) = normalize_userdata_hash(userdata_hash) else {
        return Ok(None);
    };
    match state.userdata_cache.get(&userdata_hash) {
        Some(payload) => Ok(Some(payload)),
        None => Err(AppError::BadRequest(format!(
            "unknown userdata_hash: {userdata_hash}; call /cache_userdata first"
        ))),
    }
}

fn ensure_userdata_hash(
    engine: &mut EngineLease<'_>,
    userdata_hash: &str,
    userdata_payload: &str,
) -> Result<(), String> {
    if engine.has_userdata_hash(userdata_hash) {
        return Ok(());
    }

    let cached_hash = engine.cache_userdata(userdata_payload)?;
    if cached_hash != userdata_hash.trim() {
        engine.forget_userdata_hash(userdata_hash);
        return Err(format!(
            "cache_userdata hash mismatch: expected {}, got {}",
            userdata_hash.trim(),
            cached_hash
        ));
    }

    engine.remember_userdata_hash(&cached_hash);
    Ok(())
}

fn run_engine_op<T, F>(
    state: &AppState,
    op_id: u64,
    op_name: &'static str,
    f: F,
) -> Result<T, AppError>
where
    F: FnOnce(&mut EngineLease<'_>) -> Result<T, String>,
{
    Ok(run_engine_op_with_stats(state, op_id, op_name, f)?.value)
}

struct EngineOpStats<T> {
    value: T,
    lock_elapsed: std::time::Duration,
    engine_elapsed: std::time::Duration,
}

fn run_engine_op_with_stats<T, F>(
    state: &AppState,
    op_id: u64,
    op_name: &'static str,
    f: F,
) -> Result<EngineOpStats<T>, AppError>
where
    F: FnOnce(&mut EngineLease<'_>) -> Result<T, String>,
{
    let span = tracing::debug_span!("engine_op", op_id, op = op_name);
    let _entered = span.enter();

    let lock_started = Instant::now();
    tracing::debug!("Waiting for engine slot");
    let mut engine = match state.engines.checkout(state.debug.lock_timeout) {
        Ok(engine) => engine,
        Err(err) => {
            let timeout_message = err.timeout_message();
            tracing::error!(
                lock_timeout_ms = elapsed_ms(state.debug.lock_timeout),
                error = %timeout_message,
                "Engine slot timed out"
            );
            return Err(AppError::Timeout(timeout_message));
        }
    };
    let lock_elapsed = lock_started.elapsed();
    if lock_elapsed >= state.debug.lock_warn_threshold {
        tracing::warn!(
            lock_wait_ms = elapsed_ms(lock_elapsed),
            threshold_ms = elapsed_ms(state.debug.lock_warn_threshold),
            "Engine slot wait exceeded threshold"
        );
    } else {
        tracing::debug!(
            lock_wait_ms = elapsed_ms(lock_elapsed),
            "Engine slot acquired"
        );
    }

    let engine_started = Instant::now();
    tracing::debug!("Starting engine operation");
    let result = f(&mut engine).map_err(AppError::Engine);
    let engine_elapsed = engine_started.elapsed();

    match &result {
        Ok(_) => {
            if engine_elapsed >= state.debug.engine_warn_threshold {
                tracing::warn!(
                    engine_elapsed_ms = elapsed_ms(engine_elapsed),
                    threshold_ms = elapsed_ms(state.debug.engine_warn_threshold),
                    "Engine operation exceeded threshold"
                );
            } else {
                tracing::debug!(
                    engine_elapsed_ms = elapsed_ms(engine_elapsed),
                    "Engine operation completed"
                );
            }
        }
        Err(err) => {
            tracing::error!(
                engine_elapsed_ms = elapsed_ms(engine_elapsed),
                error = %err,
                "Engine operation failed"
            );
        }
    }

    result.map(|value| EngineOpStats {
        value,
        lock_elapsed,
        engine_elapsed,
    })
}

fn run_engine_broadcast_op<T, F>(
    state: &AppState,
    op_id: u64,
    op_name: &'static str,
    clear_userdata_cache_on_success: bool,
    mut f: F,
) -> Result<Vec<T>, AppError>
where
    F: FnMut(&crate::bridge::DeckRecommend) -> Result<T, String>,
{
    let span = tracing::debug_span!("engine_broadcast_op", op_id, op = op_name);
    let _entered = span.enter();

    let lock_started = Instant::now();
    tracing::debug!("Waiting for exclusive engine pool access");
    let mut engines = match state.engines.checkout_all(state.debug.lock_timeout) {
        Ok(engines) => engines,
        Err(err) => {
            let timeout_message = err.timeout_message();
            tracing::error!(
                lock_timeout_ms = elapsed_ms(state.debug.lock_timeout),
                error = %timeout_message,
                "Exclusive engine pool lock timed out"
            );
            return Err(AppError::Timeout(timeout_message));
        }
    };
    let lock_elapsed = lock_started.elapsed();
    if lock_elapsed >= state.debug.lock_warn_threshold {
        tracing::warn!(
            lock_wait_ms = elapsed_ms(lock_elapsed),
            threshold_ms = elapsed_ms(state.debug.lock_warn_threshold),
            engine_count = engines.len(),
            "Exclusive engine pool wait exceeded threshold"
        );
    } else {
        tracing::debug!(
            lock_wait_ms = elapsed_ms(lock_elapsed),
            engine_count = engines.len(),
            "Exclusive engine pool acquired"
        );
    }

    let engine_started = Instant::now();
    tracing::debug!("Starting broadcast engine operation");
    let mut results = Vec::with_capacity(engines.len());
    for engine in engines.iter() {
        results.push(f(engine).map_err(AppError::Engine)?);
    }
    if clear_userdata_cache_on_success {
        engines.clear_userdata_hashes();
        state.userdata_cache.clear();
        tracing::info!(
            op_id,
            op = op_name,
            "Cleared cached userdata state after broadcast engine update"
        );
    }
    let engine_elapsed = engine_started.elapsed();

    if engine_elapsed >= state.debug.engine_warn_threshold {
        tracing::warn!(
            engine_elapsed_ms = elapsed_ms(engine_elapsed),
            threshold_ms = elapsed_ms(state.debug.engine_warn_threshold),
            "Broadcast engine operation exceeded threshold"
        );
    } else {
        tracing::debug!(
            engine_elapsed_ms = elapsed_ms(engine_elapsed),
            "Broadcast engine operation completed"
        );
    }

    Ok(results)
}
fn elapsed_ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn truncate_head(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

fn summarize_support_deck_debug(result: &DeckRecommendResult) -> String {
    let Some(first) = result.decks.first() else {
        return "none".into();
    };
    let mut out = format!("rate={:.2}", first.support_deck_bonus_rate);
    match &first.support_deck_cards {
        Some(cards) if !cards.is_empty() => {
            let items = cards
                .iter()
                .map(|card| format!("{}:{:.2}", card.card_id, card.bonus))
                .collect::<Vec<_>>()
                .join(",");
            out.push_str(" cards=[");
            out.push_str(&items);
            out.push(']');
        }
        _ => out.push_str(" cards=[]"),
    }
    out
}
