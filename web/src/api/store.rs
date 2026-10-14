use libp2p::{
    Multiaddr, PeerId,
    kad::{
        ProviderRecord, Record, RecordKey as Key,
        store::{Error, RecordStore, Result as StoreResult},
    },
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as DeError};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, hash_map},
    str::FromStr,
};
use web_time::{Duration, Instant};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStoreConfig {
    pub max_records: usize,
    pub max_value_bytes: usize,
    pub max_providers_per_key: usize,
    pub max_provided_keys: usize,
}

impl Default for MemoryStoreConfig {
    fn default() -> Self {
        Self {
            max_records: 1024,
            max_value_bytes: 65 * 1024,
            max_providers_per_key: 20,
            max_provided_keys: 1024,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredRecord {
    key: Vec<u8>,
    value: Vec<u8>,
    publisher: Option<String>,
    expires_in_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredProviderRecord {
    key: Vec<u8>,
    provider: String,
    addresses: Vec<String>,
    expires_in_ms: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreSnapshot {
    local_id: String,
    config: MemoryStoreConfig,
    records: Vec<StoredRecord>,
    providers: Vec<StoredProviderRecord>,
}

fn serialize_expiration(expires: Option<Instant>, now: Instant) -> Option<u64> {
    expires.map(|deadline| {
        let remaining = deadline.saturating_duration_since(now);

        u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX)
    })
}

fn deserialize_expiration(remaining_ms: Option<u64>, now: Instant) -> Option<Instant> {
    remaining_ms.map(|ms| now.checked_add(Duration::from_millis(ms)).unwrap_or(now))
}

impl StoredRecord {
    fn from_record(record: &Record, now: Instant) -> Self {
        Self {
            key: record.key.to_vec(),
            value: record.value.clone(),
            publisher: record.publisher.map(|p| p.to_string()),
            expires_in_ms: serialize_expiration(record.expires, now),
        }
    }

    fn into_record(self, now: Instant) -> Result<Record, String> {
        let publisher = self
            .publisher
            .map(|p| PeerId::from_str(&p).map_err(|e| e.to_string()))
            .transpose()?;

        Ok(Record {
            key: Key::from(self.key),
            value: self.value,
            publisher,
            expires: deserialize_expiration(self.expires_in_ms, now),
        })
    }
}

impl StoredProviderRecord {
    fn from_record(record: &ProviderRecord, now: Instant) -> Self {
        Self {
            key: record.key.to_vec(),
            provider: record.provider.to_string(),
            addresses: record.addresses.iter().map(ToString::to_string).collect(),
            expires_in_ms: serialize_expiration(record.expires, now),
        }
    }

    fn into_record(self, now: Instant) -> Result<ProviderRecord, String> {
        let provider = PeerId::from_str(&self.provider).map_err(|e| e.to_string())?;

        let addresses = self
            .addresses
            .into_iter()
            .map(|a| Multiaddr::from_str(&a).map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ProviderRecord {
            key: Key::from(self.key),
            provider,
            addresses,
            expires: deserialize_expiration(self.expires_in_ms, now),
        })
    }
}

#[derive(Debug, Clone)]
pub struct SerializableMemoryStore {
    local_id: PeerId,
    config: MemoryStoreConfig,
    records: HashMap<Key, Record>,
    providers: HashMap<Key, Vec<ProviderRecord>>,
    provided: HashSet<ProviderRecord>,
}

impl SerializableMemoryStore {
    pub fn new(local_id: PeerId) -> Self {
        Self::with_config(local_id, MemoryStoreConfig::default())
    }

    pub fn with_config(local_id: PeerId, config: MemoryStoreConfig) -> Self {
        Self {
            local_id,
            config,
            records: HashMap::new(),
            providers: HashMap::new(),
            provided: HashSet::new(),
        }
    }

    pub fn prune_expired(&mut self) {
        let now = Instant::now();

        self.records.retain(|_, record| !record.is_expired(now));

        self.providers.retain(|_, records| {
            records.retain(|record| !record.is_expired(now));
            !records.is_empty()
        });

        self.rebuild_provided();
    }

    fn rebuild_provided(&mut self) {
        self.provided.clear();

        for records in self.providers.values() {
            for record in records {
                if record.provider == self.local_id {
                    self.provided.insert(record.clone());
                }
            }
        }
    }

    fn snapshot(&self) -> StoreSnapshot {
        let now = Instant::now();

        StoreSnapshot {
            local_id: self.local_id.to_string(),
            config: self.config.clone(),
            records: self
                .records
                .values()
                .map(|r| StoredRecord::from_record(r, now))
                .collect(),
            providers: self
                .providers
                .values()
                .flat_map(|records| records.iter())
                .map(|r| StoredProviderRecord::from_record(r, now))
                .collect(),
        }
    }

    fn from_snapshot(snapshot: StoreSnapshot) -> Result<Self, String> {
        let local_id = PeerId::from_str(&snapshot.local_id).map_err(|e| e.to_string())?;

        let mut store = Self::with_config(local_id, snapshot.config);
        let now = Instant::now();

        for stored in snapshot.records {
            let record = stored.into_record(now)?;

            if store.records.contains_key(&record.key) {
                return Err("duplicate record key".into());
            }

            store.put(record).map_err(|e| e.to_string())?;
        }

        for stored in snapshot.providers {
            let record = stored.into_record(now)?;

            if let Some(existing) = store.providers.get(&record.key) {
                if existing.iter().any(|p| p.provider == record.provider) {
                    return Err("duplicate provider".into());
                }

                if existing.len() >= store.config.max_providers_per_key {
                    return Err("too many providers for key".into());
                }
            } else if store.config.max_providers_per_key == 0 {
                return Err("provider capacity is zero".into());
            }

            store.add_provider(record).map_err(|e| e.to_string())?;
        }

        Ok(store)
    }

    fn records_len_at_capacity(&self) -> bool {
        self.records.len() >= self.config.max_records
    }
}

impl Serialize for SerializableMemoryStore {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.snapshot().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SerializableMemoryStore {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = StoreSnapshot::deserialize(deserializer)?;

        Self::from_snapshot(snapshot).map_err(D::Error::custom)
    }
}

impl RecordStore for SerializableMemoryStore {
    type RecordsIter<'a> = Box<dyn Iterator<Item = Cow<'a, Record>> + 'a>;

    type ProvidedIter<'a> = Box<dyn Iterator<Item = Cow<'a, ProviderRecord>> + 'a>;

    fn get(&self, key: &Key) -> Option<Cow<'_, Record>> {
        self.records.get(key).map(Cow::Borrowed)
    }

    fn put(&mut self, record: Record) -> StoreResult<()> {
        if record.value.len() >= self.config.max_value_bytes {
            return Err(Error::ValueTooLarge);
        }

        let at_capacity = self.records.len() >= self.config.max_records;

        match self.records.entry(record.key.clone()) {
            hash_map::Entry::Occupied(mut entry) => {
                entry.insert(record);
            }

            hash_map::Entry::Vacant(entry) => {
                if at_capacity {
                    return Err(Error::MaxRecords);
                }

                entry.insert(record);
            }
        }

        Ok(())
    }

    fn remove(&mut self, key: &Key) {
        self.records.remove(key);
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        Box::new(self.records.values().map(Cow::Borrowed))
    }

    fn add_provider(&mut self, record: ProviderRecord) -> StoreResult<()> {
        let key = record.key.clone();
        let is_local = record.provider == self.local_id;
        if let Some(providers) = self.providers.get_mut(&key) {
            if let Some(existing) = providers.iter_mut().find(|p| p.provider == record.provider) {
                if is_local {
                    self.provided.replace(record.clone());
                }

                *existing = record;
                return Ok(());
            }

            if providers.len() >= self.config.max_providers_per_key {
                return Ok(());
            }

            if is_local {
                self.provided.insert(record.clone());
            }

            providers.push(record);
            return Ok(());
        }

        if self.providers.len() >= self.config.max_provided_keys {
            return Err(Error::MaxProvidedKeys);
        }

        if self.config.max_providers_per_key == 0 {
            return Ok(());
        }

        if is_local {
            self.provided.insert(record.clone());
        }

        self.providers.insert(key, vec![record]);

        Ok(())
    }

    fn providers(&self, key: &Key) -> Vec<ProviderRecord> {
        self.providers.get(key).cloned().unwrap_or_default()
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        Box::new(self.provided.iter().map(Cow::Borrowed))
    }

    fn remove_provider(&mut self, key: &Key, provider: &PeerId) {
        let mut remove_key = false;

        if let Some(providers) = self.providers.get_mut(key) {
            if let Some(index) = providers.iter().position(|p| &p.provider == provider) {
                let removed = providers.remove(index);

                if removed.provider == self.local_id {
                    self.provided.remove(&removed);
                }
            }

            remove_key = providers.is_empty();
        }

        if remove_key {
            self.providers.remove(key);
        }
    }
}
