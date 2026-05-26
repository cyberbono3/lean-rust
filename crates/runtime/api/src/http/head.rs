//! `GET /eth/v1/head`, `GET /lean/v0/head`, and
//! `GET /lean/v0/head/full` handlers.
//!
//! Reads the current canonical view from [`storage::Store::load_head`]
//! and returns either the Ream-compatible head-root shape or the richer
//! diagnostic [`HeadInfoDto`]. `Ok(None)` from the store surfaces as
//! [`HttpError::HeadNotSet`] (404); backend failures surface as
//! [`HttpError::Storage`] (500). Both paths are produced by the
//! [`super::error`] `IntoResponse` impl.

use std::sync::Arc;

use axum::{extract::State, routing::get, Json, Router};
use storage::{HeadInfo, Store};
use tracing::{debug, info};

use super::error::HttpError;
use super::store_snapshot::{HeadInfoDto, HeadRootDto};
use super::{FULL_HEAD_PATHS, LEAN_V0_HEAD_PATH};

fn load_head(store: &dyn Store) -> Result<HeadInfo, HttpError> {
    if let Some(head) = store.load_head()? {
        info!(
            head_slot = head.head.slot.get(),
            head_root = %head.head.root.to_hex(),
            finalized_slot = head.finalized.slot.get(),
            finalized_root = %head.finalized.root.to_hex(),
            "served head endpoint",
        );
        Ok(head)
    } else {
        debug!("head not yet set");
        Err(HttpError::HeadNotSet)
    }
}

/// Ream-compatible head endpoint axum handler for
/// [`super::LEAN_V0_HEAD_PATH`].
pub(crate) async fn get_head(
    State(store): State<Arc<dyn Store>>,
) -> Result<Json<HeadRootDto>, HttpError> {
    load_head(store.as_ref()).map(|head| Json(HeadRootDto::from(head)))
}

/// Full diagnostic head endpoint axum handler for
/// [`super::ETH_V1_HEAD_PATH`] and [`super::LEAN_V0_HEAD_FULL_PATH`].
pub(crate) async fn get_head_full(
    State(store): State<Arc<dyn Store>>,
) -> Result<Json<HeadInfoDto>, HttpError> {
    load_head(store.as_ref()).map(|head| Json(HeadInfoDto::from(head)))
}

pub(crate) fn router() -> Router<Arc<dyn Store>> {
    FULL_HEAD_PATHS
        .into_iter()
        .fold(Router::new(), |router, path| {
            router.route(path, get(get_head_full))
        })
        .route(LEAN_V0_HEAD_PATH, get(get_head))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::{Request, StatusCode},
        response::Response,
        Router,
    };
    use protocol::{Checkpoint, Slot};
    use storage::{HeadInfo, MemoryStore};
    use tower::ServiceExt;
    use types::Bytes32;

    use super::super::HEAD_PATHS;

    fn router_with_store(store: Arc<dyn Store>) -> Router {
        router().with_state(store)
    }

    async fn get_head_response(store: Arc<dyn Store>, path: &str) -> Response {
        router_with_store(store)
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_string(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn populated_store_returns_200_with_ream_compatible_head() {
        let info = HeadInfo::new(
            Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(5)),
            Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(2)),
        );

        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        store.save_head(info).unwrap();

        let response = get_head_response(Arc::clone(&store), LEAN_V0_HEAD_PATH).await;
        assert_eq!(response.status(), StatusCode::OK);

        let body = body_string(response).await;
        assert_eq!(
            body,
            r#"{"head":"0x1111111111111111111111111111111111111111111111111111111111111111"}"#
        );
    }

    #[tokio::test]
    async fn populated_store_returns_200_with_full_diagnostic_head() {
        let info = HeadInfo::new(
            Checkpoint::new(Bytes32::new([0x11; 32]), Slot::new(5)),
            Checkpoint::new(Bytes32::new([0x22; 32]), Slot::new(2)),
        );

        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        store.save_head(info).unwrap();

        let expected = serde_json::to_string(&HeadInfoDto::from(info)).unwrap();
        for path in FULL_HEAD_PATHS {
            let response = get_head_response(Arc::clone(&store), path).await;
            assert_eq!(response.status(), StatusCode::OK, "path {path}");

            let body = body_string(response).await;
            assert_eq!(body, expected, "path {path}");
        }
    }

    #[tokio::test]
    async fn empty_store_returns_404() {
        let store: Arc<dyn Store> = Arc::new(MemoryStore::default());
        for path in HEAD_PATHS {
            let response = get_head_response(Arc::clone(&store), path).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "path {path}");
            let body = body_string(response).await;
            assert_eq!(body, r#"{"error":"head not yet set"}"#, "path {path}");
        }
    }
}
