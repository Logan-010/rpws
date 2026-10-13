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
    let Ok(r) = shared::Record::decode(&record.value) else {
        return false;
    };

    r.verify(secret) && r.payload(secret).is_some()
}

pub fn verify_rpws_provider(provider: &kad::ProviderRecord) -> bool {
    shared::Provide::is_valid(&provider.key)
}
