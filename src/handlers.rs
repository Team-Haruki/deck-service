use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use parking_lot::MutexGuard;
use sonic_rs::{JsonValueTrait, json};

use crate::bridge::DeckRecommend;
use crate::error::AppError;
use crate::models::{
    BatchRecommendRequest, BatchRecommendResponseItem, CacheUserdataResponse, DeckRecommendOptions,
    DeckRecommendResult, UpdateMasterdataFromJsonRequest, UpdateMasterdataRequest,
    UpdateMusicmetasFromStringRequest, UpdateMusicmetasRequest,
};
use crate::state::AppState;

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
            engine.cache_userdata(&userdata)
        })
    })?;

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
        run_engine_op(state.as_ref(), op_id, "update_masterdata", |engine| {
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
        run_engine_op(state.as_ref(), op_id, "update_masterdata_from_json", |engine| {
            engine.update_masterdata_from_json(&req.data, &req.region)
        })
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
        run_engine_op(state.as_ref(), op_id, "update_musicmetas", |engine| {
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
        run_engine_op(state.as_ref(), op_id, "update_musicmetas_from_string", |engine| {
            engine.update_musicmetas_from_string(&req.data, &req.region)
        })
    })?;
    tracing::info!(
        op_id,
        op = "update_musicmetas_from_string",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Request completed"
    );
    Ok(Json(json!({ "status": "ok" })))
}

async fn recommend_legacy(state: Arc<AppState>, body: Bytes, op_id: u64) -> Result<Response, AppError> {
    let request_started = Instant::now();
    let mut options: DeckRecommendOptions = sonic_rs::from_slice(body.as_ref())
        .map_err(|e| AppError::BadRequest(format!("invalid recommend payload: {e}")))?;
    inject_default_recommend_timeout(&mut options, state.as_ref());
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

async fn recommend_batch(state: Arc<AppState>, body: Bytes, op_id: u64) -> Result<Response, AppError> {
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

    let results = tokio::task::block_in_place(|| {
        run_engine_op(state.as_ref(), op_id, "recommend_batch", |engine| {
            let region = req.region;
            let userdata_hash = req.userdata_hash;
            let mut responses = Vec::with_capacity(req.batch_options.len());
            for (index, mut option) in req.batch_options.into_iter().enumerate() {
                let alg = option
                    .get("algorithm")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned);
                inject_default_batch_timeout(&mut option, state.as_ref());
                let timeout_ms = option
                    .get("timeout_ms")
                    .and_then(|value| value.as_i64())
                    .unwrap_or_default();

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

                let started = Instant::now();
                match engine.recommend_value(&option) {
                    Ok(result) => {
                        let elapsed = started.elapsed();
                        let support_deck_debug = summarize_support_deck_debug(&result);
                        tracing::info!(
                            op_id,
                            op = "recommend_batch_item",
                            item_index = index,
                            region = %region,
                            algorithm = alg.as_deref().unwrap_or(""),
                            timeout_ms,
                            elapsed_ms = elapsed_ms(elapsed),
                            deck_count = result.decks.len(),
                            support_deck = %support_deck_debug,
                            "Batch recommendation item completed"
                        );
                        responses.push(BatchRecommendResponseItem {
                            alg,
                            cost_time: elapsed.as_secs_f64(),
                            wait_time: 0.0,
                            result: Some(result),
                            error: None,
                        });
                    }
                    Err(err) => {
                        let elapsed = started.elapsed();
                        tracing::warn!(
                            op_id,
                            op = "recommend_batch_item",
                            item_index = index,
                            region = %region,
                            algorithm = alg.as_deref().unwrap_or(""),
                            timeout_ms,
                            elapsed_ms = elapsed_ms(elapsed),
                            error = %err,
                            "Batch deck recommendation failed"
                        );
                        responses.push(BatchRecommendResponseItem {
                            alg,
                            cost_time: elapsed.as_secs_f64(),
                            wait_time: 0.0,
                            result: None,
                            error: Some(err),
                        });
                    }
                }
            }
            Ok::<Vec<BatchRecommendResponseItem>, String>(responses)
        })
    })?;

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
    let payload = zstd::stream::decode_all(Cursor::new(body))
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
        return Err(AppError::BadRequest("payload does not contain any segments".into()));
    }

    Ok(segments)
}

fn resolve_masterdata_base_dir(base_dir: &str, region: &str) -> String {
    let trimmed_region = region.trim();
    for candidate in candidate_masterdata_dirs(base_dir, trimmed_region) {
        if has_masterdata_marker(&candidate) {
            return candidate.to_string_lossy().into_owned();
        }
    }
    base_dir.to_string()
}

fn candidate_masterdata_dirs(base_dir: &str, region: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    let trimmed_base_dir = base_dir.trim();
    if !trimmed_base_dir.is_empty() {
        let base = PathBuf::from(trimmed_base_dir);
        push_candidate(&mut candidates, base.clone());
        if !region.is_empty() {
            let base_name_matches_region = base
                .file_name()
                .and_then(|value| value.to_str())
                .map(|value| value.eq_ignore_ascii_case(region))
                .unwrap_or(false);
            if !base_name_matches_region {
                push_candidate(&mut candidates, base.join(region));
            }
        }
    }

    if !region.is_empty() {
        push_candidate(&mut candidates, PathBuf::from("/data").join(region));
        push_candidate(&mut candidates, PathBuf::from("/masterdata").join(region));
    }

    candidates
}

fn push_candidate(candidates: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.as_os_str().is_empty() || candidates.iter().any(|existing| existing == &candidate) {
        return;
    }
    candidates.push(candidate);
}

fn has_masterdata_marker(path: &Path) -> bool {
    path.join("areaItemLevels.json").is_file()
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

fn inject_default_batch_timeout(option: &mut crate::models::BatchRecommendOption, state: &AppState) {
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

fn run_engine_op<T, F>(
    state: &AppState,
    op_id: u64,
    op_name: &'static str,
    f: F,
) -> Result<T, AppError>
where
    F: FnOnce(&DeckRecommend) -> Result<T, String>,
{
    let span = tracing::debug_span!("engine_op", op_id, op = op_name);
    let _entered = span.enter();

    let lock_started = Instant::now();
    tracing::debug!("Waiting for engine lock");
    let Some(engine): Option<MutexGuard<'_, DeckRecommend>> =
        state.engine.try_lock_for(state.debug.lock_timeout)
    else {
        tracing::error!(
            lock_timeout_ms = elapsed_ms(state.debug.lock_timeout),
            "Engine lock timed out"
        );
        return Err(AppError::Timeout(format!(
            "engine lock timeout after {} ms",
            state.debug.lock_timeout.as_millis()
        )));
    };
    let lock_elapsed = lock_started.elapsed();
    if lock_elapsed >= state.debug.lock_warn_threshold {
        tracing::warn!(
            lock_wait_ms = elapsed_ms(lock_elapsed),
            threshold_ms = elapsed_ms(state.debug.lock_warn_threshold),
            "Engine lock wait exceeded threshold"
        );
    } else {
        tracing::debug!(lock_wait_ms = elapsed_ms(lock_elapsed), "Engine lock acquired");
    }

    let engine_started = Instant::now();
    tracing::debug!("Starting engine operation");
    let result = f(&engine).map_err(AppError::Engine);
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

    result
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
