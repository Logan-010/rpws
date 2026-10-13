use base64::{Engine, engine::general_purpose::STANDARD};
use color_eyre::eyre::Context;
use directories::ProjectDirs;
use libp2p::{Multiaddr, identity::Keypair};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    identity: String,
    secret: String,
    listen_addrs: Vec<Multiaddr>,
    external_addrs: Vec<Multiaddr>,
    kademlia_store_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        let code = Keypair::generate_ed25519()
            .to_protobuf_encoding()
            .expect("Valid key");
        let identity = STANDARD.encode(code);
        let kademlia_store_path = ProjectDirs::from("com", "Logan-010", "rpws")
            .expect("Valid project dir")
            .data_dir()
            .join("kad.redb");

        Self {
            identity,
            secret: String::from("Change me!"),
            listen_addrs: vec![
                "/ip4/0.0.0.0/tcp/7777".parse().expect("Valid multiaddr"),
                "/ip6/::/tcp/7777".parse().expect("Valid multiaddr"),
                "/ip4/0.0.0.0/udp/7777/quic-v1"
                    .parse()
                    .expect("Valid multiaddr"),
                "/ip6/::/udp/7777/quic-v1".parse().expect("Valid multiaddr"),
            ],
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

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn kademlia_store_path(&self) -> &Path {
        &self.kademlia_store_path
    }

    pub fn listen_addrs(&self) -> &[Multiaddr] {
        &self.listen_addrs
    }

    pub fn external_addrs(&self) -> &[Multiaddr] {
        &self.external_addrs
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
