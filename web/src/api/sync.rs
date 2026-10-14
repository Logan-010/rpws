use super::swarm::Behaviour;
use color_eyre::eyre::eyre;
use libp2p::{
    PeerId, Swarm,
    futures::channel::oneshot,
    kad::{self, store::RecordStore},
    request_response::{Event, Message, OutboundRequestId},
};
use std::collections::{HashMap, HashSet};

pub type Reply = oneshot::Sender<color_eyre::Result<u32>>;

#[derive(Default)]
pub struct Sync {
    pub peers: HashSet<PeerId>,
    pub bootstraps: HashSet<PeerId>,
    pending: HashMap<OutboundRequestId, PeerId>,
    reply: Option<Reply>,
    received: u32,
    succeeded: bool,
    error: Option<String>,
}

pub fn snapshot(swarm: &mut Swarm<Behaviour>, key: &[u8; 32]) -> Vec<shared::sync::Entry> {
    let store = swarm.behaviour_mut().rpws_kad.store_mut();
    store.prune_expired();
    shared::sync::entries(
        store
            .records()
            .map(|record| (record.key.to_vec(), record.value.clone())),
        key,
    )
}

pub fn merge(
    swarm: &mut Swarm<Behaviour>,
    key: &[u8; 32],
    entries: Vec<shared::sync::Entry>,
) -> color_eyre::Result<u32> {
    let mut versions: HashMap<_, _> = snapshot(swarm, key)
        .iter()
        .filter_map(|e| e.decode_state(key))
        .map(|(name, state)| (name, state.version()))
        .collect();
    let mut count = 0;
    for entry in entries {
        let Some((name, state)) = entry.decode_state(key) else {
            continue;
        };
        if versions
            .get(&name)
            .is_some_and(|version| *version >= state.version())
        {
            continue;
        }
        for (record_key, value) in entry.records(key).expect("Verified entry") {
            swarm
                .behaviour_mut()
                .rpws_kad
                .store_mut()
                .put(kad::Record::new(record_key, value))?;
        }
        versions.insert(name, state.version());
        if !state.version().1 {
            count += 1;
        }
    }
    Ok(count)
}

impl Sync {
    pub fn start(&mut self, swarm: &mut Swarm<Behaviour>, key: &[u8; 32], reply: Option<Reply>) {
        if let Some(reply) = reply {
            self.reply = Some(reply);
            self.received = 0;
            self.succeeded = false;
            self.error = None;
        }
        let peers: HashSet<_> = self
            .bootstraps
            .iter()
            .copied()
            .chain(
                self.peers
                    .iter()
                    .copied()
                    .filter(|peer| swarm.is_connected(peer)),
            )
            .collect();
        for peer in peers {
            if self.pending.values().any(|pending| *pending == peer) {
                continue;
            }
            let id = swarm
                .behaviour_mut()
                .sync
                .send_request(&peer, shared::sync::Request::new(key));
            self.pending.insert(id, peer);
        }
        self.finish();
    }

    fn finish(&mut self) {
        if !self.pending.is_empty() {
            return;
        }
        if let Some(reply) = self.reply.take() {
            let result = if self.succeeded || self.error.is_none() {
                Ok(self.received)
            } else {
                Err(eyre!("Vault sync failed: {}", self.error.take().unwrap()))
            };
            let _ = reply.send(result);
        }
    }

    pub fn handle(
        &mut self,
        swarm: &mut Swarm<Behaviour>,
        key: &[u8; 32],
        event: Event<shared::sync::Request, shared::sync::Response>,
    ) {
        match event {
            Event::Message {
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                let matched = request.vault == shared::sync::Request::new(key).vault;
                let entries = if matched {
                    snapshot(swarm, key)
                } else {
                    Vec::new()
                };
                let _ = swarm
                    .behaviour_mut()
                    .sync
                    .send_response(channel, shared::sync::Response { matched, entries });
            }
            Event::Message {
                message:
                    Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => {
                if self.pending.remove(&request_id).is_none() {
                    return;
                }
                if !response.matched {
                    self.error = Some("The peer uses a different vault secret".into());
                } else {
                    match merge(swarm, key, response.entries) {
                        Ok(count) => {
                            self.received += count;
                            self.succeeded = true;
                        }
                        Err(error) => self.error = Some(error.to_string()),
                    }
                }
                self.finish();
            }
            Event::OutboundFailure {
                request_id, error, ..
            } => {
                self.pending.remove(&request_id);
                self.error = Some(format!(
                    "{error}. Check the WebSocket bootstrap address and that the daemon supports vault sync"
                ));
                self.finish();
            }
            _ => {}
        }
    }
}
