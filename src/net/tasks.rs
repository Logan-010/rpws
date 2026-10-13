use super::{
    Net, records,
    types::{Behaviour, BehaviourEvent},
};
use cid::Cid;
use libp2p::{
    StreamProtocol,
    futures::StreamExt,
    identify,
    kad::{self, store::RecordStore},
    mdns,
    swarm::SwarmEvent,
};
use multihash_codetable::{Code, MultihashDigest};
use std::collections::HashSet;
use std::time::Duration;

fn handle_identify(net: &mut Net, peer_id: libp2p::PeerId, info: identify::Info) {
    if info
        .protocols
        .contains(&StreamProtocol::new(Behaviour::IPFS_KAD_PROTOCOL))
    {
        for addr in &info.listen_addrs {
            net.swarm
                .behaviour_mut()
                .ipfs_kad
                .add_address(&peer_id, addr.clone());
        }
    }
    if info
        .protocols
        .contains(&StreamProtocol::new(Behaviour::RPWS_KAD_PROTOCOL))
    {
        for addr in &info.listen_addrs {
            net.swarm
                .behaviour_mut()
                .rpws_kad
                .add_address(&peer_id, addr.clone());
        }
    }
    if info
        .protocols
        .contains(&StreamProtocol::new(super::relays::HOP_PROTOCOL))
        && let Some(relays) = &mut net.relays
    {
        relays.discover(peer_id, &info.listen_addrs);
        relays.reserve(&mut net.swarm);
    }
}

fn refresh_provider(net: &mut Net) {
    if let Some(key) = &net.provider_key
        && let Err(error) = net
            .swarm
            .behaviour_mut()
            .ipfs_kad
            .start_providing(key.clone())
    {
        tracing::warn!("Failed to advertise provider: {error}");
    }
}

pub(super) fn handle_shared(net: &mut Net, event: SwarmEvent<BehaviourEvent>) {
    match event {
        SwarmEvent::ConnectionEstablished {
            peer_id,
            endpoint: libp2p::core::ConnectedPoint::Dialer { address, .. },
            ..
        } if !address
            .iter()
            .any(|p| matches!(p, libp2p::multiaddr::Protocol::P2pCircuit)) =>
        {
            if let Some(relays) = &mut net.relays {
                relays.connected(peer_id, address);
            }
        }
        SwarmEvent::NewListenAddr { address, .. } => tracing::info!("Listening on {}", address),
        SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
            peer_id,
            info,
            ..
        })) => {
            handle_identify(net, peer_id, info);
        }
        SwarmEvent::ExternalAddrConfirmed { address } => {
            tracing::info!("Confirmed external address {address}");
            refresh_provider(net);
        }
        SwarmEvent::ExternalAddrExpired { address } => {
            tracing::info!("External address expired: {address}");
            refresh_provider(net);
        }
        SwarmEvent::Behaviour(BehaviourEvent::Dcutr(ev)) => match ev.result {
            Ok(_) => tracing::debug!("Hole-punched {}", ev.remote_peer_id),
            Err(e) => tracing::debug!("Failed to hole-punch {}: {e:?}", ev.remote_peer_id),
        },
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
            for (id, addr) in list {
                tracing::info!("Discovered {} on {}", id, addr);

                net.swarm
                    .behaviour_mut()
                    .ipfs_kad
                    .add_address(&id, addr.clone());
                net.swarm.behaviour_mut().rpws_kad.add_address(&id, addr);
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Expired(list))) => {
            for (id, addr) in list {
                tracing::warn!("{} expired on {}", id, addr);

                net.swarm
                    .behaviour_mut()
                    .ipfs_kad
                    .remove_address(&id, &addr);
                net.swarm
                    .behaviour_mut()
                    .rpws_kad
                    .remove_address(&id, &addr);
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::Autonat(ev)) => match ev.result {
            Ok(_) => {
                tracing::info!("{} is reachable", ev.tested_addr);
                net.swarm.add_external_address(ev.tested_addr);
            }
            Err(e) => tracing::debug!(
                "{} is not reachable from {}: {}",
                ev.tested_addr,
                ev.server,
                e
            ),
        },
        SwarmEvent::Behaviour(BehaviourEvent::RelayClient(ev)) => {
            tracing::trace!("Relay client event: {ev:?}");
        }
        SwarmEvent::ListenerError { listener_id, error } => {
            tracing::warn!("Listener {listener_id:?} error: {error}");
        }
        SwarmEvent::ListenerClosed {
            listener_id,
            addresses,
            reason,
        } => {
            tracing::warn!(
                "Listener {listener_id:?} closed; addresses: {addresses:?}; reason: {reason:?}"
            );
            if let Some(relays) = &mut net.relays {
                relays.listener_closed(listener_id);
                relays.reserve(&mut net.swarm);
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::IpfsKad(kad::Event::OutboundQueryProgressed {
            result,
            ..
        })) => match result {
            kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                providers,
                ..
            })) => {
                for peer in providers {
                    if peer != *net.swarm.local_peer_id()
                        && !net.swarm.is_connected(&peer)
                        && let Err(error) = net.swarm.dial(peer)
                    {
                        tracing::debug!("Cannot dial provider {peer}: {error}");
                    }
                }
            }
            kad::QueryResult::StartProviding(Ok(_)) => {
                tracing::info!("Provider advertisement completed")
            }
            result => tracing::debug!("Kad query result: {result:?}"),
        },
        SwarmEvent::Behaviour(BehaviourEvent::IpfsKad(kad::Event::InboundRequest {
            request: kad::InboundRequest::AddProvider { record: Some(r) },
        })) => {
            if records::verify_ipfs_provider(&r)
                && let Err(e) = net
                    .swarm
                    .behaviour_mut()
                    .ipfs_kad
                    .store_mut()
                    .add_provider(r)
            {
                tracing::warn!("Failed to add provider: {e:?}");
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::IpfsKad(kad::Event::InboundRequest {
            request:
                kad::InboundRequest::PutRecord {
                    record: Some(r), ..
                },
        })) => {
            if records::verify_ipfs_record(&r)
                && let Err(e) = net.swarm.behaviour_mut().ipfs_kad.store_mut().put(r)
            {
                tracing::warn!("Failed to put record: {e:?}");
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(kad::Event::InboundRequest {
            request: kad::InboundRequest::AddProvider { record: Some(r) },
        })) => {
            if records::verify_rpws_provider(&r)
                && let Err(e) = net
                    .swarm
                    .behaviour_mut()
                    .rpws_kad
                    .store_mut()
                    .add_provider(r)
            {
                tracing::warn!("Failed to add provider: {e:?}");
            }
        }
        SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(kad::Event::InboundRequest {
            request:
                kad::InboundRequest::PutRecord {
                    record: Some(r), ..
                },
        })) => {
            if records::verify_rpws_record(&r)
                && let Err(e) = net.swarm.behaviour_mut().rpws_kad.store_mut().put(r)
            {
                tracing::warn!("Failed to put record: {e:?}");
            }
        }
        ev => tracing::trace!("Swarm event: {ev:?}"),
    }
}

pub async fn bootstrap<B: AsRef<[u8]>>(net: &mut Net, of: B) -> color_eyre::Result<()> {
    let mh = Code::Blake3_256.digest(of.as_ref());
    let cid = Cid::new_v1(Net::BOOTSTRAP_CODEC, mh);
    let key = kad::RecordKey::new(&cid.hash().to_bytes());
    net.provider_key = Some(key.clone());

    tracing::info!("Bootstrapping for {}", cid);

    let mut qid = None;
    let mut lid = None;
    let mut connected = HashSet::new();
    let mut connecting = HashSet::new();

    loop {
        if !connected.is_empty() && connecting.is_empty() {
            break;
        }

        match net.swarm.select_next_some().await {
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                tracing::trace!("Connected to {}", peer_id);

                if connecting.remove(&peer_id) {
                    connected.insert(peer_id);
                }

                handle_identify(net, peer_id, info);
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(id) = peer_id
                    && connecting.remove(&id)
                {
                    tracing::warn!("Failed to dial peer {id}: {error}");
                    if connecting.is_empty() && connected.is_empty() {
                        if let Some(quid) = qid.take()
                            && let Some(ref mut query) =
                                net.swarm.behaviour_mut().ipfs_kad.query_mut(&quid)
                        {
                            query.finish();
                        }

                        tracing::warn!("No providers found for {cid}, retrying...");
                        qid = Some(
                            net.swarm
                                .behaviour_mut()
                                .ipfs_kad
                                .get_providers(key.clone()),
                        );
                    }
                } else {
                    tracing::trace!("Connection error for {peer_id:?}: {error}");
                }
            }
            SwarmEvent::Behaviour(BehaviourEvent::IpfsKad(
                kad::Event::OutboundQueryProgressed {
                    id, result, step, ..
                },
            )) => match result {
                kad::QueryResult::Bootstrap(Ok(b)) => {
                    tracing::info!("Bootstrapped {}", b.peer);

                    if qid.is_none()
                        && (step.count.get() as u32 <= b.num_remaining / 2 || step.last)
                    {
                        qid = Some(
                            net.swarm
                                .behaviour_mut()
                                .ipfs_kad
                                .get_providers(key.clone()),
                        );
                        lid = Some(
                            net.swarm
                                .behaviour_mut()
                                .ipfs_kad
                                .start_providing(key.clone())?,
                        );
                    }
                }
                kad::QueryResult::Bootstrap(Err(e)) => {
                    tracing::warn!("IPFS bootstrap error: {e:?}")
                }
                kad::QueryResult::StartProviding(Ok(_)) => {
                    tracing::info!("Providing {cid}");
                    if lid == Some(id) {
                        break;
                    }
                }
                kad::QueryResult::StartProviding(Err(e)) => {
                    _ = lid.take();
                    tracing::warn!("Failed to provide {cid}: {e:?}, retrying...");
                    lid = Some(
                        net.swarm
                            .behaviour_mut()
                            .ipfs_kad
                            .start_providing(key.clone())?,
                    );
                }
                kad::QueryResult::GetProviders(Ok(
                    kad::GetProvidersOk::FinishedWithNoAdditionalRecord { .. },
                )) => {
                    if qid == Some(id) {
                        tracing::warn!("No providers found for {} ({}), retrying...", cid, id);
                        qid = Some(
                            net.swarm
                                .behaviour_mut()
                                .ipfs_kad
                                .get_providers(key.clone()),
                        );
                    }
                }
                kad::QueryResult::GetProviders(Err(e)) => {
                    if qid == Some(id) {
                        tracing::warn!(
                            "Find providers error {} for {} ({}), retrying...",
                            e,
                            cid,
                            id
                        );
                        qid = Some(
                            net.swarm
                                .behaviour_mut()
                                .ipfs_kad
                                .get_providers(key.clone()),
                        );
                    }
                }
                kad::QueryResult::GetProviders(Ok(kad::GetProvidersOk::FoundProviders {
                    providers,
                    ..
                })) if qid == Some(id) => {
                    for p in providers {
                        if p == *net.swarm.local_peer_id() {
                            continue;
                        }

                        if net.swarm.is_connected(&p) {
                            connecting.remove(&p);
                            connected.insert(p);
                        } else if !connecting.contains(&p) {
                            tracing::debug!("Dialing peer {p}");
                            if let Err(e) = net.swarm.dial(p) {
                                tracing::warn!("Failed to dial peer {p}: {e:?}")
                            } else {
                                connecting.insert(p);
                            }
                        }
                    }
                }
                res => tracing::debug!("Kad query result: {res:?}"),
            },
            ev => handle_shared(net, ev),
        }
    }

    Ok(())
}

pub async fn run(mut net: Net) -> color_eyre::Result<()> {
    let mut maintenance = tokio::time::interval(Duration::from_secs(30));
    maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            ev = net.swarm.select_next_some() => handle_shared(&mut net, ev),
            _ = maintenance.tick() => {
                if let Some(relays) = &mut net.relays {
                    relays.reserve(&mut net.swarm);
                }
                if let Some(key) = &net.provider_key {
                    net.swarm.behaviour_mut().ipfs_kad.get_providers(key.clone());
                }
            }
        }
    }
}
