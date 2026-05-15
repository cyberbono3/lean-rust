//! Error surface for the Prometheus `/metrics` handler.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Errors surfaced by metrics collection or Prometheus text encoding.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MetricsError {
    /// Prometheus rejected a collector, metric descriptor, or encode
    /// operation.
    #[error(transparent)]
    Prometheus(#[from] prometheus::Error),

    /// A `u64` provider returned a value that cannot be represented by
    /// Prometheus' integer gauge type.
    #[error("metric {name} value {value} exceeds i64::MAX")]
    ValueOutOfRange {
        /// Metric name whose provider produced the out-of-range value.
        name: String,
        /// Value returned by the provider.
        value: u64,
    },
}

impl MetricsError {
    /// HTTP status code paired with this error in [`IntoResponse`].
    ///
    /// Metrics scrape failures are all server-side failures: provider
    /// values were invalid, collector registration failed, or text
    /// encoding failed.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

impl IntoResponse for MetricsError {
    fn into_response(self) -> Response {
        (self.status(), self.to_string()).into_response()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn into_response_maps_to_500_plain_text() {
        let err = MetricsError::ValueOutOfRange {
            name: "lean_too_large".to_owned(),
            value: u64::MAX,
        };
        let want_status = err.status();
        let response = err.into_response();
        assert_eq!(response.status(), want_status);

        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(
            &body[..],
            b"metric lean_too_large value 18446744073709551615 exceeds i64::MAX"
        );
    }
}
