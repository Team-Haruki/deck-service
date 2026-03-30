use std::sync::Arc;

use axum::extract::State;
use axum::Json;

use crate::error::AppError;
use crate::models::{
    DeckRecommendOptions, DeckRecommendResult, UpdateMasterdataFromJsonRequest,
    UpdateMasterdataRequest, UpdateMusicmetasFromStringRequest, UpdateMusicmetasRequest,
};
use crate::state::AppState;

pub async fn health() -> &'static str {
    "ok"
}

pub async fn recommend(
    State(state): State<Arc<AppState>>,
    Json(options): Json<DeckRecommendOptions>,
) -> Result<Json<DeckRecommendResult>, AppError> {
    let engine = state.engine.lock().map_err(|e| AppError::Engine(e.to_string()))?;
    let result = tokio::task::block_in_place(|| engine.recommend(&options))
        .map_err(AppError::Engine)?;
    Ok(Json(result))
}

pub async fn update_masterdata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state.engine.lock().map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_masterdata(&req.base_dir, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(sonic_rs::json!({ "status": "ok" })))
}

pub async fn update_masterdata_from_json(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMasterdataFromJsonRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state.engine.lock().map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_masterdata_from_json(&req.data, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(sonic_rs::json!({ "status": "ok" })))
}

pub async fn update_musicmetas(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state.engine.lock().map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_musicmetas(&req.file_path, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(sonic_rs::json!({ "status": "ok" })))
}

pub async fn update_musicmetas_from_string(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UpdateMusicmetasFromStringRequest>,
) -> Result<Json<sonic_rs::Value>, AppError> {
    let engine = state.engine.lock().map_err(|e| AppError::Engine(e.to_string()))?;
    tokio::task::block_in_place(|| engine.update_musicmetas_from_string(&req.data, &req.region))
        .map_err(AppError::Engine)?;
    Ok(Json(sonic_rs::json!({ "status": "ok" })))
}
