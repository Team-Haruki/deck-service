use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sonic_rs::json;

#[derive(Debug)]
pub enum AppError {
    Engine(String),
    Timeout(String),
    BadRequest(String),
    UnsupportedMediaType(String),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Engine(msg) => write!(f, "Engine error: {msg}"),
            AppError::Timeout(msg) => write!(f, "Timeout: {msg}"),
            AppError::BadRequest(msg) => write!(f, "Bad request: {msg}"),
            AppError::UnsupportedMediaType(msg) => write!(f, "Unsupported media type: {msg}"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Engine(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            AppError::Timeout(msg) => (StatusCode::GATEWAY_TIMEOUT, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::UnsupportedMediaType(msg) => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, msg.clone())
            }
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}
