use crate::{
    config::Config,
    net::{relays::Relays, requests::Request, swarm, tasks},
};
use libp2p::{
    Multiaddr, StreamProtocol, Swarm, autonat, dcutr, identify,
    identity::Keypair,
    kad, mdns,
    multiaddr::Protocol,
    ping, relay, request_response,
    swarm::{NetworkBehaviour, behaviour::toggle::Toggle},
};
use redb::Database;
use tokio::sync::mpsc;

pub struct Net {
    pub(super) swarm: Swarm<Behaviour>,
    pub(super) secret: [u8; 32],
    pub(super) relays: Option<Relays>,
    pub(super) provider_key: Option<kad::RecordKey>,
    api: mpsc::Sender<Request>,
    pub(super) requests: mpsc::Receiver<Request>,
    pub(super) sync_peers: std::collections::HashSet<libp2p::PeerId>,
    pub(super) sync_pending:
        std::collections::HashMap<request_response::OutboundRequestId, libp2p::PeerId>,
}

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
        let (api, requests) = mpsc::channel(32);

        Ok(Self {
            swarm,
            secret: shared::derive_key(config.secret()?),
            relays: None,
            provider_key: None,
            api,
            requests,
            sync_peers: Default::default(),
            sync_pending: Default::default(),
        })
    }

    pub fn api(&self) -> mpsc::Sender<Request> {
        self.api.clone()
    }

    pub async fn run<B: AsRef<[u8]>>(&mut self, local: bool, of: B) -> color_eyre::Result<()> {
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

        tasks::run(self, of).await
    }
}

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    pub ipfs_kad: kad::Behaviour<kad::store::MemoryStore>,
    pub rpws_kad: kad::Behaviour<libp2p_kad_redb::RedbStore>,
    pub sync: request_response::cbor::Behaviour<shared::sync::Request, shared::sync::Response>,
    dcutr: dcutr::Behaviour,
    mdns: Toggle<mdns::tokio::Behaviour>,
    relay_client: relay::client::Behaviour,
    autonat: autonat::v2::client::Behaviour,
}

impl Behaviour {
    const CLIENT_PROTOCOL: &str = concat!("/rpws/", env!("CARGO_PKG_VERSION"));
    pub const IPFS_KAD_PROTOCOL: &str = "/ipfs/kad/1.0.0";
    pub const RPWS_KAD_PROTOCOL: &str = "/rpws/kad/2.0.0";

    pub fn new(
        keypair: &Keypair,
        relay_client: relay::client::Behaviour,
        database: Database,
    ) -> Result<Self, std::io::Error> {
        Ok(Self {
            sync: request_response::cbor::Behaviour::new(
                [(
                    StreamProtocol::new(shared::sync::PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default()
                    .with_request_timeout(std::time::Duration::from_secs(15)),
            ),
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
            mdns: Toggle::from(Some(mdns::tokio::Behaviour::new(
                mdns::Config {
                    enable_ipv6: true,
                    ..Default::default()
                },
                keypair.public().to_peer_id(),
            )?)),
            relay_client,
            autonat: autonat::v2::client::Behaviour::default(),
        })
    }

    #[cfg(test)]
    pub(super) fn disable_mdns(&mut self) {
        self.mdns = Toggle::from(None);
    }
}
