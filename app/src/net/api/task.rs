use super::build;
use crate::net::requests::Request;
use std::net::SocketAddr;
use tokio::{net::TcpListener, sync::mpsc};

pub async fn task(addr: SocketAddr, requests: mpsc::Sender<Request>) -> color_eyre::Result<()> {
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("API listening on http://{}/", listener.local_addr()?);

    let router = build(requests);

    axum::serve(listener, router.into_make_service()).await?;

    Ok(())
}
