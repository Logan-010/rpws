use super::Behaviour;
use crate::config::Config;
use color_eyre::eyre::bail;
use libp2p::{Swarm, SwarmBuilder, kad, multiaddr::Protocol, noise, tcp, yamux};
use redb::Database;
use std::time::Duration;
use tokio::{fs, task};

pub async fn build_swarm(config: &Config) -> color_eyre::Result<Swarm<Behaviour>> {
    let identity = config.identity()?;

    tracing::info!("Peer id: {}", identity.public().to_peer_id());

    if let Some(p) = config.kademlia_store_path().parent()
        && !fs::try_exists(p).await?
    {
        fs::create_dir_all(p).await?;
    }

    let path = config.kademlia_store_path().to_path_buf();
    tracing::debug!("Opening database at {}", path.display());
    let database = task::spawn_blocking(move || Database::create(path)).await??;

    tracing::debug!("Creating swarm");
    let mut swarm = SwarmBuilder::with_existing_identity(identity)
        .with_tokio()
        .with_tcp(
            tcp::Config::new().nodelay(true),
            noise::Config::new,
            yamux::Config::default,
        )?
        .with_quic()
        .with_dns()?
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await?
        .with_relay_client(noise::Config::new, yamux::Config::default)?
        .with_behaviour(|keypair, relay_client| {
            Ok(Behaviour::new(keypair, relay_client, database)?)
        })?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .with_connection_timeout(Duration::from_secs(20))
        .build();
    swarm
        .behaviour_mut()
        .ipfs_kad
        .set_mode(Some(kad::Mode::Server));
    swarm
        .behaviour_mut()
        .rpws_kad
        .set_mode(Some(kad::Mode::Server));

    for addr in config.listen_addrs() {
        tracing::debug!("Telling swarm to listen on {addr} event");
        swarm.listen_on(addr.clone())?;
    }

    for addr in config.external_addrs() {
        tracing::debug!("Adding external address {addr}");
        swarm.add_external_address(addr.clone());
    }

    for mut addr in config.bootstrap_addrs().iter().cloned() {
        let Some(Protocol::P2p(id)) = addr.pop() else {
            bail!("Bootstrap address must end with /p2p");
        };

        swarm.behaviour_mut().ipfs_kad.add_address(&id, addr);
    }

    Ok(swarm)
}
