use blake3::Hash;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Aead};
use multihash_codetable::{Code, Multihash, MultihashDigest};
use serde::{Deserialize, Serialize};

pub struct Provide<D: AsRef<[u8]>>(D);

impl<D: AsRef<[u8]>> Provide<D> {
    pub fn new(data: D) -> Self {
        Self(data)
    }

    pub fn encode(&self) -> Vec<u8> {
        Code::Blake3_256.digest(self.0.as_ref()).to_bytes()
    }

    pub fn is_valid(d: D) -> bool {
        Multihash::from_bytes(d.as_ref()).is_ok()
    }
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    payload: Vec<u8>,
    hash: Hash,
}

#[derive(Serialize, Deserialize)]
pub struct RecordPayload {
    url: String,
    username: String,
    password: String,
    timestamp: u64,
    extra: String,
}

impl Record {
    const NONCE_LENGTH: usize = 24;

    pub fn new(payload: RecordPayload, secret: &[u8; 32]) -> Self {
        let encoded_payload = postcard::to_stdvec(&payload).expect("Could not encode payload");
        let nonce: [u8; Self::NONCE_LENGTH] = rand::random();
        let mut encrypted = XChaCha20Poly1305::new(secret.into())
            .encrypt(&nonce.into(), encoded_payload.as_slice())
            .expect("Could not encrypt payload");
        encrypted.extend_from_slice(&nonce);

        let hash = blake3::keyed_hash(secret, &encrypted);

        Self {
            payload: encrypted,
            hash,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("Could not encode record")
    }

    pub fn decode<D: AsRef<[u8]>>(data: D) -> postcard::Result<Self> {
        postcard::from_bytes(data.as_ref())
    }

    pub fn verify(&self, secret: &[u8; 32]) -> bool {
        blake3::keyed_hash(secret, &self.payload) == self.hash
    }

    pub fn payload(&self, secret: &[u8; 32]) -> Option<RecordPayload> {
        let mut nonce = [0u8; Self::NONCE_LENGTH];
        if self.payload.len() < Self::NONCE_LENGTH {
            return None;
        }
        nonce.clone_from_slice(&self.payload[self.payload.len() - Self::NONCE_LENGTH..]);
        let decrypted = XChaCha20Poly1305::new(secret.into())
            .decrypt(&nonce.into(), self.payload.as_slice())
            .ok()?;

        postcard::from_bytes(&decrypted).ok()
    }
}

pub fn derive_key<D: AsRef<[u8]>>(secret: D) -> [u8; 32] {
    blake3::derive_key("RPWS Blake3 Key Creation", secret.as_ref())
}
