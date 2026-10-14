use super::{store::SerializableMemoryStore, swarm::Behaviour};
use color_eyre::eyre::eyre;
use libp2p::{
    Swarm,
    futures::channel::oneshot,
    kad::{self, store::RecordStore},
};
use std::collections::HashMap;

type Reply<T> = oneshot::Sender<color_eyre::Result<T>>;
type GetReply = Reply<Option<shared::RecordPayload>>;

pub enum Request {
    Sync {
        reply: super::sync::Reply,
    },
    Connect {
        reply: Reply<()>,
    },
    List {
        reply: Reply<HashMap<String, shared::RecordPayload>>,
    },
    Get {
        name: String,
        reply: GetReply,
    },
    Put {
        name: String,
        payload: shared::RecordPayload,
        reply: Reply<()>,
    },
    Delete {
        name: String,
        reply: Reply<()>,
    },
    KadSnapshot {
        reply: Reply<SerializableMemoryStore>,
    },
}

enum Query {
    Providers {
        key: kad::RecordKey,
        reply: GetReply,
    },
    Provider {
        key: kad::RecordKey,
        reply: GetReply,
    },
    Record {
        key: kad::RecordKey,
        mapping: kad::RecordKey,
        reply: GetReply,
    },
}

impl Query {
    fn reply(&self) -> &GetReply {
        match self {
            Self::Providers { reply, .. }
            | Self::Provider { reply, .. }
            | Self::Record { reply, .. } => reply,
        }
    }

    fn into_reply(self) -> GetReply {
        match self {
            Self::Providers { reply, .. }
            | Self::Provider { reply, .. }
            | Self::Record { reply, .. } => reply,
        }
    }
}

#[derive(Default)]
pub(super) struct Requests(HashMap<kad::QueryId, Query>);

impl Requests {
    pub fn handle(&mut self, swarm: &mut Swarm<Behaviour>, secret: &[u8; 32], request: Request) {
        match request {
            Request::Sync { .. } => unreachable!("Sync requests are handled by the swarm task"),
            Request::Connect { reply } => {
                let _ = reply.send(Ok(()));
            }
            Request::KadSnapshot { reply } => {
                let store = swarm.behaviour_mut().rpws_kad.store_mut();
                store.prune_expired();
                let _ = reply.send(Ok(store.clone()));
            }
            Request::List { reply } => {
                let _ = reply.send(Ok(Self::list(swarm, secret)));
            }
            Request::Get { name, reply } => {
                let key = kad::RecordKey::new(&shared::Provider::key(name));
                let kad = &mut swarm.behaviour_mut().rpws_kad;
                kad.store_mut().prune_expired();
                if super::records::state(kad.store_mut(), &key, secret)
                    .is_some_and(|state| state.version().1)
                {
                    let _ = reply.send(Ok(None));
                    return;
                }
                if kad.store_mut().get(&key).is_some() {
                    let id = kad.get_record(key.clone());
                    self.0.insert(id, Query::Provider { key, reply });
                } else {
                    let id = kad.get_providers(key.clone());
                    self.0.insert(id, Query::Providers { key, reply });
                }
            }
            Request::Put {
                name,
                payload,
                reply,
            } => {
                let _ = reply.send(Self::put(swarm, secret, name, payload));
            }
            Request::Delete { name, reply } => {
                let key = kad::RecordKey::new(&shared::Provider::key(&name));
                let now = web_time::SystemTime::now()
                    .duration_since(web_time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                let timestamp =
                    super::records::state(swarm.behaviour_mut().rpws_kad.store_mut(), &key, secret)
                        .map_or(now, |state| state.next_timestamp(now));
                let _ = reply.send(Self::put_state(
                    swarm,
                    secret,
                    name,
                    shared::RecordState::Deleted { timestamp },
                ));
            }
        }
    }

    fn list(
        swarm: &mut Swarm<Behaviour>,
        secret: &[u8; 32],
    ) -> HashMap<String, shared::RecordPayload> {
        let store = swarm.behaviour_mut().rpws_kad.store_mut();
        store.prune_expired();
        let records: HashMap<_, _> = store
            .records()
            .map(|entry| (entry.key.clone(), entry))
            .collect();
        records
            .values()
            .filter_map(|entry| {
                let provider = shared::Provider::decode(&entry.value).ok()?;
                if !provider.verify(entry.key.as_ref(), secret) {
                    return None;
                }
                let name = provider.name(secret)?;
                let key = kad::RecordKey::new(provider.hash());
                let entry = records.get(&key)?;
                let record = shared::Record::decode(&entry.value).ok()?;
                if entry.key.as_ref() != record.hash() || !record.verify(secret) {
                    return None;
                }
                Some((name, record.payload(secret)?))
            })
            .collect()
    }

    fn put(
        swarm: &mut Swarm<Behaviour>,
        secret: &[u8; 32],
        name: String,
        mut payload: shared::RecordPayload,
    ) -> color_eyre::Result<()> {
        let key = kad::RecordKey::new(&shared::Provider::key(&name));
        if let Some(state) =
            super::records::state(swarm.behaviour_mut().rpws_kad.store_mut(), &key, secret)
        {
            payload.timestamp = state.next_timestamp(payload.timestamp);
        }
        Self::put_state(swarm, secret, name, shared::RecordState::Live(payload))
    }

    fn put_state(
        swarm: &mut Swarm<Behaviour>,
        secret: &[u8; 32],
        name: String,
        state: shared::RecordState,
    ) -> color_eyre::Result<()> {
        let deleted = state.version().1;
        let peer = *swarm.local_peer_id();
        let record = shared::Record::from_state(state, secret);
        let provider = shared::Provider::new(&name, &record, secret);
        let key = kad::RecordKey::new(&shared::Provider::key(&name));
        let record = kad::Record::new(record.hash().to_vec(), record.encode());
        let provider = kad::Record::new(key.clone(), provider.encode());
        let kad = &mut swarm.behaviour_mut().rpws_kad;
        kad.store_mut().prune_expired();
        kad.store_mut().put(record.clone())?;
        kad.store_mut().put(provider.clone())?;
        kad.put_record(record, kad::Quorum::One)?;
        kad.put_record(provider, kad::Quorum::One)?;
        if deleted {
            kad.store_mut().remove_provider(&key, &peer);
            kad.stop_providing(&key);
        } else {
            kad.start_providing(key)?;
        }
        Ok(())
    }

    pub fn progressed(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        secret: &[u8; 32],
        id: kad::QueryId,
        result: kad::QueryResult,
        last: bool,
    ) {
        let Some(query) = self.0.remove(&id) else {
            match result {
                kad::QueryResult::PutRecord(Err(error)) => {
                    tracing::warn!("Failed to publish password record: {error}")
                }
                kad::QueryResult::StartProviding(Err(error)) => {
                    tracing::warn!("Failed to advertise password provider: {error}")
                }
                result => tracing::debug!("RPWS query result: {result:?}"),
            }
            return;
        };
        if query.reply().is_canceled() {
            Self::finish(swarm, id);
            return;
        }
        match (query, result) {
            (Query::Providers { key, reply }, kad::QueryResult::GetProviders(result)) => {
                match result {
                    Ok(kad::GetProvidersOk::FoundProviders { providers, .. }) => {
                        for peer in providers {
                            if peer != *swarm.local_peer_id() && !swarm.is_connected(&peer) {
                                let _ = swarm.dial(peer);
                            }
                        }
                    }
                    Ok(kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. }) => {}
                    Err(error) => tracing::debug!("Password provider search failed: {error}"),
                }
                Self::finish(swarm, id);
                let id = swarm.behaviour_mut().rpws_kad.get_record(key.clone());
                self.0.insert(id, Query::Provider { key, reply });
            }
            (query, kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(mut found)))) => {
                found.record.expires = None;
                match query {
                    Query::Provider { key, reply } => {
                        if super::records::state(
                            swarm.behaviour_mut().rpws_kad.store_mut(),
                            &key,
                            secret,
                        )
                        .is_some_and(|state| state.version().1)
                        {
                            Self::finish(swarm, id);
                            let _ = reply.send(Ok(None));
                            return;
                        }
                        let provider =
                            shared::Provider::decode(&found.record.value)
                                .ok()
                                .filter(|provider| {
                                    found.record.key == key && provider.verify(key.as_ref(), secret)
                                });
                        if let Some(provider) = provider {
                            Self::finish(swarm, id);
                            let mapping = key;
                            let key = kad::RecordKey::new(provider.hash());
                            // Cache verified remote results so they appear in list
                            // and survive a Save round trip.
                            if let Err(error) =
                                swarm.behaviour_mut().rpws_kad.store_mut().put(found.record)
                            {
                                tracing::warn!("Failed to cache password provider: {error}");
                            }
                            let id = swarm.behaviour_mut().rpws_kad.get_record(key.clone());
                            self.0.insert(
                                id,
                                Query::Record {
                                    key,
                                    mapping,
                                    reply,
                                },
                            );
                        } else if last {
                            let _ = reply.send(Err(eyre!("Invalid password provider")));
                        } else {
                            self.0.insert(id, Query::Provider { key, reply });
                        }
                    }
                    Query::Record {
                        key,
                        mapping,
                        reply,
                    } => {
                        if super::records::state(
                            swarm.behaviour_mut().rpws_kad.store_mut(),
                            &mapping,
                            secret,
                        )
                        .is_some_and(|state| state.version().1)
                        {
                            Self::finish(swarm, id);
                            let _ = reply.send(Ok(None));
                            return;
                        }
                        let state = shared::Record::decode(&found.record.value)
                            .ok()
                            .filter(|record| {
                                found.record.key == key
                                    && key.as_ref() == record.hash()
                                    && record.verify(secret)
                            })
                            .and_then(|record| record.state(secret));
                        if let Some(state) = state {
                            Self::finish(swarm, id);
                            if let Err(error) =
                                swarm.behaviour_mut().rpws_kad.store_mut().put(found.record)
                            {
                                tracing::warn!("Failed to cache password record: {error}");
                            }
                            let _ = reply.send(Ok(match state {
                                shared::RecordState::Live(payload) => Some(payload),
                                shared::RecordState::Deleted { .. } => None,
                            }));
                        } else if last {
                            let _ = reply.send(Err(eyre!("Invalid password record")));
                        } else {
                            self.0.insert(
                                id,
                                Query::Record {
                                    key,
                                    mapping,
                                    reply,
                                },
                            );
                        }
                    }
                    Query::Providers { reply, .. } => {
                        let _ = reply.send(Err(eyre!("Unexpected password query result")));
                    }
                }
            }
            (query, kad::QueryResult::GetRecord(Err(kad::GetRecordError::NotFound { .. }))) => {
                let _ = query.into_reply().send(Ok(None));
            }
            (query, kad::QueryResult::GetRecord(Err(error))) => {
                let _ = query
                    .into_reply()
                    .send(Err(eyre!("Password lookup failed: {error}")));
            }
            (query, _) if !last => {
                self.0.insert(id, query);
            }
            (query, _) => {
                let _ = query.into_reply().send(Err(eyre!(
                    "Password lookup completed without a valid record"
                )));
            }
        }
    }

    fn finish(swarm: &mut Swarm<Behaviour>, id: kad::QueryId) {
        if let Some(mut query) = swarm.behaviour_mut().rpws_kad.query_mut(&id) {
            query.finish();
        }
    }

    pub fn maintain(&mut self, swarm: &mut Swarm<Behaviour>) {
        self.0.retain(|id, query| {
            if query.reply().is_canceled() {
                Self::finish(swarm, *id);
                false
            } else {
                true
            }
        });
    }
}
