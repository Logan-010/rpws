use super::{
    Net,
    types::{Behaviour, BehaviourEvent},
};
use cid::Cid;
use libp2p::{StreamProtocol, futures::StreamExt, identify, kad, mdns, swarm::SwarmEvent};
use multihash_codetable::{Code, MultihashDigest};
use std::collections::HashSet;

fn handle_shared(net: &mut Net, event: SwarmEvent<BehaviourEvent>) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => tracing::info!("Listening on {}", address),
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

            if protocols.contains(&StreamProtocol::new(Behaviour::IPFS_KAD_PROTOCOL)) {
                for addr in listen_addrs.clone() {
                    net.swarm
                        .behaviour_mut()
                        .ipfs_kad
                        .add_address(&peer_id, addr);
                }
            }
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
        ev => tracing::trace!("Swarm event: {ev:?}"),
    }
}

pub async fn bootstrap<B: AsRef<[u8]>>(net: &mut Net, of: B) -> color_eyre::Result<()> {
    let mh = Code::Blake3_256.digest(of.as_ref());
    let cid = Cid::new_v1(Net::BOOTSTRAP_CODEC, mh);
    let key = kad::RecordKey::new(&cid.hash().to_bytes());

    tracing::info!("Bootstrapping for {}", cid);

    let mut qid = None;
    let mut connected = HashSet::new();
    let mut connecting = HashSet::new();

    loop {
        if !connected.is_empty() && connecting.is_empty() {
            break;
        }

        match net.swarm.select_next_some().await {
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

                if connecting.remove(&peer_id) {
                    connected.insert(peer_id);
                }

                if protocols.contains(&StreamProtocol::new(Behaviour::IPFS_KAD_PROTOCOL)) {
                    for addr in listen_addrs.clone() {
                        net.swarm
                            .behaviour_mut()
                            .ipfs_kad
                            .add_address(&peer_id, addr);
                    }
                }
                if protocols.contains(&StreamProtocol::new(Behaviour::RPWS_KAD_PROTOCOL)) {
                    for addr in listen_addrs {
                        net.swarm
                            .behaviour_mut()
                            .rpws_kad
                            .add_address(&peer_id, addr);
                    }
                }
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
                    }
                }
                kad::QueryResult::Bootstrap(Err(e)) => {
                    tracing::warn!("IPFS bootstrap error: {e:?}")
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
