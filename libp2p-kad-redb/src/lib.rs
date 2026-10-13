use libp2p::Multiaddr;
use libp2p::PeerId;
use libp2p::kad::{self, ProviderRecord, Record, RecordKey, store::RecordStore};
use redb::ReadableDatabase;
use redb::{
    Database, MultimapTableDefinition, ReadableMultimapTable, ReadableTable, TableDefinition,
    TableError,
};
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, time::Instant, vec::IntoIter};

/// A storage failure with operation context and the underlying error.
#[derive(Debug)]
pub struct StoreError {
    operation: &'static str,
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl StoreError {
    fn new(
        operation: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self {
            operation,
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.operation, self.source)
    }
}

impl std::error::Error for StoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

fn decode_expiration(expires: Option<u64>) -> Result<Option<Instant>, StoreError> {
    expires
        .map(|millis| {
            Instant::now()
                .checked_add(std::time::Duration::from_millis(millis))
                .ok_or_else(|| {
                    StoreError::new(
                        "failed to decode expiration",
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "expiration exceeds the supported instant range",
                        ),
                    )
                })
        })
        .transpose()
}

#[derive(Serialize, Deserialize)]
struct RecordSerde {
    key: RecordKey,
    value: Vec<u8>,
    publisher: Option<PeerId>,
    expires: Option<u64>,
}

#[derive(Serialize, Deserialize)]
struct ProviderRecordSerde {
    key: RecordKey,
    provider: PeerId,
    expires: Option<u64>,
    addresses: Vec<Multiaddr>,
}

const RECORDS: TableDefinition<'static, &[u8], &[u8]> = TableDefinition::new("KAD-REDB.RECORDS");
const PROVIDER_RECORDS: MultimapTableDefinition<'static, &[u8], &[u8]> =
    MultimapTableDefinition::new("KAD-REDB.PROVIDER-RECORDS");

/// Redb-based store for Kad storage.
///
/// The `try_*` methods preserve storage errors. The `RecordStore` implementation
/// logs failures and returns absent/empty results for reads. Since that trait has
/// no storage-error variant, failed writes map to `MaxRecords` or
/// `MaxProvidedKeys`; these do not necessarily indicate a capacity limit.
pub struct RedbStore {
    peer: PeerId,
    db: Database,
}

impl RedbStore {
    /// Create a new store object with your own peer id and an open database connection
    pub fn new(peer: PeerId, databse: Database) -> Self {
        Self { peer, db: databse }
    }
}

impl RedbStore {
    /// Store a record, preserving database and serialization errors.
    pub fn try_put(&mut self, r: Record) -> Result<(), StoreError> {
        let write = self
            .db
            .begin_write()
            .map_err(|e| StoreError::new("failed to begin write transaction", e))?;

        {
            let mut records = write
                .open_table(RECORDS)
                .map_err(|e| StoreError::new("failed to open records table", e))?;

            let s = postcard::to_stdvec(&RecordSerde {
                key: r.key.clone(),
                value: r.value,
                publisher: r.publisher,
                expires: r
                    .expires
                    .map(|i| i.saturating_duration_since(Instant::now()).as_millis() as u64),
            })
            .map_err(|e| StoreError::new("failed to serialize record", e))?;

            records
                .insert(r.key.to_vec().as_slice(), s.as_slice())
                .map_err(|e| StoreError::new("failed to insert record", e))?;
        }

        write
            .commit()
            .map_err(|e| StoreError::new("failed to commit record", e))?;

        Ok(())
    }

    /// Retrieve a record, distinguishing absence from storage failures.
    pub fn try_get(&self, k: &RecordKey) -> Result<Option<Record>, StoreError> {
        let read = self
            .db
            .begin_read()
            .map_err(|e| StoreError::new("failed to begin read transaction", e))?;

        let records = match read.open_table(RECORDS) {
            Ok(table) => table,
            Err(TableError::TableDoesNotExist(_)) => return Ok(None),
            Err(e) => return Err(StoreError::new("failed to open records table", e)),
        };

        records
            .get(k.to_vec().as_slice())
            .map_err(|e| StoreError::new("failed to query records table", e))?
            .map(|a| {
                let d: RecordSerde = postcard::from_bytes(a.value())
                    .map_err(|e| StoreError::new("failed to decode record", e))?;

                Ok(Record {
                    key: d.key,
                    value: d.value,
                    publisher: d.publisher,
                    expires: decode_expiration(d.expires)?,
                })
            })
            .transpose()
    }

    /// Remove a record, reporting transaction failures.
    pub fn try_remove(&mut self, k: &RecordKey) -> Result<(), StoreError> {
        let write = self
            .db
            .begin_write()
            .map_err(|e| StoreError::new("failed to begin write transaction", e))?;

        {
            let mut records = write
                .open_table(RECORDS)
                .map_err(|e| StoreError::new("failed to open records table", e))?;

            records
                .remove(k.to_vec().as_slice())
                .map_err(|e| StoreError::new("failed to remove record", e))?;
        }

        write
            .commit()
            .map_err(|e| StoreError::new("failed to commit record removal", e))
    }

    /// Store a provider record, preserving database and serialization errors.
    pub fn try_add_provider(&mut self, record: ProviderRecord) -> Result<(), StoreError> {
        let write = self
            .db
            .begin_write()
            .map_err(|e| StoreError::new("failed to begin write transaction", e))?;

        {
            let mut records = write
                .open_multimap_table(PROVIDER_RECORDS)
                .map_err(|e| StoreError::new("failed to open provider records table", e))?;

            let s = postcard::to_stdvec(&ProviderRecordSerde {
                key: record.key.clone(),
                provider: record.provider,
                expires: record
                    .expires
                    .map(|i| i.saturating_duration_since(Instant::now()).as_millis() as u64),
                addresses: record.addresses,
            })
            .map_err(|e| StoreError::new("failed to serialize provider record", e))?;

            records
                .insert(record.key.to_vec().as_slice(), s.as_slice())
                .map_err(|e| StoreError::new("failed to insert provider record", e))?;
        }

        write
            .commit()
            .map_err(|e| StoreError::new("failed to commit provider record", e))?;

        Ok(())
    }

    /// Remove a provider record, aborting on read or decoding failures.
    pub fn try_remove_provider(&mut self, k: &RecordKey, p: &PeerId) -> Result<(), StoreError> {
        let write = self
            .db
            .begin_write()
            .map_err(|e| StoreError::new("failed to begin write transaction", e))?;

        {
            let mut records = write
                .open_multimap_table(PROVIDER_RECORDS)
                .map_err(|e| StoreError::new("failed to open provider records table", e))?;

            let mut to_remove = Vec::new();
            {
                let values = records
                    .get(k.as_ref())
                    .map_err(|e| StoreError::new("failed to query provider records table", e))?;
                for entry in values {
                    let entry =
                        entry.map_err(|e| StoreError::new("failed to read provider record", e))?;
                    let bytes = entry.value().to_vec();
                    let decoded: ProviderRecordSerde = postcard::from_bytes(&bytes)
                        .map_err(|e| StoreError::new("failed to decode provider record", e))?;
                    if decoded.provider == *p {
                        to_remove.push(bytes);
                    }
                }
            }

            for value in to_remove {
                records
                    .remove(k.as_ref(), value.as_slice())
                    .map_err(|e| StoreError::new("failed to remove provider record", e))?;
            }
        }

        write
            .commit()
            .map_err(|e| StoreError::new("failed to commit provider removal", e))
    }

    /// Retrieve locally provided records, reporting read and decoding failures.
    pub fn try_provided(&self) -> Result<Vec<ProviderRecord>, StoreError> {
        let mut records = Vec::new();

        let read = self
            .db
            .begin_read()
            .map_err(|e| StoreError::new("failed to begin read transaction", e))?;

        match read.open_multimap_table(PROVIDER_RECORDS) {
            Ok(table) => {
                for e in table
                    .iter()
                    .map_err(|e| StoreError::new("failed to iterate provider records table", e))?
                {
                    let (_, v) =
                        e.map_err(|e| StoreError::new("failed to read provider records entry", e))?;
                    for p in v.into_iter() {
                        let record_bytes =
                            p.map_err(|e| StoreError::new("failed to read provider record", e))?;

                        let r: ProviderRecordSerde = postcard::from_bytes(record_bytes.value())
                            .map_err(|e| StoreError::new("failed to decode provider record", e))?;
                        let pr = ProviderRecord {
                            key: r.key,
                            provider: r.provider,
                            expires: decode_expiration(r.expires)?,
                            addresses: r.addresses,
                        };

                        if pr.provider == self.peer {
                            records.push(pr);
                        }
                    }
                }
            }
            Err(TableError::TableDoesNotExist(_)) => {
                // No actual error
            }
            Err(e) => return Err(StoreError::new("failed to open provider records table", e)),
        }

        Ok(records)
    }

    /// Retrieve providers for a key, reporting read and decoding failures.
    pub fn try_providers(&self, key: &RecordKey) -> Result<Vec<ProviderRecord>, StoreError> {
        let mut records = Vec::new();

        let read = self
            .db
            .begin_read()
            .map_err(|e| StoreError::new("failed to begin read transaction", e))?;

        match read.open_multimap_table(PROVIDER_RECORDS) {
            Ok(table) => {
                let values = table
                    .get(key.to_vec().as_slice())
                    .map_err(|e| StoreError::new("failed to query provider records table", e))?;

                for r in values.into_iter() {
                    let record_bytes =
                        r.map_err(|e| StoreError::new("failed to read provider record", e))?;

                    let r: ProviderRecordSerde = postcard::from_bytes(record_bytes.value())
                        .map_err(|e| StoreError::new("failed to decode provider record", e))?;
                    let pr = ProviderRecord {
                        key: r.key,
                        provider: r.provider,
                        expires: decode_expiration(r.expires)?,
                        addresses: r.addresses,
                    };

                    records.push(pr);
                }
            }
            Err(TableError::TableDoesNotExist(_)) => {
                // No actual error
            }
            Err(e) => return Err(StoreError::new("failed to open provider records table", e)),
        }

        Ok(records)
    }

    /// Retrieve all records, reporting read and decoding failures.
    pub fn try_records(&self) -> Result<Vec<Record>, StoreError> {
        let mut records = Vec::new();

        let read = self
            .db
            .begin_read()
            .map_err(|e| StoreError::new("failed to begin read transaction", e))?;

        match read.open_table(RECORDS) {
            Ok(table) => {
                for e in table
                    .iter()
                    .map_err(|e| StoreError::new("failed to iterate records table", e))?
                {
                    let (_, v) =
                        e.map_err(|e| StoreError::new("failed to read records entry", e))?;

                    let d: RecordSerde = postcard::from_bytes(v.value())
                        .map_err(|e| StoreError::new("failed to decode record", e))?;
                    let r = Record {
                        key: d.key,
                        value: d.value,
                        publisher: d.publisher,
                        expires: decode_expiration(d.expires)?,
                    };

                    records.push(r);
                }
            }
            Err(TableError::TableDoesNotExist(_)) => {
                // No actual error
            }
            Err(e) => return Err(StoreError::new("failed to open records table", e)),
        }

        Ok(records)
    }
}

// RecordStore cannot represent storage errors. Log the cause and use its
// supported failure values; callers needing the cause should use try_* methods.
impl RecordStore for RedbStore {
    type ProvidedIter<'a>
        = IntoIter<Cow<'a, ProviderRecord>>
    where
        Self: 'a;
    type RecordsIter<'a>
        = IntoIter<Cow<'a, Record>>
    where
        Self: 'a;

    fn put(&mut self, r: Record) -> kad::store::Result<()> {
        self.try_put(r).map_err(|error| {
            tracing::error!(%error, "Kademlia record write failed");
            kad::store::Error::MaxRecords
        })
    }

    fn get(&self, k: &RecordKey) -> Option<Cow<'_, Record>> {
        self.try_get(k)
            .unwrap_or_else(|error| {
                tracing::error!(%error, "Kademlia record read failed");
                None
            })
            .map(Cow::Owned)
    }

    fn remove(&mut self, k: &RecordKey) {
        if let Err(error) = self.try_remove(k) {
            tracing::error!(%error, "Kademlia record removal failed");
        }
    }

    fn add_provider(&mut self, record: ProviderRecord) -> kad::store::Result<()> {
        self.try_add_provider(record).map_err(|error| {
            tracing::error!(%error, "Kademlia provider write failed");
            kad::store::Error::MaxProvidedKeys
        })
    }

    fn remove_provider(&mut self, k: &RecordKey, p: &PeerId) {
        if let Err(error) = self.try_remove_provider(k, p) {
            tracing::error!(%error, "Kademlia provider removal failed");
        }
    }

    fn provided(&self) -> Self::ProvidedIter<'_> {
        self.try_provided()
            .unwrap_or_else(|error| {
                tracing::error!(%error, "Kademlia local providers read failed");
                Vec::new()
            })
            .into_iter()
            .map(Cow::Owned)
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn providers(&self, key: &RecordKey) -> Vec<ProviderRecord> {
        self.try_providers(key).unwrap_or_else(|error| {
            tracing::error!(%error, "Kademlia providers read failed");
            Vec::new()
        })
    }

    fn records(&self) -> Self::RecordsIter<'_> {
        self.try_records()
            .unwrap_or_else(|error| {
                tracing::error!(%error, "Kademlia records read failed");
                Vec::new()
            })
            .into_iter()
            .map(Cow::Owned)
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod test {
    use super::{PROVIDER_RECORDS, ProviderRecordSerde, RECORDS, RedbStore};
    use libp2p::PeerId;
    use libp2p::kad::{Record, RecordKey, store::RecordStore};
    use redb::{Database, ReadableDatabase};
    use tempfile::NamedTempFile;

    fn create_store() -> RedbStore {
        let file = NamedTempFile::new().expect("failed to create temporary file");
        let db = Database::create(file.path()).expect("failed to create database object");
        RedbStore::new(PeerId::random(), db)
    }

    #[test]
    fn test_insert_and_retrieval() {
        let mut store = create_store();

        store
            .put(Record::new(b"hello".to_vec(), b"world".to_vec()))
            .expect("failed to put value");

        let record = store
            .get(&RecordKey::new(b"hello"))
            .expect("failed to get value")
            .value
            .clone();

        assert_eq!(record, b"world");
    }

    #[test]
    fn missing_tables_are_not_read_errors() {
        let store = create_store();
        let key = RecordKey::new(b"missing");

        assert!(store.try_get(&key).unwrap().is_none());
        assert!(store.try_records().unwrap().is_empty());
        assert!(store.try_providers(&key).unwrap().is_empty());
        assert!(store.try_provided().unwrap().is_empty());
    }

    #[test]
    fn corrupt_records_report_errors_without_panicking() {
        let store = create_store();
        let key = RecordKey::new(b"corrupt");
        let write = store.db.begin_write().unwrap();
        {
            let mut table = write.open_table(RECORDS).unwrap();
            table.insert(key.as_ref(), &[0xff][..]).unwrap();
        }
        write.commit().unwrap();

        let error = store.try_get(&key).unwrap_err();
        assert!(error.to_string().contains("failed to decode record"));
        assert!(std::error::Error::source(&error).is_some());
        assert!(store.try_records().is_err());
        assert!(store.get(&key).is_none());
        assert_eq!(store.records().count(), 0);
    }

    #[test]
    fn corrupt_provider_aborts_removal() {
        let mut store = create_store();
        let key = RecordKey::new(b"provider");
        let valid = postcard::to_stdvec(&ProviderRecordSerde {
            key: key.clone(),
            provider: store.peer,
            expires: None,
            addresses: Vec::new(),
        })
        .unwrap();
        let corrupt = &[0xff][..];
        let write = store.db.begin_write().unwrap();
        {
            let mut table = write.open_multimap_table(PROVIDER_RECORDS).unwrap();
            table.insert(key.as_ref(), valid.as_slice()).unwrap();
            table.insert(key.as_ref(), corrupt).unwrap();
        }
        write.commit().unwrap();

        assert!(store.try_providers(&key).is_err());
        assert!(store.try_provided().is_err());
        assert!(store.providers(&key).is_empty());
        assert_eq!(store.provided().count(), 0);
        let peer = store.peer;
        assert!(store.try_remove_provider(&key, &peer).is_err());
        store.remove_provider(&key, &peer);

        let read = store.db.begin_read().unwrap();
        let table = read.open_multimap_table(PROVIDER_RECORDS).unwrap();
        let values: Vec<_> = table
            .get(key.as_ref())
            .unwrap()
            .map(|entry| entry.unwrap().value().to_vec())
            .collect();
        assert_eq!(values.len(), 2);
        assert!(values.contains(&valid));
    }

    #[test]
    fn table_type_errors_are_reported_for_writes() {
        let mut store = create_store();
        let write = store.db.begin_write().unwrap();
        {
            let wrong_type: redb::TableDefinition<&[u8], u64> =
                redb::TableDefinition::new("KAD-REDB.RECORDS");
            write.open_table(wrong_type).unwrap();
        }
        write.commit().unwrap();

        let key = RecordKey::new(b"hello");
        let error = store
            .try_put(Record::new(key.clone(), b"world".to_vec()))
            .unwrap_err();
        assert!(error.to_string().contains("failed to open records table"));
        assert!(matches!(
            store.put(Record::new(key.clone(), b"world".to_vec())),
            Err(libp2p::kad::store::Error::MaxRecords)
        ));
        assert!(store.try_remove(&key).is_err());
        store.remove(&key);
        assert!(store.try_get(&key).is_err());
    }
}
