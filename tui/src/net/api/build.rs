use super::handlers;
use crate::net::requests::Request;
use axum::{
    Router,
    http::StatusCode,
    routing::{delete, get, post, put},
};
use tokio::sync::mpsc;

pub fn build(requests: mpsc::Sender<Request>) -> Router {
    Router::new()
        .route("/health", get(|| async { (StatusCode::OK, "Running!") }))
        .route("/get", post(handlers::get))
        .route("/list", get(handlers::list))
        .route("/put", put(handlers::put))
        .route("/delete", delete(handlers::delete))
        .with_state(requests)
}
