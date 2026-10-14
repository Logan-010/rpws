use base64::{Engine, engine::general_purpose::STANDARD};
use color_eyre::eyre::{Context, bail};
use directories::ProjectDirs;
use libp2p::{Multiaddr, identity::Keypair, multiaddr::Protocol};
use serde::{Deserialize, Serialize};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

const BOOTNODES: [&str; 4] = [
    "QmNnooDu7bfjPFoTZYxMNLWUQJyrVwtbZg5gBMjTezGAJN",
    "QmQCU2EcMqAqQPR2i9bChDtGNJchTbq5TbXJJ16u19uLTa",
    "QmbLHAnMoJPWSCR5Zhtx6BHJX9KiKNN6tpvbUcqanj75Nb",
    "QmcZf59bWwK5XFi76CZX8cbJ4BhTzzA3gU1ZjYZcYW3dwt",
];
const BOOTSTRAP_ADDR: &str = "/dnsaddr/bootstrap.libp2p.io";

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    identity: String,
    secret: String,
    api_addr: SocketAddr,
    listen_addrs: Vec<Multiaddr>,
    bootstrap_addrs: Vec<Multiaddr>,
    external_addrs: Vec<Multiaddr>,
    kademlia_store_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let code = Keypair::generate_ed25519()
            .to_protobuf_encoding()
            .expect("Valid key");
        let identity = STANDARD.encode(code);
        let raw: [u8; 32] = rand::random();
        let secret = STANDARD.encode(raw);
        let kademlia_store_path = ProjectDirs::from("com", "Logan-010", "rpws")
            .expect("Valid project dir")
            .data_dir()
            .join("kad.redb");

        let bootstrap_addrs = BOOTNODES
            .iter()
            .map(|id_str| {
                BOOTSTRAP_ADDR
                    .parse::<Multiaddr>()
                    .expect("Valid multiaddr")
                    .with(Protocol::P2p(id_str.parse().expect("Valid peer id")))
            })
            .collect::<Vec<Multiaddr>>();

        let mut listen_addrs = vec![
            "/ip4/0.0.0.0/tcp/7777".parse().expect("Valid multiaddr"),
            "/ip6/::/tcp/7777".parse().expect("Valid multiaddr"),
            "/ip4/0.0.0.0/udp/7777/quic-v1"
                .parse()
                .expect("Valid multiaddr"),
            "/ip6/::/udp/7777/quic-v1".parse().expect("Valid multiaddr"),
            "/ip4/127.0.0.1/tcp/7776/ws"
                .parse()
                .expect("Valid multiaddr"),
            "/ip6/::1/tcp/7776/ws".parse().expect("Valid multiaddr"),
        ];
        listen_addrs.extend(
            bootstrap_addrs
                .clone()
                .into_iter()
                .map(|a| a.with(Protocol::P2pCircuit)),
        );

        Self {
            identity,
            secret,
            api_addr: "[::1]:7778".parse().expect("Valid address"),
            listen_addrs,
            bootstrap_addrs,
            external_addrs: Vec::new(),
            kademlia_store_path,
        }
    }
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn identity(&self) -> color_eyre::Result<Keypair> {
        Ok(Keypair::from_protobuf_encoding(
            &STANDARD.decode(&self.identity)?,
        )?)
    }

    pub fn secret(&self) -> color_eyre::Result<[u8; 32]> {
        let raw = STANDARD.decode(&self.secret)?;
        if raw.len() != 32 {
            bail!("Invalid secret");
        }
        let mut secret = [0u8; 32];
        secret.clone_from_slice(&raw);
        Ok(secret)
    }

    pub fn kademlia_store_path(&self) -> &Path {
        &self.kademlia_store_path
    }

    pub fn api_address(&self) -> SocketAddr {
        self.api_addr
    }

    pub fn listen_addrs(&self) -> &[Multiaddr] {
        &self.listen_addrs
    }

    pub fn external_addrs(&self) -> &[Multiaddr] {
        &self.external_addrs
    }

    pub fn bootstrap_addrs(&self) -> &[Multiaddr] {
        &self.bootstrap_addrs
    }

    pub fn encode(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn decode<S: AsRef<str>>(data: S) -> color_eyre::Result<Self> {
        let decoded: Self = toml::from_str(data.as_ref())?;
        _ = decoded.identity().context("Invalid identity")?;
        Ok(decoded)
    }
}
