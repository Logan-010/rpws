use crate::{
    config::Config,
    net::{swarm, tasks},
};
use libp2p::{
    Multiaddr, StreamProtocol, Swarm, autonat, dcutr, identify, identity::Keypair, kad, mdns,
    multiaddr::Protocol, ping, relay, swarm::NetworkBehaviour,
};
use redb::Database;

pub struct Net {
    pub(super) swarm: Swarm<Behaviour>,
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

        Ok(Self { swarm })
    }

    pub async fn bootstrap<B: AsRef<[u8]>>(
        &mut self,
        local: bool,
        of: B,
    ) -> color_eyre::Result<()> {
        if !local {
            let base_addr = Self::BOOTSTRAP_ADDR
                .parse::<Multiaddr>()
                .expect("Valid address");

            for id in Self::BOOTNODES {
                let addr = base_addr
                    .clone()
                    .with_p2p(id.parse().expect("Valid ID"))
                    .expect("Valid address base")
                    .with(Protocol::P2pCircuit);

                self.swarm.listen_on(addr)?;
            }
        }

        tasks::bootstrap(self, of).await
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
