//! Encrypted vault enumeration shared by native and browser peers.
use crate::{Provider, Record, RecordPayload, RecordState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL: &str = "/rpws/sync/2.0.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub vault: [u8; 32],
}

impl Request {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            vault: *blake3::keyed_hash(key, b"rpws encrypted vault sync v2").as_bytes(),
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
        let (name, state) = self.decode_state(key)?;
        match state {
            RecordState::Live(payload) => Some((name, payload)),
            RecordState::Deleted { .. } => None,
        }
    }

    pub fn decode_state(&self, key: &[u8; 32]) -> Option<(String, RecordState)> {
        let mapping = Provider::decode(&self.mapping).ok()?;
        let name = mapping.name(key)?;
        let record = Record::decode(&self.record).ok()?;
        if mapping.hash() != record.hash() || !record.verify(key) {
            return None;
        }
        Some((name, record.state(key)?))
    }

    pub fn new(name: &str, payload: RecordPayload, key: &[u8; 32]) -> Self {
        Self::from_state(name, RecordState::Live(payload), key)
    }

    pub fn from_state(name: &str, state: RecordState, key: &[u8; 32]) -> Self {
        let record = Record::from_state(state, key);
        Self {
            mapping: Provider::new(name, &record, key).encode(),
            record: record.encode(),
        }
    }

    pub fn records(&self, key: &[u8; 32]) -> Option<[(Vec<u8>, Vec<u8>); 2]> {
        let (name, _) = self.decode_state(key)?;
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
            entry.decode_state(key)?;
            Some(entry)
        })
        .collect()
}

pub fn mapping_state(
    mapping: &[u8],
    key: &[u8; 32],
    mut lookup: impl FnMut(&[u8]) -> Option<Vec<u8>>,
) -> Option<RecordState> {
    let mapping = Provider::decode(mapping).ok()?;
    mapping.name(key)?;
    let record = Record::decode(lookup(mapping.hash())?).ok()?;
    (mapping.hash() == record.hash() && record.verify(key)).then_some(())?;
    record.state(key)
}

/// DHT mappings may arrive before their payload. Never let such a mapping
/// overwrite a known deletion; a later complete sync can accept a newer edit.
pub fn accepts_mapping(
    current: Option<&[u8]>,
    incoming: &[u8],
    key: &[u8; 32],
    mut lookup: impl FnMut(&[u8]) -> Option<Vec<u8>>,
) -> bool {
    let Some(current) = current.and_then(|value| mapping_state(value, key, &mut lookup)) else {
        return true;
    };
    match mapping_state(incoming, key, lookup) {
        Some(incoming) => incoming.version() >= current.version(),
        None => !current.version().1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_is_enumerated_and_blocks_stale_or_incomplete_mappings() {
        let key = crate::derive_key([7; 32]);
        let deleted = Entry::from_state("account", RecordState::Deleted { timestamp: 10 }, &key);
        let live = Entry::new(
            "account",
            RecordPayload {
                url: String::new(),
                username: "alice".into(),
                password: "secret".into(),
                timestamp: 10,
                extra: String::new(),
            },
            &key,
        );
        let mut records: HashMap<_, _> = deleted.records(&key).unwrap().into_iter().collect();
        assert_eq!(entries(records.clone(), &key).len(), 1);
        assert!(deleted.decode(&key).is_none());
        assert_eq!(
            deleted.decode_state(&key).unwrap().1,
            RecordState::Deleted { timestamp: 10 }
        );
        assert!(deleted.decode_state(&[0; 32]).is_none());
        assert!(!accepts_mapping(
            Some(&deleted.mapping),
            &live.mapping,
            &key,
            |hash| records.get(hash).cloned()
        ));
        records.extend(live.records(&key).unwrap());
        assert!(!accepts_mapping(
            Some(&deleted.mapping),
            &live.mapping,
            &key,
            |hash| records.get(hash).cloned()
        ));
        let newer = Entry::new(
            "account",
            RecordPayload {
                timestamp: 11,
                ..live.decode(&key).unwrap().1
            },
            &key,
        );
        records.extend(newer.records(&key).unwrap());
        assert!(accepts_mapping(
            Some(&deleted.mapping),
            &newer.mapping,
            &key,
            |hash| records.get(hash).cloned()
        ));
    }
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
