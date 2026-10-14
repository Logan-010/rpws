use libp2p::kad;
use multihash_codetable::Multihash;

pub fn verify_ipfs_record(_record: &kad::Record) -> bool {
    // Currently, IDGAF about IPFS records. They're not useful for this app.
    false
}

pub fn verify_ipfs_provider(provider: &kad::ProviderRecord) -> bool {
    let dat: &[u8] = provider.key.as_ref();
    // Simply check if the key is a valid multihash
    Multihash::from_bytes(dat).is_ok()
}

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
    store: &libp2p_kad_redb::RedbStore,
    key: &kad::RecordKey,
    secret: &[u8; 32],
) -> color_eyre::Result<Option<shared::RecordState>> {
    let Some(mapping) = store.try_get(key)? else {
        return Ok(None);
    };
    let provider = match shared::Provider::decode(&mapping.value) {
        Ok(p) => p,
        Err(_) => return Ok(None),
    };
    let record = store.try_get(&kad::RecordKey::new(provider.hash()))?;
    Ok(shared::sync::mapping_state(&mapping.value, secret, |_| {
        record.as_ref().map(|r| r.value.clone())
    }))
}

pub fn accepts(
    store: &libp2p_kad_redb::RedbStore,
    record: &kad::Record,
    secret: &[u8; 32],
) -> color_eyre::Result<bool> {
    if !verify_rpws_record(record, secret) {
        return Ok(false);
    }
    if shared::Provider::decode(&record.value).is_err() {
        return Ok(true);
    }
    let current = store.try_get(&record.key)?;
    let mut error = None;
    let accepted = shared::sync::accepts_mapping(
        current.as_ref().map(|r| r.value.as_slice()),
        &record.value,
        secret,
        |hash| match store.try_get(&kad::RecordKey::new(&hash)) {
            Ok(record) => record.map(|r| r.value.clone()),
            Err(e) => {
                error = Some(e);
                None
            }
        },
    );
    if let Some(error) = error {
        return Err(error.into());
    }
    Ok(accepted)
}

pub fn verify_rpws_provider(provider: &kad::ProviderRecord) -> bool {
    shared::Provide::is_valid(&provider.key)
}
