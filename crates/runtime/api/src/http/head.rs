//! `GET /eth/v1/head` handler.
//!
//! Reads the current canonical view from [`storage::Store::load_head`]
//! and returns it as a [`HeadInfoDto`]. `Ok(None)` from the store
//! surfaces as [`HttpError::HeadNotSet`] (404); backend failures
//! surface as [`HttpError::Storage`] (500). Both paths are produced by
//! the [`super::error`] `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, Json};
use storage::Store;

use super::error::HttpError;
use super::store_snapshot::HeadInfoDto;

/// Canonical mount path for [`get_head`]. Shared between
/// [`crate::http::service::HttpService`]'s router build and the
/// integration tests so the route stays in one place.
pub(crate) const PATH: &str = "/eth/v1/head";

/// `GET /eth/v1/head` axum handler.
///
/// The route is wired up in [`crate::HttpService::start`]; the handler
/// is `pub(crate)` so the service builds the router from this module.
pub(crate) async fn get_head(
    State(store): State<Arc<dyn Store>>,
) -> Result<Json<HeadInfoDto>, HttpError> {
    store
        .load_head()?
        .map(HeadInfoDto::from)
        .map(Json)
        .ok_or(HttpError::HeadNotSet)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        response::Response,
        routing::get,
        Router,
    };
    use protocol::{Checkpoint, Slot};
    use storage::{HeadInfo, MemoryStore};
    use tower::ServiceExt;
    use types::Bytes32;

    fn router(store: Arc<dyn Store>) -> Router {
        Router::new().route(PATH, get(get_head)).with_state(store)
    }

    async fn get_head_response(store: Arc<dyn Store>) -> Response {
        router(store)
            .oneshot(Request::builder().uri(PATH).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn populated_store_returns_200_with_head() {
        let info = HeadInfo::new(
            Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(5)),
            Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(2)),
        );

        let store = Arc::new(MemoryStore::default());
        store.save_head(info).unwrap();

        let response = get_head_response(store).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_string(response).await;
        let expected = serde_json::to_string(&HeadInfoDto::from(info)).unwrap();
        assert_eq!(body, expected);
    }

    #[tokio::test]
    async fn empty_store_returns_404() {
        let response = get_head_response(Arc::new(MemoryStore::default())).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = body_string(response).await;
        assert_eq!(body, r#"{"error":"head not yet set"}"#);
    }
}
