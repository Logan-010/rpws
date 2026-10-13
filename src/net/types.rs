use std::collections::HashSet;

use crate::{
    config::Config,
    net::{relays::Relays, swarm, tasks},
};
use libp2p::{
    Multiaddr, PeerId, StreamProtocol, Swarm, autonat, dcutr, identify, identity::Keypair, kad,
    mdns, multiaddr::Protocol, ping, relay, swarm::NetworkBehaviour,
};
use redb::Database;

pub struct Net {
    pub(super) swarm: Swarm<Behaviour>,
    pub(super) relays: Option<Relays>,
    pub(super) provider_key: Option<kad::RecordKey>,
}

pub struct NetApi {}

impl Net {
    pub const BOOTSTRAP_CODEC: u64 = 0x77;

    const BOOTNODES: [&str; 4] = [
        "QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
        "QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
        "QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
        "QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
    ];
    const BOOTSTRAP_ADDR: &str = "/dnsaddr/bootstrap.libp2p.io";

    pub async fn new(config: &Config) -> color_eyre::Result<Self> {
        let swarm = swarm::build_swarm(config).await?;

        Ok(Self {
            swarm,
            relays: None,
            provider_key: None,
        })
    }

    pub async fn bootstrap<B: AsRef<[u8]>>(
        &mut self,
        local: bool,
        of: B,
    ) -> color_eyre::Result<()> {
        if !local {
            self.relays = Some(Relays::default());
            let base_addr = Self::BOOTSTRAP_ADDR
                .parse::<Multiaddr>()
                .expect("Valid address");

            for id in Self::BOOTNODES {
                let peer = id.parse().expect("Valid ID");
                let addr = base_addr.clone().with(Protocol::P2p(peer));

                self.swarm
                    .behaviour_mut()
                    .ipfs_kad
                    .add_address(&peer, addr.clone());
            }
            self.swarm.behaviour_mut().ipfs_kad.bootstrap()?;
        }

        tasks::bootstrap(self, of).await
    }

    pub fn cleanup(&mut self) {
        // Rendezvous discovery is complete; do not restart IPFS provider queries.
        self.provider_key = None;
        for mut query in self.swarm.behaviour_mut().ipfs_kad.iter_queries_mut() {
            query.finish();
        }
        let rpws_peers: HashSet<PeerId> = self
            .swarm
            .behaviour_mut()
            .rpws_kad
            .kbuckets()
            .flat_map(|bucket| {
                bucket
                    .iter()
                    .map(|entry| *entry.node.key.preimage())
                    .collect::<Vec<_>>()
            })
            .collect();

        let ipfs_peers: Vec<PeerId> = self
            .swarm
            .behaviour_mut()
            .ipfs_kad
            .kbuckets()
            .flat_map(|bucket| {
                bucket
                    .iter()
                    .map(|entry| *entry.node.key.preimage())
                    .collect::<Vec<_>>()
            })
            .collect();

        for peer in &ipfs_peers {
            self.swarm.behaviour_mut().ipfs_kad.remove_peer(peer);
        }

        let local_peer_id = *self.swarm.local_peer_id();

        *self.swarm.behaviour_mut().ipfs_kad.store_mut() =
            kad::store::MemoryStore::new(local_peer_id);

        let disconnect_peers: Vec<PeerId> = self
            .swarm
            .connected_peers()
            .filter(|peer| {
                !rpws_peers.contains(peer)
                    && !self
                        .relays
                        .as_ref()
                        .is_some_and(|relays| relays.has_reservation(peer))
            })
            .copied()
            .collect();

        for peer in disconnect_peers {
            let _ = self.swarm.disconnect_peer_id(peer);
        }
    }

    pub fn api(&mut self) -> NetApi {
        NetApi {}
    }

    pub async fn run(self) -> color_eyre::Result<()> {
        tasks::run(self).await
    }
}

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    pub ipfs_kad: kad::Behaviour<kad::store::MemoryStore>,
    pub rpws_kad: kad::Behaviour<libp2p_kad_redb::RedbStore>,
    dcutr: dcutr::Behaviour,
    mdns: mdns::tokio::Behaviour,
    relay_client: relay::client::Behaviour,
    autonat: autonat::v2::client::Behaviour,
}

impl Behaviour {
    const CLIENT_PROTOCOL: &str = concat!("/rpws/", env!("CARGO_PKG_VERSION"));
    pub const IPFS_KAD_PROTOCOL: &str = "/ipfs/kad/1.0.0";
    pub const RPWS_KAD_PROTOCOL: &str = "/rpws/kad/1.0.0";

    pub fn new(
        keypair: &Keypair,
        relay_client: relay::client::Behaviour,
        database: Database,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            ping: ping::Behaviour::new(ping::Config::new()),
            identify: identify::Behaviour::new(
                identify::Config::new(String::from(Self::CLIENT_PROTOCOL), keypair.public())
                    .with_push_listen_addr_updates(true),
            ),
            ipfs_kad: kad::Behaviour::with_config(
                keypair.public().to_peer_id(),
                kad::store::MemoryStore::new(keypair.public().to_peer_id()),
                kad::Config::new(StreamProtocol::new(Self::IPFS_KAD_PROTOCOL))
                    .set_record_filtering(kad::StoreInserts::FilterBoth)
                    .clone(),
            ),
            rpws_kad: kad::Behaviour::with_config(
                keypair.public().to_peer_id(),
                libp2p_kad_redb::RedbStore::new(keypair.public().to_peer_id(), database),
                kad::Config::new(StreamProtocol::new(Self::RPWS_KAD_PROTOCOL))
                    .set_record_filtering(kad::StoreInserts::FilterBoth)
                    .clone(),
            ),
            dcutr: dcutr::Behaviour::new(keypair.public().to_peer_id()),
            mdns: mdns::tokio::Behaviour::new(
                mdns::Config {
                    enable_ipv6: true,
                    ..Default::default()
                },
                keypair.public().to_peer_id(),
            )?,
            relay_client,
            autonat: autonat::v2::client::Behaviour::default(),
        })
    }
}
