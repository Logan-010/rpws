use axum::{Router, http::StatusCode, routing::get};

pub fn build() -> Router {
    Router::new().route("/health", get(|| async { (StatusCode::OK, "Running!") }))
}
