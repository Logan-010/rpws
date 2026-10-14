use crate::api::store;
use libp2p::{
    Multiaddr,
    identity::{DecodingError, Keypair},
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub struct Save {
    identity: Vec<u8>,
    pub secret: [u8; 32],
    pub bootstrap_addrs: Vec<Multiaddr>,
    pub store: Option<store::SerializableMemoryStore>,
}

impl Default for Save {
    fn default() -> Self {
        Self {
            identity: Keypair::generate_ed25519()
                .to_protobuf_encoding()
                .expect("Valid keypair"),
            secret: rand::random(),
            bootstrap_addrs: Vec::new(),
            store: None,
        }
    }
}

impl Save {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn encode(&self) -> postcard::Result<Vec<u8>> {
        postcard::to_stdvec(self)
    }

    pub fn decode<D: AsRef<[u8]>>(data: D) -> postcard::Result<Self> {
        postcard::from_bytes(data.as_ref())
    }

    pub fn identity(&self) -> Result<Keypair, DecodingError> {
        Keypair::from_protobuf_encoding(&self.identity)
    }
}
