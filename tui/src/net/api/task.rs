use super::build;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub async fn task(addr: SocketAddr) -> color_eyre::Result<()> {
    let listener = TcpListener::bind(addr).await?;

    tracing::info!("API listening on http://{}/", listener.local_addr()?);

    let router = build();

    axum::serve(listener, router.into_make_service()).await?;

    Ok(())
}
