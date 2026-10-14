//! Encrypted vault enumeration shared by native and browser peers.
use crate::{Provider, Record, RecordPayload};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL: &str = "/rpws/sync/1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub vault: [u8; 32],
}

impl Request {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            vault: *blake3::keyed_hash(key, b"rpws encrypted vault sync v1").as_bytes(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub mapping: Vec<u8>,
    pub record: Vec<u8>,
}

impl Entry {
    pub fn decode(&self, key: &[u8; 32]) -> Option<(String, RecordPayload)> {
        let mapping = Provider::decode(&self.mapping).ok()?;
        let name = mapping.name(key)?;
        let record = Record::decode(&self.record).ok()?;
        if mapping.hash() != record.hash() || !record.verify(key) {
            return None;
        }
        Some((name, record.payload(key)?))
    }

    pub fn new(name: &str, payload: RecordPayload, key: &[u8; 32]) -> Self {
        let record = Record::new(payload, key);
        Self {
            mapping: Provider::new(name, &record, key).encode(),
            record: record.encode(),
        }
    }

    pub fn records(&self, key: &[u8; 32]) -> Option<[(Vec<u8>, Vec<u8>); 2]> {
        let (name, _) = self.decode(key)?;
        let mapping = Provider::decode(&self.mapping).ok()?;
        Some([
            (mapping.hash().to_vec(), self.record.clone()),
            (Provider::key(name), self.mapping.clone()),
        ])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub matched: bool,
    pub entries: Vec<Entry>,
}

/// Only complete, authenticated mappings are exported; orphan payloads and
/// records belonging to another vault never become visible through sync.
pub fn entries(
    records: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
    key: &[u8; 32],
) -> Vec<Entry> {
    let records: HashMap<_, _> = records.into_iter().collect();
    records
        .iter()
        .filter_map(|(stored_key, bytes)| {
            let mapping = Provider::decode(bytes).ok()?;
            if !mapping.verify(stored_key, key) {
                return None;
            }
            let entry = Entry {
                mapping: bytes.clone(),
                record: records.get(mapping.hash().as_slice())?.clone(),
            };
            entry.decode(key)?;
            Some(entry)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enumeration_authenticates_mapping_and_payload_without_plaintext_on_wire() {
        let key = crate::derive_key([7; 32]);
        let entry = Entry::new(
            "private name",
            RecordPayload {
                url: "url".into(),
                username: "alice".into(),
                password: "private password".into(),
                timestamp: 5,
                extra: String::new(),
            },
            &key,
        );
        let records = entry.records(&key).unwrap();
        assert_eq!(entries(records.clone(), &key).len(), 1);
        assert!(entries(records, &[0; 32]).is_empty());
        assert!(entry.decode(&[0; 32]).is_none());
        let encoded = postcard::to_stdvec(&entry).unwrap();
        assert!(
            !encoded
                .windows(16)
                .any(|bytes| bytes == b"private password")
        );
        let mut damaged = entry;
        damaged.record[0] ^= 1;
        assert!(damaged.decode(&key).is_none());
    }
}
