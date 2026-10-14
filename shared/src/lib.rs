use blake3::Hash;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, aead::Aead};
use multihash_codetable::{Code, Multihash, MultihashDigest};
use serde::{Deserialize, Serialize};

pub mod sync;

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
pub struct Provider {
    key: Vec<u8>,
    hash: Hash,
    signature: Hash,
    name: Vec<u8>,
}

impl Provider {
    pub fn new<D: AsRef<[u8]>>(name: D, record: &Record, secret: &[u8; 32]) -> Self {
        let key = Self::key(name.as_ref());
        let hash = record.hash;
        let nonce: [u8; Record::NONCE_LENGTH] = rand::random();
        let mut name = XChaCha20Poly1305::new(secret.into())
            .encrypt(&nonce.into(), name.as_ref())
            .expect("Could not encrypt name");
        name.extend_from_slice(&nonce);
        let signature = Self::sign(&key, &hash, &name, secret);
        Self {
            key,
            hash,
            signature,
            name,
        }
    }

    pub fn key<D: AsRef<[u8]>>(name: D) -> Vec<u8> {
        Provide::new(name).encode()
    }

    fn sign(key: &[u8], hash: &Hash, name: &[u8], secret: &[u8; 32]) -> Hash {
        let mut data = key.to_vec();
        data.extend_from_slice(hash.as_bytes());
        data.extend_from_slice(name);
        blake3::keyed_hash(secret, &data)
    }

    pub fn encode(&self) -> Vec<u8> {
        postcard::to_stdvec(self).expect("Could not encode provider")
    }

    pub fn decode<D: AsRef<[u8]>>(data: D) -> postcard::Result<Self> {
        #[derive(Deserialize)]
        struct Mapping {
            key: Vec<u8>,
            hash: Hash,
            signature: Hash,
        }

        let (mapping, remaining) = postcard::take_from_bytes::<Mapping>(data.as_ref())?;
        let name = if remaining.is_empty() {
            Vec::new()
        } else {
            postcard::from_bytes(remaining)?
        };
        Ok(Self {
            key: mapping.key,
            hash: mapping.hash,
            signature: mapping.signature,
            name,
        })
    }

    pub fn verify(&self, key: &[u8], secret: &[u8; 32]) -> bool {
        self.key == key && Self::sign(key, &self.hash, &self.name, secret) == self.signature
    }

    pub fn hash(&self) -> &[u8; 32] {
        self.hash.as_bytes()
    }

    pub fn name(&self, secret: &[u8; 32]) -> Option<String> {
        if !self.verify(&self.key, secret) || self.name.len() < Record::NONCE_LENGTH {
            return None;
        }
        let (encrypted, nonce) = self.name.split_at(self.name.len() - Record::NONCE_LENGTH);
        let mut nonce_bytes = [0; Record::NONCE_LENGTH];
        nonce_bytes.copy_from_slice(nonce);
        let name = XChaCha20Poly1305::new(secret.into())
            .decrypt(&nonce_bytes.into(), encrypted)
            .ok()?;
        let name = String::from_utf8(name).ok()?;
        (Self::key(&name) == self.key).then_some(name)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Record {
    payload: Vec<u8>,
    hash: Hash,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordPayload {
    pub url: String,
    pub username: String,
    pub password: String,
    pub timestamp: u64,
    pub extra: String,
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

    pub fn hash(&self) -> &[u8; 32] {
        self.hash.as_bytes()
    }

    pub fn payload(&self, secret: &[u8; 32]) -> Option<RecordPayload> {
        let mut nonce = [0u8; Self::NONCE_LENGTH];
        if self.payload.len() < Self::NONCE_LENGTH {
            return None;
        }
        nonce.clone_from_slice(&self.payload[self.payload.len() - Self::NONCE_LENGTH..]);
        let decrypted = XChaCha20Poly1305::new(secret.into())
            .decrypt(
                &nonce.into(),
                &self.payload[..self.payload.len() - Self::NONCE_LENGTH],
            )
            .ok()?;

        postcard::from_bytes(&decrypted).ok()
    }
}

pub fn derive_key<D: AsRef<[u8]>>(secret: D) -> [u8; 32] {
    blake3::derive_key("RPWS Blake3 Key Creation", secret.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload() -> RecordPayload {
        RecordPayload {
            url: "https://example.com".into(),
            username: "alice".into(),
            password: "secret".into(),
            timestamp: 1,
            extra: String::new(),
        }
    }

    #[test]
    fn encrypted_record_roundtrip_rejects_tampering_and_wrong_secret() {
        let secret = derive_key(b"test-secret");
        let mut record = Record::decode(Record::new(payload(), &secret).encode()).unwrap();
        assert!(record.verify(&secret));
        assert_eq!(record.payload(&secret), Some(payload()));
        assert!(!record.verify(&derive_key(b"wrong-secret")));
        assert!(record.payload(&derive_key(b"wrong-secret")).is_none());
        record.payload[0] ^= 1;
        assert!(!record.verify(&secret));
        assert!(record.payload(&secret).is_none());
        record.payload.truncate(Record::NONCE_LENGTH - 1);
        assert!(record.payload(&secret).is_none());
    }

    #[test]
    fn provider_binds_name_and_record_hash_to_secret() {
        let secret = derive_key(b"test-secret");
        let record = Record::new(payload(), &secret);
        let mut provider =
            Provider::decode(Provider::new("example", &record, &secret).encode()).unwrap();
        assert!(provider.verify(&Provider::key("example"), &secret));
        assert_eq!(provider.hash(), record.hash());
        assert_eq!(provider.name(&secret).as_deref(), Some("example"));
        assert!(provider.name(&derive_key(b"wrong-secret")).is_none());
        assert!(!provider.verify(&Provider::key("other"), &secret));
        assert!(!provider.verify(&Provider::key("example"), &derive_key(b"wrong-secret")));
        provider.name[0] ^= 1;
        assert!(!provider.verify(&Provider::key("example"), &secret));
        assert!(provider.name(&secret).is_none());
        provider.name[0] ^= 1;
        provider.hash = blake3::hash(b"other-record");
        assert!(!provider.verify(&Provider::key("example"), &secret));
    }

    #[test]
    fn legacy_provider_mappings_remain_readable() {
        let secret = derive_key(b"test-secret");
        let record = Record::new(payload(), &secret);
        let key = Provider::key("example");
        let signature = Provider::sign(&key, &record.hash, &[], &secret);
        let encoded = postcard::to_stdvec(&(&key, &record.hash, &signature)).unwrap();
        let provider = Provider::decode(encoded).unwrap();
        assert!(provider.verify(&key, &secret));
        assert_eq!(provider.hash(), record.hash());
        assert!(provider.name(&secret).is_none());
    }
}
