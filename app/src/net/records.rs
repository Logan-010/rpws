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

    record.key.as_ref() == r.hash() && r.verify(secret) && r.payload(secret).is_some()
}

pub fn verify_rpws_provider(provider: &kad::ProviderRecord) -> bool {
    shared::Provide::is_valid(&provider.key)
}
