use crate::{
    api::{
        records,
        requests::{Request, Requests},
        swarm::{Behaviour, BehaviourEvent},
    },
    save::Save,
};
use libp2p::{
    Multiaddr, StreamProtocol, Swarm,
    futures::{FutureExt, StreamExt, channel::mpsc},
    identify,
    kad::{self, store::RecordStore},
    multiaddr::Protocol,
    swarm::SwarmEvent,
};
use multihash_codetable::{Code, MultihashDigest};
use web_time::{Duration, Instant};

fn dialable_address(address: &Multiaddr) -> bool {
    address.iter().all(|protocol| match protocol {
        Protocol::Ip4(ip) => !ip.is_link_local(),
        Protocol::Ip6(ip) => !ip.is_unicast_link_local(),
        Protocol::Tcp(_) | Protocol::Tls | Protocol::Ws(_) | Protocol::Wss(_) => true,
        Protocol::P2p(_) => true,
        Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dns(_) => true,
        _ => false,
    }) && address
        .iter()
        .any(|protocol| matches!(protocol, Protocol::Ws(_) | Protocol::Wss(_)))
}

pub async fn task(
    mut swarm: Swarm<Behaviour>,
    save: Save,
    mut receiver: mpsc::UnboundedReceiver<Request>,
) -> color_eyre::Result<()> {
    let mut requests = Requests::default();
    let secret = shared::derive_key(save.secret);
    let mut sync = super::sync::Sync::default();
    for address in &save.bootstrap_addrs {
        if let Some(Protocol::P2p(peer)) = address.iter().last() {
            sync.bootstraps.insert(peer);
        }
    }
    sync.start(&mut swarm, &secret, None);
    let mut next_sync = Instant::now() + Duration::from_secs(5);
    let mut maintenance = gloo_timers::future::IntervalStream::new(1_000).fuse();
    let provider_key = kad::RecordKey::new(&Code::Blake3_256.digest(&save.secret).to_bytes());
    let mut provider_query = None;
    let mut next_search = Instant::now();
    loop {
        let event = libp2p::futures::select! {
            event = swarm.select_next_some().fuse() => event,
            request = receiver.next().fuse() => {
                let Some(request) = request else { return Ok(()); };
                match request {
                    Request::Sync { reply } => sync.start(&mut swarm, &secret, Some(reply)),
                    request => requests.handle(&mut swarm, &secret, request),
                }
                continue;
            },
            _ = maintenance.select_next_some() => {
                requests.maintain(&mut swarm);
                if Instant::now() >= next_sync {
                    sync.start(&mut swarm, &secret, None);
                    next_sync = Instant::now() + Duration::from_secs(5);
                }
                if provider_query.is_none() && Instant::now() >= next_search {
                    provider_query = Some(swarm.behaviour_mut().rpws_kad.get_providers(provider_key.clone()));
                }
                continue;
            },
        };
        match event {
            SwarmEvent::OutgoingConnectionError {
                connection_id,
                peer_id,
                error,
            } => {
                tracing::warn!("WebSocket dial failed for {peer_id:?}: {error}");
                sync.dial_failed(connection_id, peer_id, &error);
            }
            SwarmEvent::Behaviour(BehaviourEvent::Sync(event)) => {
                sync.handle(&mut swarm, &secret, event)
            }
            SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(
                kad::Event::OutboundQueryProgressed {
                    id, result, step, ..
                },
            )) if provider_query == Some(id) => {
                match result {
                    kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                        providers,
                        ..
                    })) => {
                        for peer in providers {
                            if peer != *swarm.local_peer_id()
                                && !swarm.is_connected(&peer)
                                && let Err(error) = swarm.dial(peer)
                            {
                                tracing::debug!("Failed to dial provider {peer}: {error}");
                            }
                        }
                    }
                    kad::QueryResult::GetProviders(Err(error)) => {
                        tracing::debug!("Provider search failed: {error}");
                    }
                    _ => {}
                }
                if step.last {
                    provider_query = None;
                    next_search = Instant::now() + Duration::from_secs(5);
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(
                kad::Event::OutboundQueryProgressed {
                    id, result, step, ..
                },
            )) => requests.progressed(&mut swarm, &secret, id, result, step.last),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info:
                    identify::Info {
                        listen_addrs,
                        protocols,
                        ..
                    },
                ..
            })) => {
                tracing::trace!("Connected to {}", peer_id);
                if protocols.contains(&StreamProtocol::new(shared::sync::PROTOCOL)) {
                    sync.peers.insert(peer_id);
                    sync.start(&mut swarm, &secret, None);
                }

                if protocols.contains(&StreamProtocol::new(Behaviour::RPWS_KAD_PROTOCOL)) {
                    for addr in listen_addrs.iter().filter(|addr| dialable_address(addr)) {
                        swarm
                            .behaviour_mut()
                            .rpws_kad
                            .add_address(&peer_id, addr.clone());
                    }
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(kad::Event::InboundRequest {
                request: kad::InboundRequest::AddProvider { record: Some(r) },
            })) => {
                if records::verify_rpws_provider(&r)
                    && let Err(e) = swarm.behaviour_mut().rpws_kad.store_mut().add_provider(r)
                {
                    tracing::warn!("Failed to add provider: {e:?}");
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(kad::Event::InboundRequest {
                request:
                    kad::InboundRequest::PutRecord {
                        record: Some(mut r),
                        ..
                    },
            })) => {
                let store = swarm.behaviour_mut().rpws_kad.store_mut();
                // Deletion markers must not age out and allow stale replicas back.
                r.expires = None;
                if records::accepts(store, &r, &secret)
                    && let Err(e) = store.put(r)
                {
                    tracing::warn!("Failed to put record: {e:?}");
                }
            }
            ev => tracing::trace!("Swarm event: {ev:?}"),
        }
    }
}
