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
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const SEARCH_RETRY: Duration = Duration::from_secs(5);

#[derive(Default)]
struct ProviderRetries(HashMap<libp2p::PeerId, (Instant, Duration)>);

impl ProviderRetries {
    fn ready(&self, peer: &libp2p::PeerId, now: Instant) -> bool {
        self.0
            .get(peer)
            .is_none_or(|(retry_at, _)| now >= *retry_at)
    }

    fn failed(&mut self, peer: libp2p::PeerId, now: Instant) -> Duration {
        let delay = self.0.get(&peer).map_or(SEARCH_RETRY, |(_, delay)| {
            (*delay * 2).min(Duration::from_secs(60))
        });
        self.0.insert(peer, (now + delay, delay));
        delay
    }
}

fn dialable_address(address: &libp2p::Multiaddr) -> bool {
    use libp2p::multiaddr::Protocol;
    !address.iter().any(|protocol| match protocol {
        Protocol::Ip6(ip) => ip.is_unicast_link_local(),
        Protocol::Ws(_)
        | Protocol::Wss(_)
        | Protocol::Tls
        | Protocol::WebTransport
        | Protocol::WebRTCDirect
        | Protocol::WebRTC => true,
        _ => false,
    })
}

fn handle_identify(net: &mut Net, peer_id: libp2p::PeerId, info: identify::Info) {
    if net.provider_key.is_some()
        && info
            .protocols
            .contains(&StreamProtocol::new(Behaviour::IPFS_KAD_PROTOCOL))
    {
        for addr in info
            .listen_addrs
            .iter()
            .filter(|addr| dialable_address(addr))
        {
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
        for addr in info
            .listen_addrs
            .iter()
            .filter(|addr| dialable_address(addr))
        {
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

fn find_providers(net: &mut Net, key: &kad::RecordKey) -> kad::QueryId {
    if let Ok(hash) = cid::multihash::Multihash::<64>::from_bytes(key.as_ref()) {
        tracing::info!(
            "Searching for providers for {}",
            Cid::new_v1(Net::BOOTSTRAP_CODEC, hash)
        );
    }
    net.swarm
        .behaviour_mut()
        .ipfs_kad
        .get_providers(key.clone())
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
                if !dialable_address(&addr) {
                    continue;
                }
                tracing::info!("Discovered {} on {}", id, addr);

                if net.provider_key.is_some() {
                    net.swarm
                        .behaviour_mut()
                        .ipfs_kad
                        .add_address(&id, addr.clone());
                }
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
                key,
                providers,
                ..
            })) if net.provider_key.as_ref() == Some(&key) => {
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
    let mut identified = HashSet::new();
    let mut rpws_peers = HashSet::new();
    let mut retries = ProviderRetries::default();
    let mut search_started = false;
    let mut next_search = Instant::now();
    let mut maintenance = tokio::time::interval(Duration::from_secs(1));
    maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        if !connected.is_empty() && connecting.is_empty() {
            break;
        }

        let event = tokio::select! {
            event = net.swarm.select_next_some() => event,
            _ = maintenance.tick() => {
                if search_started && qid.is_none() && Instant::now() >= next_search {
                    qid = Some(find_providers(net, &key));
                }
                if let Some(relays) = &mut net.relays { relays.reserve(&mut net.swarm); }
                continue;
            }
        };
        match event {
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                tracing::trace!("Connected to {}", peer_id);
                identified.insert(peer_id);
                let supports_rpws = info
                    .protocols
                    .contains(&StreamProtocol::new(Behaviour::RPWS_KAD_PROTOCOL));
                if supports_rpws {
                    rpws_peers.insert(peer_id);
                }
                if connecting.remove(&peer_id) && supports_rpws {
                    connected.insert(peer_id);
                }

                handle_identify(net, peer_id, info);
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
                connected.remove(&peer_id);
                identified.remove(&peer_id);
                rpws_peers.remove(&peer_id);
                if connecting.remove(&peer_id) {
                    retries.failed(peer_id, Instant::now());
                }
            }
            SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                if let Some(id) = peer_id
                    && connecting.remove(&id)
                {
                    let delay = retries.failed(id, Instant::now());
                    tracing::warn!("Provider {id} is unavailable; retrying in {delay:?}");
                    tracing::debug!("Failed to dial provider {id}: {error}");
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

                    if !search_started
                        && (step.count.get() as u32 <= b.num_remaining / 2 || step.last)
                    {
                        qid = Some(find_providers(net, &key));
                        search_started = true;
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
                        lid = None;
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
                        tracing::info!(
                            "Provider search for {cid} completed; retrying in {SEARCH_RETRY:?}"
                        );
                        qid = None;
                        next_search = Instant::now() + SEARCH_RETRY;
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
                        qid = None;
                        next_search = Instant::now() + SEARCH_RETRY;
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
                            if rpws_peers.contains(&p) {
                                connecting.remove(&p);
                                connected.insert(p);
                            } else if !identified.contains(&p) {
                                connecting.insert(p);
                            }
                        } else if !connecting.contains(&p) && retries.ready(&p, Instant::now()) {
                            tracing::info!("Found provider {p} for {cid}; dialing");
                            tracing::debug!("Dialing peer {p}");
                            if let Err(e) = net.swarm.dial(p) {
                                let delay = retries.failed(p, Instant::now());
                                tracing::warn!(
                                    "Provider {p} is unavailable; retrying in {delay:?}"
                                );
                                tracing::debug!("Failed to dial provider {p}: {e:?}");
                            } else {
                                connecting.insert(p);
                            }
                        }
                    }
                    if step.last {
                        qid = None;
                        next_search = Instant::now() + SEARCH_RETRY;
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
                if let Some(key) = net.provider_key.clone() {
                    find_providers(&mut net, &key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn offline_provider_retries_are_delayed_and_capped() {
        let peer = libp2p::PeerId::random();
        let mut retries = ProviderRetries::default();
        let mut now = Instant::now();
        for seconds in [5, 10, 20, 40, 60, 60] {
            assert!(retries.ready(&peer, now));
            let delay = retries.failed(peer, now);
            assert_eq!(delay, Duration::from_secs(seconds));
            assert!(!retries.ready(&peer, now + delay - Duration::from_millis(1)));
            now += delay;
            assert!(retries.ready(&peer, now));
        }
        assert!(retries.ready(&libp2p::PeerId::random(), now));
    }

    #[test]
    fn address_filter_preserves_lan_and_relay_routes() {
        for address in ["/ip6/fe80::1/tcp/7777", "/dns4/example.com/tcp/443/tls/ws"] {
            assert!(!dialable_address(&address.parse().unwrap()));
        }
        for address in [
            "/ip6/fd7a:115c:a1e0::9838:bf18/tcp/7777",
            "/ip4/10.0.0.1/tcp/7777",
            "/ip4/127.0.0.1/udp/7777/quic-v1",
        ] {
            assert!(dialable_address(&address.parse().unwrap()));
        }
        let relay = format!(
            "/ip4/38.146.27.201/tcp/4001/p2p/{}/p2p-circuit/p2p/{}",
            libp2p::PeerId::random(),
            libp2p::PeerId::random()
        );
        assert!(dialable_address(&relay.parse().unwrap()));
    }

    async fn net(directory: &tempfile::TempDir) -> Net {
        let mut config: toml::Value = toml::from_str(&Config::new().encode().unwrap()).unwrap();
        config["kademlia_store_path"] =
            toml::Value::String(directory.path().join("kad.redb").to_str().unwrap().into());
        config["listen_addrs"] =
            toml::Value::Array(vec![toml::Value::String("/ip4/127.0.0.1/tcp/0".into())]);
        Net::new(&Config::decode(toml::to_string(&config).unwrap()).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bootstrap_finds_remote_provider_before_cleanup() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let provider_dir = tempfile::tempdir().unwrap();
            let seeker_dir = tempfile::tempdir().unwrap();
            let mut provider = net(&provider_dir).await;
            let mut seeker = net(&seeker_dir).await;
            let address = loop {
                if let SwarmEvent::NewListenAddr { address, .. } = provider.swarm.select_next_some().await { break address; }
            };
            provider.swarm.add_external_address(address.clone());
            let peer = *provider.swarm.local_peer_id();
            let secret = b"provider-discovery-regression";
            let key = kad::RecordKey::new(&Code::Blake3_256.digest(secret).to_bytes());
            provider.swarm.behaviour_mut().ipfs_kad.start_providing(key).unwrap();
            seeker.swarm.behaviour_mut().ipfs_kad.add_address(&peer, address);
            seeker.swarm.behaviour_mut().ipfs_kad.bootstrap().unwrap();
            {
                let bootstrap = seeker.bootstrap(true, secret);
                tokio::pin!(bootstrap);
                loop {
                    tokio::select! {
                        result = &mut bootstrap => { result.unwrap(); break; },
                        event = provider.swarm.select_next_some() => handle_shared(&mut provider, event),
                    }
                }
            }
            assert!(seeker.swarm.is_connected(&peer));
            assert!(seeker.swarm.behaviour_mut().rpws_kad.kbuckets().any(|bucket| bucket.iter().any(|entry| entry.node.key.preimage() == &peer)));
            seeker.cleanup();
            assert!(seeker.provider_key.is_none());
            assert!(seeker.swarm.behaviour_mut().ipfs_kad.kbuckets().all(|bucket| bucket.is_empty()));
            assert!(seeker.swarm.is_connected(&peer));
        }).await.expect("provider lookup and connection timed out");
    }

    #[derive(libp2p::swarm::NetworkBehaviour)]
    struct RelayServer {
        relay: libp2p::relay::Behaviour,
        identify: identify::Behaviour,
    }

    #[tokio::test]
    async fn cleanup_keeps_relay_usable_for_inbound_connections() {
        use libp2p::{SwarmBuilder, multiaddr::Protocol, noise, relay, tcp, yamux};
        tokio::time::timeout(Duration::from_secs(20), async {
            let directory = tempfile::tempdir().unwrap();
            let mut client = net(&directory).await;
            client.relays = Some(super::super::relays::Relays::default());
            let mut server = SwarmBuilder::with_new_identity().with_tokio()
                .with_tcp(tcp::Config::default(), noise::Config::new, yamux::Config::default).unwrap()
                .with_behaviour(|key| RelayServer {
                    relay: relay::Behaviour::new(key.public().to_peer_id(), relay::Config::default()),
                    identify: identify::Behaviour::new(identify::Config::new("/test/1".into(), key.public())),
                }).unwrap()
                .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60))).build();
            server.listen_on("/ip4/127.0.0.1/tcp/0".parse().unwrap()).unwrap();
            let address = loop {
                if let SwarmEvent::NewListenAddr { address, .. } = server.select_next_some().await { break address; }
            };
            server.add_external_address(address.clone());
            let relay_peer = *server.local_peer_id();
            client.swarm.dial(address.with_p2p(relay_peer).unwrap()).unwrap();
            let circuit = loop {
                tokio::select! {
                    _ = server.select_next_some() => {},
                    event = client.swarm.select_next_some() => {
                        let confirmed = match &event {
                            SwarmEvent::ExternalAddrConfirmed { address } if address.iter().any(|p| matches!(p, Protocol::P2pCircuit)) => Some(address.clone()),
                            _ => None,
                        };
                        handle_shared(&mut client, event);
                        if let Some(address) = confirmed { break address; }
                    }
                }
            };
            client.cleanup();
            assert!(client.swarm.is_connected(&relay_peer));
            let dialer_directory = tempfile::tempdir().unwrap();
            let mut dialer = net(&dialer_directory).await;
            dialer.swarm.dial(circuit.with_p2p(*client.swarm.local_peer_id()).unwrap()).unwrap();
            loop {
                tokio::select! {
                    _ = server.select_next_some() => {},
                    event = client.swarm.select_next_some() => {
                        if let SwarmEvent::ConnectionEstablished { peer_id, endpoint, .. } = &event
                            && peer_id == dialer.swarm.local_peer_id() {
                            assert!(endpoint.is_relayed());
                            break;
                        }
                        handle_shared(&mut client, event);
                    },
                    event = dialer.swarm.select_next_some() => handle_shared(&mut dialer, event),
                }
            }
        }).await.expect("relay reservation or inbound connection after cleanup timed out");
    }
}
