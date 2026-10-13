use super::Net;
use color_eyre::eyre::eyre;
use libp2p::kad;
use std::collections::HashMap;
use tokio::sync::oneshot;

type GetReply = oneshot::Sender<color_eyre::Result<Option<shared::RecordPayload>>>;

pub enum Request {
    List {
        reply: oneshot::Sender<color_eyre::Result<HashMap<String, shared::RecordPayload>>>,
    },
    Get {
        name: String,
        reply: GetReply,
    },
    Put {
        name: String,
        payload: shared::RecordPayload,
        reply: oneshot::Sender<color_eyre::Result<()>>,
    },
    Delete {
        name: String,
        reply: oneshot::Sender<color_eyre::Result<()>>,
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
    pub fn handle(&mut self, net: &mut Net, request: Request) {
        match request {
            Request::List { reply } => {
                let result = Self::list(net);
                let _ = reply.send(result);
            }
            Request::Get { name, reply } => {
                let key = kad::RecordKey::new(&shared::Provider::key(name));
                let kad = &mut net.swarm.behaviour_mut().rpws_kad;
                let stored = match kad.store_mut().try_get(&key) {
                    Ok(stored) => stored,
                    Err(error) => {
                        let _ = reply.send(Err(error.into()));
                        return;
                    }
                };
                if stored.is_some() {
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
                let result = Self::put(net, name, payload);
                let _ = reply.send(result);
            }
            Request::Delete { name, reply } => {
                let result = Self::delete(net, name);
                let _ = reply.send(result);
            }
        }
    }

    // Remove the local name mapping and stop advertising it. Remote replicas
    // are independent; Kademlia does not support a distributed delete.
    fn delete(net: &mut Net, name: String) -> color_eyre::Result<()> {
        let key = kad::RecordKey::new(&shared::Provider::key(name));
        let peer = *net.swarm.local_peer_id();
        let kad = &mut net.swarm.behaviour_mut().rpws_kad;
        kad.store_mut().try_remove(&key)?;
        kad.store_mut().try_remove_provider(&key, &peer)?;
        kad.stop_providing(&key);
        Ok(())
    }

    fn list(net: &mut Net) -> color_eyre::Result<HashMap<String, shared::RecordPayload>> {
        let records = net
            .swarm
            .behaviour_mut()
            .rpws_kad
            .store_mut()
            .try_records()?;
        let records: HashMap<_, _> = records
            .into_iter()
            .map(|entry| (entry.key.clone(), entry))
            .collect();
        Ok(records
            .values()
            .filter_map(|entry| {
                let provider = shared::Provider::decode(&entry.value).ok()?;
                if !provider.verify(entry.key.as_ref(), &net.secret) {
                    return None;
                }
                let name = provider.name(&net.secret)?;
                let key = kad::RecordKey::new(provider.hash());
                let entry = records.get(&key)?;
                let record = shared::Record::decode(&entry.value).ok()?;
                if entry.key.as_ref() != record.hash() || !record.verify(&net.secret) {
                    return None;
                }
                Some((name, record.payload(&net.secret)?))
            })
            .collect())
    }

    fn put(net: &mut Net, name: String, payload: shared::RecordPayload) -> color_eyre::Result<()> {
        let record = shared::Record::new(payload, &net.secret);
        let provider = shared::Provider::new(&name, &record, &net.secret);
        let key = kad::RecordKey::new(&shared::Provider::key(&name));
        let record = kad::Record::new(record.hash().to_vec(), record.encode());
        let provider = kad::Record::new(key.clone(), provider.encode());
        let kad = &mut net.swarm.behaviour_mut().rpws_kad;
        kad.store_mut().try_put(record.clone())?;
        kad.store_mut().try_put(provider.clone())?;
        kad.put_record(record, kad::Quorum::One)?;
        kad.put_record(provider, kad::Quorum::One)?;
        kad.start_providing(key)?;
        Ok(())
    }

    pub fn progressed(
        &mut self,
        net: &mut Net,
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
        if query.reply().is_closed() {
            Self::finish(net, id);
            return;
        }
        match (query, result) {
            (Query::Providers { key, reply }, kad::QueryResult::GetProviders(result)) => {
                match result {
                    Ok(kad::GetProvidersOk::FoundProviders { providers, .. }) => {
                        for peer in providers {
                            if peer != *net.swarm.local_peer_id() && !net.swarm.is_connected(&peer)
                            {
                                let _ = net.swarm.dial(peer);
                            }
                        }
                    }
                    Ok(kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. }) => {}
                    Err(error) => tracing::debug!("Password provider search failed: {error}"),
                }
                Self::finish(net, id);
                let id = net.swarm.behaviour_mut().rpws_kad.get_record(key.clone());
                self.0.insert(id, Query::Provider { key, reply });
            }
            (query, kad::QueryResult::GetRecord(Ok(kad::GetRecordOk::FoundRecord(found)))) => {
                match query {
                    Query::Provider { key, reply } => {
                        let provider =
                            shared::Provider::decode(&found.record.value)
                                .ok()
                                .filter(|provider| {
                                    found.record.key == key
                                        && provider.verify(key.as_ref(), &net.secret)
                                });
                        if let Some(provider) = provider {
                            Self::finish(net, id);
                            let key = kad::RecordKey::new(provider.hash());
                            let id = net.swarm.behaviour_mut().rpws_kad.get_record(key.clone());
                            self.0.insert(id, Query::Record { key, reply });
                        } else if last {
                            let _ = reply.send(Err(eyre!("Invalid password provider")));
                        } else {
                            self.0.insert(id, Query::Provider { key, reply });
                        }
                    }
                    Query::Record { key, reply } => {
                        let payload = shared::Record::decode(&found.record.value)
                            .ok()
                            .filter(|record| {
                                found.record.key == key
                                    && key.as_ref() == record.hash()
                                    && record.verify(&net.secret)
                            })
                            .and_then(|record| record.payload(&net.secret));
                        if let Some(payload) = payload {
                            Self::finish(net, id);
                            let _ = reply.send(Ok(Some(payload)));
                        } else if last {
                            let _ = reply.send(Err(eyre!("Invalid password record")));
                        } else {
                            self.0.insert(id, Query::Record { key, reply });
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

    fn finish(net: &mut Net, id: kad::QueryId) {
        if let Some(mut query) = net.swarm.behaviour_mut().rpws_kad.query_mut(&id) {
            query.finish();
        }
    }

    pub fn maintain(&mut self, net: &mut Net) {
        self.0.retain(|id, query| {
            if query.reply().is_closed() {
                Self::finish(net, *id);
                false
            } else {
                true
            }
        });
    }
}
