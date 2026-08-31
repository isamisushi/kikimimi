//! Central error → HTTP response mapping. Every handler returns
//! `Result<T, AppError>` so status codes stay consistent with the API
//! contract (401/400/413/422/429/500) without repeating `IntoResponse` impls.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub enum AppError {
    Unauthorized(&'static str),
    BadRequest(String),
    UnprocessableEntity(String),
    PayloadTooLarge(String),
    NotFound(String),
    TooManyRequests { retry_after_secs: u64 },
    Internal(anyhow::Error),
}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        AppError::Internal(e)
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        AppError::Internal(e.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::Unauthorized(msg) => {
                (StatusCode::UNAUTHORIZED, Json(json!({ "error": msg }))).into_response()
            }
            AppError::BadRequest(msg) => {
                (StatusCode::BAD_REQUEST, Json(json!({ "error": msg }))).into_response()
            }
            AppError::UnprocessableEntity(msg) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": msg })),
            )
                .into_response(),
            AppError::PayloadTooLarge(msg) => {
                (StatusCode::PAYLOAD_TOO_LARGE, Json(json!({ "error": msg }))).into_response()
            }
            AppError::NotFound(msg) => {
                (StatusCode::NOT_FOUND, Json(json!({ "error": msg }))).into_response()
            }
            AppError::TooManyRequests { retry_after_secs } => (
                StatusCode::TOO_MANY_REQUESTS,
                [("Retry-After", retry_after_secs.to_string())],
                Json(json!({ "error": "overloaded" })),
            )
                .into_response(),
            AppError::Internal(e) => {
                // NEVER log request/response bodies here (may contain tokens
                // via mis-set headers); log only the error chain.
                tracing::error!(error = ?e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "internal error" })),
                )
                    .into_response()
            }
        }
    }
}
