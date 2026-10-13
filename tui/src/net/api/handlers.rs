use crate::net::requests::Request;
use axum::{body::Bytes, extract::State, http::StatusCode};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);

type ApiError = (StatusCode, &'static str);

async fn request<T>(
    sender: mpsc::Sender<Request>,
    request: Request,
    reply: oneshot::Receiver<color_eyre::Result<T>>,
) -> Result<T, ApiError> {
    tokio::time::timeout(REQUEST_TIMEOUT, async {
        sender
            .send(request)
            .await
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Network unavailable"))?;
        reply
            .await
            .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Network unavailable"))?
            .map_err(|error| {
                tracing::error!(%error, "API network request failed");
                (StatusCode::BAD_GATEWAY, "Network request failed")
            })
    })
    .await
    .map_err(|_| (StatusCode::GATEWAY_TIMEOUT, "Network request timed out"))?
}

pub(super) async fn get(
    State(sender): State<mpsc::Sender<Request>>,
    body: Bytes,
) -> Result<([(axum::http::HeaderName, &'static str); 1], Vec<u8>), ApiError> {
    let name: String =
        postcard::from_bytes(&body).map_err(|_| (StatusCode::BAD_REQUEST, "Invalid name"))?;
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Name must not be empty"));
    }
    let (reply, response) = oneshot::channel();
    let payload = request(sender, Request::Get { name, reply }, response)
        .await?
        .ok_or((StatusCode::NOT_FOUND, "Password not found"))?;
    let body = postcard::to_stdvec(&payload).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not encode password",
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        body,
    ))
}

pub(super) async fn put(
    State(sender): State<mpsc::Sender<Request>>,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let (name, payload): (String, shared::RecordPayload) =
        postcard::from_bytes(&body).map_err(|_| (StatusCode::BAD_REQUEST, "Invalid password"))?;
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Name must not be empty"));
    }
    let (reply, response) = oneshot::channel();
    request(
        sender,
        Request::Put {
            name,
            payload,
            reply,
        },
        response,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list(
    State(sender): State<mpsc::Sender<Request>>,
) -> Result<([(axum::http::HeaderName, &'static str); 1], Vec<u8>), ApiError> {
    let (reply, response) = oneshot::channel();
    let payloads = request(sender, Request::List { reply }, response).await?;
    let body = postcard::to_stdvec(&payloads).map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not encode passwords",
        )
    })?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        body,
    ))
}
