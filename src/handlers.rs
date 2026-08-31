use std::io::{Cursor, Read};
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use sonic_rs::{LazyValue, json};

use crate::error::AppError;
use crate::masterdata::resolve_masterdata_base_dir;
use crate::models::{
    BatchRecommendResponseItem, CacheUserdataResponse, CalculateOptions,
    UpdateMasterdataFromJsonRequest, UpdateMasterdataRequest, UpdateMusicmetasFromStringRequest,
    UpdateMusicmetasRequest, WorldBloomSupportOptions,
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

pub async fn calculate(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let options = parse_json_body::<CalculateOptions>(&body, "calculate")?;
    calculate_with_options(state, options, "calculate").await
}

pub async fn world_bloom_support_cards(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    let options = parse_json_body::<WorldBloomSupportOptions>(&body, "world bloom support cards")?;
    let userdata_hash = normalize_userdata_hash(options.userdata_hash.as_deref());
    let userdata_payload = resolve_userdata_payload(state.as_ref(), userdata_hash.as_deref())?;

    tracing::info!(
        op_id,
        op = "world_bloom_support_cards",
        region = %options.region,
        event_id = options.event_id.unwrap_or_default(),
        world_bloom_event_turn = options.world_bloom_event_turn.unwrap_or_default(),
        world_bloom_finale_turn = options.world_bloom_finale_turn.unwrap_or_default(),
        world_bloom_character_id = options.world_bloom_character_id.unwrap_or_default(),
        hash_prefix = %userdata_hash.as_deref().map(|hash| truncate_head(hash, 8)).unwrap_or_default(),
        "World bloom support cards request parsed"
    );

    let result = tokio::task::block_in_place(|| {
        run_engine_op(
            state.as_ref(),
            op_id,
            "world_bloom_support_cards",
            |engine| {
                if let (Some(userdata_hash), Some(userdata_payload)) =
                    (userdata_hash.as_deref(), userdata_payload.as_deref())
                {
                    ensure_userdata_hash(engine, userdata_hash, userdata_payload)?;
                }
                engine.get_world_bloom_support_cards_value(&options)
            },
        )
    })?;

    tracing::info!(
        op_id,
        op = "world_bloom_support_cards",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "World bloom support cards request completed"
    );

    Ok(Json(result))
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
        run_engine_exclusive_op(state.as_ref(), op_id, "update_masterdata", true, |engine| {
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
        run_engine_exclusive_op(
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
        run_engine_exclusive_op(state.as_ref(), op_id, "update_musicmetas", true, |engine| {
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
        run_engine_exclusive_op(
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

fn parse_json_body<T>(body: &Bytes, name: &str) -> Result<T, AppError>
where
    T: DeserializeOwned,
{
    sonic_rs::from_slice(body.as_ref())
        .map_err(|e| AppError::BadRequest(format!("invalid {name} payload: {e}")))
}

#[derive(Debug, Deserialize)]
struct RecommendRequestMeta {
    region: String,
    live_type: String,
    music_id: i32,
    music_diff: String,
    #[serde(default)]
    userdata_hash: Option<String>,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    timeout_ms: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct BatchRecommendRequest<'a> {
    region: String,
    #[serde(borrow)]
    batch_options: Vec<LazyValue<'a>>,
    userdata_hash: String,
}

#[derive(Debug, Deserialize)]
struct BatchRecommendItemMeta {
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    timeout_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct NativeBatchRecommendItem {
    cost_time: f64,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

async fn recommend_legacy(
    state: Arc<AppState>,
    body: Bytes,
    op_id: u64,
) -> Result<Response, AppError> {
    let request_started = Instant::now();
    let options_json = std::str::from_utf8(body.as_ref())
        .map_err(|_| AppError::BadRequest("recommend payload must be valid UTF-8 JSON".into()))?;
    let meta: RecommendRequestMeta = sonic_rs::from_slice(body.as_ref())
        .map_err(|e| AppError::BadRequest(format!("invalid recommend payload: {e}")))?;
    let userdata_hash = normalize_userdata_hash(meta.userdata_hash.as_deref());
    let userdata_payload = resolve_userdata_payload(state.as_ref(), userdata_hash.as_deref())?;
    let default_timeout_ms = state.debug.default_recommend_timeout_ms;
    let timeout_ms = meta.timeout_ms.or(default_timeout_ms).unwrap_or_default();
    tracing::info!(
        op_id,
        op = "recommend_legacy",
        region = %meta.region,
        live_type = %meta.live_type,
        music_id = meta.music_id,
        music_diff = %meta.music_diff,
        algorithm = meta.algorithm.as_deref().unwrap_or(""),
        target = meta.target.as_deref().unwrap_or(""),
        timeout_ms,
        "Legacy recommend request parsed"
    );

    let result = tokio::task::block_in_place(|| {
        run_engine_op(state.as_ref(), op_id, "recommend_legacy", |engine| {
            if let (Some(userdata_hash), Some(userdata_payload)) =
                (userdata_hash.as_deref(), userdata_payload.as_deref())
            {
                ensure_userdata_hash(engine, userdata_hash, userdata_payload)?;
            }
            engine.recommend_raw_with_default_timeout(options_json, default_timeout_ms)
        })
    })?;

    tracing::info!(
        op_id,
        op = "recommend_legacy",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        response_bytes = result.len(),
        "Legacy recommend request completed"
    );

    json_response(result)
}

async fn recommend_batch(
    state: Arc<AppState>,
    body: Bytes,
    op_id: u64,
) -> Result<Response, AppError> {
    let request_started = Instant::now();
    let payload = parse_single_decompressed_json_segment(body.as_ref(), "batch recommend")?;
    let req: BatchRecommendRequest<'_> = sonic_rs::from_slice(&payload)
        .map_err(|e| AppError::BadRequest(format!("invalid batch recommend payload: {e}")))?;

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
    let batch_options = batch_options
        .into_iter()
        .map(|option| option.as_raw_str().to_owned())
        .collect::<Vec<_>>();
    let userdata_payload = resolve_userdata_payload(state.as_ref(), Some(userdata_hash.as_str()))?
        .expect("batch recommend requires userdata payload");
    let default_timeout_ms = state.debug.default_recommend_timeout_ms;
    let results = if state.debug.engine_thread_count > 1 {
        tokio::task::block_in_place(|| {
            recommend_batch_native(
                state.as_ref(),
                op_id,
                &region,
                &userdata_hash,
                userdata_payload.as_ref(),
                &batch_options,
                default_timeout_ms,
            )
        })?
    } else {
        tokio::task::block_in_place(|| {
            recommend_batch_with_pool(
                Arc::clone(&state),
                op_id,
                &region,
                &userdata_hash,
                Arc::clone(&userdata_payload),
                &batch_options,
                default_timeout_ms,
            )
        })?
    };

    tracing::info!(
        op_id,
        op = "recommend_batch",
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        item_count = results.len(),
        "Batch recommend request completed"
    );

    json_response(batch_recommend_response_json(&results)?)
}

#[allow(clippy::too_many_arguments)]
fn recommend_batch_native(
    state: &AppState,
    op_id: u64,
    region: &str,
    userdata_hash: &str,
    userdata_payload: &str,
    batch_options: &[String],
    default_timeout_ms: Option<i32>,
) -> Result<Vec<BatchRecommendResponseItem>, AppError> {
    let mut options_json = String::with_capacity(
        batch_options.iter().map(String::len).sum::<usize>() + batch_options.len() + 1,
    );
    options_json.push('[');
    options_json.push_str(&batch_options.join(","));
    options_json.push(']');

    let stats = run_engine_op_with_stats(state, op_id, "recommend_batch_native", |engine| {
        ensure_userdata_hash(engine, userdata_hash, userdata_payload)?;
        engine.recommend_batch_raw_with_context(
            &options_json,
            region,
            userdata_hash,
            default_timeout_ms,
        )
    })?;
    tracing::info!(
        op_id,
        op = "recommend_batch_native",
        wait_ms = elapsed_ms(stats.lock_elapsed),
        elapsed_ms = elapsed_ms(stats.engine_elapsed),
        item_count = batch_options.len(),
        engine_thread_count = state.debug.engine_thread_count,
        "Native batch recommendation completed"
    );
    merge_native_batch_results(&stats.value, batch_options, stats.lock_elapsed)
}

#[allow(clippy::too_many_arguments)]
fn recommend_batch_with_pool(
    state: Arc<AppState>,
    op_id: u64,
    region: &str,
    userdata_hash: &str,
    userdata_payload: Arc<str>,
    batch_options: &[String],
    default_timeout_ms: Option<i32>,
) -> Result<Vec<BatchRecommendResponseItem>, AppError> {
    let worker_count = state.engines.size().min(batch_options.len());
    std::thread::scope(|scope| {
        let next_index = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(worker_count);

        for _ in 0..worker_count {
            let state = Arc::clone(&state);
            let userdata_payload = Arc::clone(&userdata_payload);
            let next_index = Arc::clone(&next_index);
            handles.push(scope.spawn(move || {
                let mut items = Vec::new();
                loop {
                    let index = next_index.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(option_json) = batch_options.get(index) else {
                        break;
                    };
                    items.push(process_batch_recommend_item(
                        state.as_ref(),
                        op_id,
                        index,
                        region,
                        userdata_hash,
                        userdata_payload.as_ref(),
                        option_json,
                        default_timeout_ms,
                    ));
                }
                items
            }));
        }

        let mut results = std::iter::repeat_with(|| None)
            .take(batch_options.len())
            .collect::<Vec<Option<BatchRecommendResponseItem>>>();
        for handle in handles {
            let items = handle
                .join()
                .map_err(|_| AppError::Engine("batch recommend worker panicked".into()))?;
            for (index, item) in items {
                results[index] = Some(item);
            }
        }

        Ok(results
            .into_iter()
            .map(|item| item.expect("batch recommend worker did not return a response"))
            .collect())
    })
}

async fn calculate_with_options(
    state: Arc<AppState>,
    options: CalculateOptions,
    op_name: &'static str,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let op_id = state.next_op_id();
    let request_started = Instant::now();
    tracing::info!(
        op_id,
        op = op_name,
        region = %options.region,
        mode = %options.mode,
        music_id = options.music_id.unwrap_or_default(),
        difficulty = options.difficulty.as_deref().unwrap_or(""),
        deck_id = options.deck_id.unwrap_or_default(),
        character_id = options.character_id.unwrap_or_default(),
        "Calculation request parsed"
    );

    let result = tokio::task::block_in_place(|| {
        run_engine_op(state.as_ref(), op_id, op_name, |engine| {
            engine.calculate_value(&options)
        })
    })?;

    tracing::info!(
        op_id,
        op = op_name,
        elapsed_ms = elapsed_ms(request_started.elapsed()),
        "Calculation request completed"
    );
    Ok(Json(result))
}

#[allow(clippy::too_many_arguments)]
fn process_batch_recommend_item(
    state: &AppState,
    op_id: u64,
    index: usize,
    region: &str,
    userdata_hash: &str,
    userdata_payload: &str,
    option_json: &str,
    default_timeout_ms: Option<i32>,
) -> (usize, BatchRecommendResponseItem) {
    let meta = sonic_rs::from_str::<BatchRecommendItemMeta>(option_json).ok();
    let alg = meta.as_ref().and_then(|meta| meta.algorithm.clone());
    let timeout_ms = meta
        .as_ref()
        .and_then(|meta| meta.timeout_ms)
        .or(default_timeout_ms.map(i64::from))
        .unwrap_or_default();

    tracing::debug!(
        op_id,
        op = "recommend_batch_item",
        item_index = index,
        region = %region,
        algorithm = alg.as_deref().unwrap_or(""),
        timeout_ms,
        "Starting batch recommendation item"
    );

    match run_engine_op_with_stats(state, op_id, "recommend_batch_item", |engine| {
        ensure_userdata_hash(engine, userdata_hash, userdata_payload)?;
        engine.recommend_raw_with_context(
            option_json,
            Some(region),
            Some(userdata_hash),
            default_timeout_ms,
        )
    }) {
        Ok(stats) => {
            tracing::info!(
                op_id,
                op = "recommend_batch_item",
                item_index = index,
                region = %region,
                algorithm = alg.as_deref().unwrap_or(""),
                timeout_ms,
                wait_ms = elapsed_ms(stats.lock_elapsed),
                elapsed_ms = elapsed_ms(stats.engine_elapsed),
                response_bytes = stats.value.len(),
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

fn json_response(body: String) -> Result<Response, AppError> {
    Response::builder()
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body))
        .map_err(|err| AppError::Engine(format!("failed to build JSON response: {err}")))
}

fn batch_recommend_response_json(items: &[BatchRecommendResponseItem]) -> Result<String, AppError> {
    let mut out = String::with_capacity(
        items
            .iter()
            .map(|item| item.result.as_ref().map_or(96, |result| result.len() + 96))
            .sum::<usize>(),
    );
    out.push('[');
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('{');

        let mut wrote_field = false;
        if let Some(alg) = item.alg.as_deref() {
            push_json_field_prefix(&mut out, &mut wrote_field, "alg")?;
            push_json_string(&mut out, alg)?;
        }

        push_json_field_prefix(&mut out, &mut wrote_field, "cost_time")?;
        push_json_f64(&mut out, item.cost_time)?;
        push_json_field_prefix(&mut out, &mut wrote_field, "wait_time")?;
        push_json_f64(&mut out, item.wait_time)?;

        if let Some(result) = item.result.as_deref() {
            push_json_field_prefix(&mut out, &mut wrote_field, "result")?;
            out.push_str(result);
        }
        if let Some(error) = item.error.as_deref() {
            push_json_field_prefix(&mut out, &mut wrote_field, "error")?;
            push_json_string(&mut out, error)?;
        }

        out.push('}');
    }
    out.push(']');
    Ok(out)
}

fn merge_native_batch_results(
    native_json: &str,
    batch_options: &[String],
    wait_time: std::time::Duration,
) -> Result<Vec<BatchRecommendResponseItem>, AppError> {
    let native_items: Vec<NativeBatchRecommendItem> = sonic_rs::from_str(native_json)
        .map_err(|err| AppError::Engine(format!("invalid native batch response: {err}")))?;
    if native_items.len() != batch_options.len() {
        return Err(AppError::Engine(format!(
            "native batch returned {} items for {} requests",
            native_items.len(),
            batch_options.len()
        )));
    }

    Ok(native_items
        .into_iter()
        .zip(batch_options)
        .map(|(item, option_json)| {
            let alg = sonic_rs::from_str::<BatchRecommendItemMeta>(option_json)
                .ok()
                .and_then(|meta| meta.algorithm);
            BatchRecommendResponseItem {
                alg,
                cost_time: item.cost_time,
                wait_time: wait_time.as_secs_f64(),
                result: item.result,
                error: item.error,
            }
        })
        .collect())
}

fn push_json_field_prefix(
    out: &mut String,
    wrote_field: &mut bool,
    key: &str,
) -> Result<(), AppError> {
    if *wrote_field {
        out.push(',');
    } else {
        *wrote_field = true;
    }
    push_json_string(out, key)?;
    out.push(':');
    Ok(())
}

fn push_json_string(out: &mut String, value: &str) -> Result<(), AppError> {
    let encoded = sonic_rs::to_string(value)
        .map_err(|err| AppError::Engine(format!("failed to encode JSON string: {err}")))?;
    out.push_str(&encoded);
    Ok(())
}

fn push_json_f64(out: &mut String, value: f64) -> Result<(), AppError> {
    if !value.is_finite() {
        return Err(AppError::Engine(format!(
            "failed to encode non-finite JSON number: {value}"
        )));
    }
    out.push_str(&value.to_string());
    Ok(())
}

fn parse_single_decompressed_json_segment(body: &[u8], name: &str) -> Result<Vec<u8>, AppError> {
    let segments = extract_decompressed_segments(body)?;
    if segments.len() != 1 {
        return Err(AppError::BadRequest(format!(
            "{name} payload expects exactly one JSON segment"
        )));
    }

    Ok(segments.into_iter().next().unwrap())
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

    match engine.attach_cached_userdata(userdata_hash) {
        Ok(()) => {
            engine.remember_userdata_hash(userdata_hash);
            return Ok(());
        }
        Err(err) => {
            tracing::debug!(
                hash_prefix = %truncate_head(userdata_hash, 8),
                error = %err,
                "Shared userdata cache attach missed; falling back to parsing payload"
            );
        }
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

fn run_engine_exclusive_op<T, F>(
    state: &AppState,
    op_id: u64,
    op_name: &'static str,
    clear_userdata_cache_on_success: bool,
    f: F,
) -> Result<T, AppError>
where
    F: FnOnce(&crate::bridge::DeckRecommend) -> Result<T, String>,
{
    let span = tracing::debug_span!("engine_exclusive_op", op_id, op = op_name);
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
    tracing::debug!("Starting exclusive engine operation");
    let engine = engines
        .iter()
        .next()
        .ok_or_else(|| AppError::Engine("engine pool is empty".into()))?;
    let result = f(engine).map_err(AppError::Engine)?;
    if clear_userdata_cache_on_success {
        engines.clear_userdata_hashes();
        state.userdata_cache.clear();
        tracing::info!(
            op_id,
            op = op_name,
            "Cleared cached userdata state after exclusive engine update"
        );
    }
    let engine_elapsed = engine_started.elapsed();

    if engine_elapsed >= state.debug.engine_warn_threshold {
        tracing::warn!(
            engine_elapsed_ms = elapsed_ms(engine_elapsed),
            threshold_ms = elapsed_ms(state.debug.engine_warn_threshold),
            "Exclusive engine operation exceeded threshold"
        );
    } else {
        tracing::debug!(
            engine_elapsed_ms = elapsed_ms(engine_elapsed),
            "Exclusive engine operation completed"
        );
    }

    Ok(result)
}
fn elapsed_ms(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn truncate_head(value: &str, count: usize) -> String {
    value.chars().take(count).collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{batch_recommend_response_json, merge_native_batch_results};

    #[test]
    fn native_batch_results_preserve_item_success_and_error() {
        let options = vec![
            r#"{"algorithm":"dfs"}"#.to_string(),
            r#"{"algorithm":"ga"}"#.to_string(),
        ];
        let native = r#"[
            {"cost_time":0.125,"result":"{\"decks\":[]}"},
            {"cost_time":0.25,"error":"invalid option"}
        ]"#;

        let items = merge_native_batch_results(native, &options, Duration::from_millis(10))
            .expect("native batch response should parse");
        let response =
            batch_recommend_response_json(&items).expect("merged batch response should serialize");

        assert!(response.contains(r#""alg":"dfs""#));
        assert!(response.contains(r#""result":{"decks":[]}"#));
        assert!(response.contains(r#""alg":"ga""#));
        assert!(response.contains(r#""error":"invalid option""#));
        assert!(response.contains(r#""wait_time":0.01"#));
    }

    #[test]
    fn native_batch_results_reject_wrong_item_count() {
        let error = merge_native_batch_results("[]", &["{}".to_string()], Duration::ZERO)
            .expect_err("wrong item count should fail");
        assert!(
            error
                .to_string()
                .contains("returned 0 items for 1 requests")
        );
    }
}
