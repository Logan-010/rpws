use crate::{api::store, save::Save};
use color_eyre::eyre::bail;
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder, Transport, core::upgrade::Version, identify,
    identity::Keypair, kad, multiaddr::Protocol, noise, ping, swarm::NetworkBehaviour,
    websocket_websys as websocket, yamux,
};
use std::time::Duration;

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    pub ipfs_kad: kad::Behaviour<kad::store::MemoryStore>,
    pub rpws_kad: kad::Behaviour<store::SerializableMemoryStore>,
}

impl Behaviour {
    const CLIENT_PROTOCOL: &str = concat!("/rpws/web/", env!("CARGO_PKG_VERSION"));
    pub const IPFS_KAD_PROTOCOL: &str = "/ipfs/kad/1.0.0";
    pub const RPWS_KAD_PROTOCOL: &str = "/rpws/kad/1.0.0";

    pub fn new(keypair: &Keypair, store: Option<store::SerializableMemoryStore>) -> Self {
        Self {
            ping: ping::Behaviour::new(ping::Config::new()),
            identify: identify::Behaviour::new(identify::Config::new(
                String::from(Self::CLIENT_PROTOCOL),
                keypair.public(),
            )),
            ipfs_kad: kad::Behaviour::with_config(
                keypair.public().to_peer_id(),
                kad::store::MemoryStore::new(keypair.public().to_peer_id()),
                kad::Config::new(StreamProtocol::new(Self::IPFS_KAD_PROTOCOL))
                    .set_record_filtering(kad::StoreInserts::FilterBoth)
                    .clone(),
            ),
            rpws_kad: kad::Behaviour::with_config(
                keypair.public().to_peer_id(),
                store.unwrap_or_else(|| {
                    store::SerializableMemoryStore::new(keypair.public().to_peer_id())
                }),
                kad::Config::new(StreamProtocol::new(Self::RPWS_KAD_PROTOCOL))
                    .set_record_filtering(kad::StoreInserts::FilterBoth)
                    .clone(),
            ),
        }
    }
}

pub async fn new_swarm(save: Save) -> color_eyre::Result<Swarm<Behaviour>> {
    let identity = save.identity()?;

    let transport = websocket::Transport::default()
        .upgrade(Version::V1)
        .authenticate(noise::Config::new(&identity)?)
        .multiplex(yamux::Config::default())
        .boxed();

    let mut swarm = SwarmBuilder::with_existing_identity(identity)
        .with_wasm_bindgen()
        .with_other_transport(|_| transport)?
        .with_behaviour(|keypair| Behaviour::new(keypair, save.store))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(15)))
        .build();
    swarm
        .behaviour_mut()
        .ipfs_kad
        .set_mode(Some(kad::Mode::Client));
    swarm
        .behaviour_mut()
        .rpws_kad
        .set_mode(Some(kad::Mode::Client));

    for mut addr in save.bootstrap_addrs {
        let Some(Protocol::P2p(id)) = addr.pop() else {
            bail!("Bootstrap addr must end in /p2p")
        };

        swarm.behaviour_mut().ipfs_kad.add_address(&id, addr);
    }

    Ok(swarm)
}
