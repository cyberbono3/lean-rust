//! HTTP error surface for the `/eth/v1/...` handlers.
//!
//! Each variant maps onto a status code via the [`IntoResponse`] impl;
//! the response body is the JSON object `{"error": "<Display>"}`.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Errors surfaced by the lean-api HTTP handlers.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HttpError {
    /// `Store::load_head` returned `Ok(None)` — the runtime has not yet
    /// persisted a canonical head.
    #[error("head not yet set")]
    HeadNotSet,

    /// Underlying [`storage::Store`] reported a backend failure. The
    /// inner `Display` is surfaced in the JSON body; acceptable at
    /// devnet0 (unauthenticated, no sensitive backend) — production
    /// deployments may want a generic message instead.
    #[error(transparent)]
    Storage(#[from] storage::StorageError),
}

impl HttpError {
    /// HTTP status code paired with this error in [`IntoResponse`].
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::HeadNotSet => StatusCode::NOT_FOUND,
            Self::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// JSON wire shape for error responses: `{"error": "<message>"}`.
#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.to_string(),
        };
        (self.status(), Json(body)).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn assert_response(err: HttpError, want_status: StatusCode, want_body: &[u8]) {
        let response = err.into_response();
        assert_eq!(response.status(), want_status);
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(&body[..], want_body);
    }

    #[tokio::test]
    async fn into_response_maps_status_and_body() {
        let cases: [(HttpError, StatusCode, &[u8]); 2] = [
            (
                HttpError::HeadNotSet,
                StatusCode::NOT_FOUND,
                br#"{"error":"head not yet set"}"#,
            ),
            (
                HttpError::Storage(storage::StorageError::Backend {
                    message: "boom".to_owned(),
                }),
                StatusCode::INTERNAL_SERVER_ERROR,
                br#"{"error":"storage backend error: boom"}"#,
            ),
        ];
        for (err, want_status, want_body) in cases {
            assert_response(err, want_status, want_body).await;
        }
    }
}
