use super::types::Behaviour;
use libp2p::{Multiaddr, PeerId, Swarm, core::transport::ListenerId, multiaddr::Protocol};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub(super) const HOP_PROTOCOL: &str = "/libp2p/circuit/relay/0.2.0/hop";
const MAX_RESERVATIONS: usize = 2;
const RETRY_DELAY: Duration = Duration::from_secs(300);

struct Candidate {
    address: Multiaddr,
    retry_at: Instant,
}

#[derive(Default)]
pub(super) struct Relays {
    candidates: HashMap<PeerId, Candidate>,
    listeners: HashMap<ListenerId, PeerId>,
    direct_addresses: HashMap<PeerId, Multiaddr>,
}

impl Relays {
    pub fn connected(&mut self, peer: PeerId, address: Multiaddr) {
        self.direct_addresses.insert(peer, address);
    }

    pub fn discover(&mut self, peer: PeerId, addresses: &[Multiaddr]) {
        if self.candidates.contains_key(&peer) {
            return;
        }

        if let Some(address) = self
            .direct_addresses
            .get(&peer)
            .into_iter()
            .chain(addresses)
            .find(|addr| {
                !addr.iter().any(|p| matches!(p, Protocol::P2pCircuit))
                    && addr
                        .iter()
                        .any(|p| matches!(p, Protocol::Tcp(_) | Protocol::QuicV1))
            })
        {
            let Ok(address) = address.clone().with_p2p(peer) else {
                return;
            };
            self.candidates.insert(
                peer,
                Candidate {
                    address,
                    retry_at: Instant::now(),
                },
            );
        }
    }

    pub fn listener_closed(&mut self, listener: ListenerId) {
        if let Some(peer) = self.listeners.remove(&listener)
            && let Some(candidate) = self.candidates.get_mut(&peer)
        {
            candidate.retry_at = Instant::now() + RETRY_DELAY;
        }
    }

    pub fn reserve(&mut self, swarm: &mut Swarm<Behaviour>) {
        let now = Instant::now();
        for (peer, candidate) in &mut self.candidates {
            if self.listeners.len() >= MAX_RESERVATIONS {
                break;
            }
            if candidate.retry_at > now || self.listeners.values().any(|p| p == peer) {
                continue;
            }
            let address = candidate.address.clone().with(Protocol::P2pCircuit);
            tracing::debug!("Requesting relay reservation on {address}");
            match swarm.listen_on(address) {
                Ok(listener) => {
                    self.listeners.insert(listener, *peer);
                }
                Err(error) => {
                    tracing::warn!("Cannot listen through relay {peer}: {error}");
                    candidate.retry_at = now + RETRY_DELAY;
                }
            }
        }
    }
}
