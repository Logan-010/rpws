use super::handlers;
use crate::net::requests::Request;
use axum::{
    Router,
    http::StatusCode,
    routing::{get, post, put},
};
use tokio::sync::mpsc;

pub fn build(requests: mpsc::Sender<Request>) -> Router {
    Router::new()
        .route("/health", get(|| async { (StatusCode::OK, "Running!") }))
        .route("/get", post(handlers::get))
        .route("/put", put(handlers::put))
        .with_state(requests)
}
