use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use sonic_rs::{JsonValueTrait, json};

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
    expect_octet_stream_content_type(&headers)?;

    let segments = extract_decompressed_segments(body.as_ref())?;
    if segments.len() != 1 {
        return Err(AppError::BadRequest(
            "cache_userdata expects exactly one payload segment".into(),
        ));
    }

    let userdata = String::from_utf8(segments.into_iter().next().unwrap())
        .map_err(|_| AppError::BadRequest("userdata payload must be valid UTF-8 JSON".into()))?;

    let userdata_hash = tokio::task::block_in_place(|| {
        let engine = state
            .engine
            .lock()
            .map_err(|e| AppError::Engine(e.to_string()))?;
        engine.cache_userdata(&userdata).map_err(AppError::Engine)
    })?;

    Ok(Json(CacheUserdataResponse { userdata_hash }).into_response())
}

pub async fn recommend(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, AppError> {
    let content_type = request_content_type(&headers);
    if is_octet_stream_content_type(&content_type) {
        return recommend_batch(state, body).await;
    }
    if is_json_content_type(&content_type) {
        return recommend_legacy(state, body).await;
    }

    Err(AppError::UnsupportedMediaType(format!(
        "unsupported content type for /recommend: {content_type}"
    )))
}

pub async fn update_masterdata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let resolved_base_dir = resolve_masterdata_base_dir(&req.base_dir, &req.region);
    let engine = state
        .engine
        .lock()
        .map_err(|e| AppError::Engine(e.to_string()))?;
    if resolved_base_dir != req.base_dir {
        tracing::info!(
            requested_base_dir = %req.base_dir,
            resolved_base_dir = %resolved_base_dir,
            region = %req.region,
            "Resolved masterdata path for deck-service"
        );
    }
    tokio::task::block_in_place(|| engine.update_masterdata(&resolved_base_dir, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_masterdata_from_json(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataFromJsonRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state
        .engine
        .lock()
        .map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_masterdata_from_json(&req.data, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_musicmetas(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state
        .engine
        .lock()
        .map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_musicmetas(&req.file_path, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn update_musicmetas_from_string(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasFromStringRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state
        .engine
        .lock()
        .map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_musicmetas_from_string(&req.data, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(json!({ "status": "ok" })))
}

async fn recommend_legacy(state: Arc<AppState>, body: Bytes) -> Result<Response, AppError> {
    let options: DeckRecommendOptions = sonic_rs::from_slice(body.as_ref())
        .map_err(|e| AppError::BadRequest(format!("invalid recommend payload: {e}")))?;

    let result: DeckRecommendResult = tokio::task::block_in_place(|| {
        let engine = state
            .engine
            .lock()
            .map_err(|e| AppError::Engine(e.to_string()))?;
        engine.recommend(&options).map_err(AppError::Engine)
    })?;

    Ok(Json(result).into_response())
}

async fn recommend_batch(state: Arc<AppState>, body: Bytes) -> Result<Response, AppError> {
    let req = parse_batch_recommend_request(body.as_ref())?;

    if req.batch_options.is_empty() {
        return Err(AppError::BadRequest(
            "batch_options must contain at least one recommend option".into(),
        ));
    }
    if req.userdata_hash.trim().is_empty() {
        return Err(AppError::BadRequest("userdata_hash is required".into()));
    }

    let results = tokio::task::block_in_place(|| {
        let engine = state
            .engine
            .lock()
            .map_err(|e| AppError::Engine(e.to_string()))?;

        let region = req.region;
        let userdata_hash = req.userdata_hash;
        let mut responses = Vec::with_capacity(req.batch_options.len());
        for mut option in req.batch_options {
            let alg = option
                .get("algorithm")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned);

            option.insert("region".into(), json!(region.as_str()));
            option.insert("userdata_hash".into(), json!(userdata_hash.as_str()));

            let started = Instant::now();
            match engine.recommend_value(&option) {
                Ok(result) => responses.push(BatchRecommendResponseItem {
                    alg,
                    cost_time: started.elapsed().as_secs_f64(),
                    wait_time: 0.0,
                    result: Some(result),
                    error: None,
                }),
                Err(err) => {
                    tracing::warn!(
                        region = %region,
                        algorithm = alg.as_deref().unwrap_or(""),
                        error = %err,
                        "Batch deck recommendation failed"
                    );
                    responses.push(BatchRecommendResponseItem {
                        alg,
                        cost_time: started.elapsed().as_secs_f64(),
                        wait_time: 0.0,
                        result: None,
                        error: Some(err),
                    });
                }
            }
        }
        Ok::<Vec<BatchRecommendResponseItem>, AppError>(responses)
    })?;

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
