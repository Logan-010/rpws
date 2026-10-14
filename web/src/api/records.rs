use super::store::SerializableMemoryStore;
use libp2p::kad;
use libp2p::kad::store::RecordStore;

pub fn verify_rpws_record(record: &kad::Record, secret: &[u8; 32]) -> bool {
    if let Ok(provider) = shared::Provider::decode(&record.value)
        && provider.verify(record.key.as_ref(), secret)
    {
        return true;
    }
    let Ok(r) = shared::Record::decode(&record.value) else {
        return false;
    };

    record.key.as_ref() == r.hash() && r.verify(secret) && r.state(secret).is_some()
}

pub fn state(
    store: &SerializableMemoryStore,
    key: &kad::RecordKey,
    secret: &[u8; 32],
) -> Option<shared::RecordState> {
    let mapping = store.get(key)?;
    shared::sync::mapping_state(&mapping.value, secret, |hash| {
        store
            .get(&kad::RecordKey::new(&hash))
            .map(|r| r.value.clone())
    })
}

pub fn accepts(store: &SerializableMemoryStore, record: &kad::Record, secret: &[u8; 32]) -> bool {
    if !verify_rpws_record(record, secret) {
        return false;
    }
    if shared::Provider::decode(&record.value).is_err() {
        return true;
    }
    let current = store.get(&record.key);
    shared::sync::accepts_mapping(
        current.as_ref().map(|r| r.value.as_slice()),
        &record.value,
        secret,
        |hash| {
            store
                .get(&kad::RecordKey::new(&hash))
                .map(|r| r.value.clone())
        },
    )
}

pub fn verify_rpws_provider(provider: &kad::ProviderRecord) -> bool {
    shared::Provide::is_valid(&provider.key)
}
