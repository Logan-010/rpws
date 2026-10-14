use super::{
    Net, records,
    requests::Requests,
    types::{Behaviour, BehaviourEvent},
};
use cid::Cid;
use libp2p::{
    Multiaddr, StreamProtocol,
    futures::StreamExt,
    identify,
    kad::{self, store::RecordStore},
    mdns,
    multiaddr::Protocol,
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

fn dialable_address(address: &Multiaddr) -> bool {
    address.iter().all(|protocol| match protocol {
        Protocol::Ip4(ip) => !ip.is_link_local(),
        Protocol::Ip6(ip) => !ip.is_unicast_link_local(),
        Protocol::Ws(_) | Protocol::Wss(_) | Protocol::Tls => true,
        Protocol::QuicV1 => true,
        Protocol::Tcp(_) | Protocol::Udp(_) => true,
        Protocol::P2p(_) => true,
        Protocol::P2pCircuit => true,
        Protocol::Dns4(_) | Protocol::Dns6(_) | Protocol::Dnsaddr(_) | Protocol::Dns(_) => true,
        _ => false,
    }) && address.iter().any(|protocol| {
        matches!(
            protocol,
            Protocol::Tcp(_) | Protocol::QuicV1 | Protocol::Dnsaddr(_)
        )
    })
}

fn handle_identify(net: &mut Net, peer_id: libp2p::PeerId, info: identify::Info) {
    if info
        .protocols
        .contains(&StreamProtocol::new(shared::sync::PROTOCOL))
    {
        net.sync_peers.insert(peer_id);
        super::sync::request(net, peer_id);
    }
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
        SwarmEvent::Behaviour(BehaviourEvent::Sync(event)) => super::sync::handle(net, event),
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
        SwarmEvent::NewListenAddr { mut address, .. } => {
            let p = address.pop();

            if let Some(Protocol::P2p(_)) = p {
            } else if let Some(pr) = p {
                address = address.with(pr);
            }

            tracing::info!(
                "Listening on {}",
                address.with(Protocol::P2p(*net.swarm.local_peer_id()))
            );
        }
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
                    record: Some(mut r),
                    ..
                },
        })) => {
            let store = net.swarm.behaviour_mut().rpws_kad.store_mut();
            r.expires = None;
            match records::accepts(store, &r, &net.secret) {
                Ok(true) => {
                    if let Err(e) = store.try_put(r) {
                        tracing::warn!("Failed to put record: {e:?}");
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::warn!("Failed to verify record version: {e:?}"),
            }
        }
        ev => tracing::trace!("Swarm event: {ev:?}"),
    }
}

pub async fn run<B: AsRef<[u8]>>(net: &mut Net, of: B) -> color_eyre::Result<()> {
    let mh = Code::Blake3_256.digest(of.as_ref());
    let cid = Cid::new_v1(Net::BOOTSTRAP_CODEC, mh);
    let key = kad::RecordKey::new(&cid.hash().to_bytes());
    net.provider_key = Some(key.clone());

    // Browser peers discover the same rendezvous key in the RPWS DHT.
    if let Err(error) = net
        .swarm
        .behaviour_mut()
        .rpws_kad
        .start_providing(key.clone())
    {
        tracing::warn!("Failed to advertise RPWS vault provider: {error}");
    }

    tracing::info!("Running network for {}", cid);

    let mut qid = None;
    let mut requests = Requests::default();
    let mut connecting = HashSet::new();
    let mut retries = ProviderRetries::default();
    let mut search_started = false;
    let mut next_search = Instant::now();
    let mut next_sync = Instant::now();
    let mut maintenance = tokio::time::interval(Duration::from_secs(1));
    maintenance.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        let event = tokio::select! {
            event = net.swarm.select_next_some() => event,
            Some(request) = net.requests.recv() => {
                requests.handle(net, request);
                continue;
            }
            _ = maintenance.tick() => {
                requests.maintain(net);
                if Instant::now() >= next_sync {
                    let peers: Vec<_> = net.sync_peers.iter().copied().filter(|peer| net.swarm.is_connected(peer)).collect();
                    for peer in peers { super::sync::request(net, peer); }
                    next_sync = Instant::now() + SEARCH_RETRY;
                }
                if search_started && qid.is_none() && Instant::now() >= next_search {
                    qid = Some(find_providers(net, &key));
                }
                if let Some(relays) = &mut net.relays { relays.reserve(&mut net.swarm); }
                continue;
            }
        };
        match event {
            SwarmEvent::Behaviour(BehaviourEvent::RpwsKad(
                kad::Event::OutboundQueryProgressed {
                    id, result, step, ..
                },
            )) => requests.progressed(net, id, result, step.last),
            SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                peer_id,
                info,
                ..
            })) => {
                connecting.remove(&peer_id);
                handle_identify(net, peer_id, info);
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                num_established: 0,
                ..
            } => {
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
                        refresh_provider(net);
                    }
                }
                kad::QueryResult::Bootstrap(Err(e)) => {
                    tracing::warn!("IPFS bootstrap error: {e:?}")
                }
                kad::QueryResult::StartProviding(Ok(_)) => {
                    tracing::info!("Providing {cid}");
                }
                kad::QueryResult::StartProviding(Err(e)) => {
                    tracing::warn!("Failed to provide {cid}: {e:?}, retrying...");
                    refresh_provider(net);
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
                        if !net.swarm.is_connected(&p)
                            && !connecting.contains(&p)
                            && retries.ready(&p, Instant::now())
                        {
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

    async fn net(directory: &tempfile::TempDir) -> Net {
        let mut config: toml::Value = toml::from_str(&Config::new().encode().unwrap()).unwrap();
        config["kademlia_store_path"] =
            toml::Value::String(directory.path().join("kad.redb").to_str().unwrap().into());
        config["listen_addrs"] =
            toml::Value::Array(vec![toml::Value::String("/ip4/127.0.0.1/tcp/0".into())]);
        config["bootstrap_addrs"] = toml::Value::Array(Vec::new());
        let mut net = Net::new(&Config::decode(toml::to_string(&config).unwrap()).unwrap())
            .await
            .unwrap();
        // Keep local regression swarms from discovering unrelated live daemons
        // and following their public DHT routes.
        net.swarm.behaviour_mut().disable_mdns();
        net
    }

    fn payload(password: &str) -> shared::RecordPayload {
        shared::RecordPayload {
            url: "https://example.com".into(),
            username: "alice".into(),
            password: password.into(),
            timestamp: 1,
            extra: String::new(),
        }
    }

    #[tokio::test]
    async fn deletion_survives_stale_sync_and_database_restart() {
        use super::super::{
            requests::{Request, Requests},
            sync,
        };
        let directory = tempfile::tempdir().unwrap();
        let mut peer = net(&directory).await;
        let secret = peer.secret;
        let name = "deleted account";
        let stale = shared::sync::Entry::new(name, payload("old password"), &secret);
        sync::merge(&mut peer, vec![stale.clone()]).unwrap();
        let mut requests = Requests::default();
        let (reply, response) = tokio::sync::oneshot::channel();
        requests.handle(
            &mut peer,
            Request::Delete {
                name: name.into(),
                reply,
            },
        );
        response.await.unwrap().unwrap();
        let deletion = sync::snapshot(&mut peer).unwrap();
        assert!(deletion.iter().all(|entry| entry.decode(&secret).is_none()));
        sync::merge(&mut peer, vec![stale.clone()]).unwrap();
        let mapping = kad::RecordKey::new(&shared::Provider::key(name));
        let state = super::super::records::state(
            peer.swarm.behaviour_mut().rpws_kad.store_mut(),
            &mapping,
            &secret,
        )
        .unwrap()
        .unwrap();
        assert!(state.version().1);
        let timestamp = state.version().0;
        let store = peer.swarm.behaviour_mut().rpws_kad.store_mut();
        assert!(
            !super::super::records::accepts(
                store,
                &kad::Record::new(shared::Provider::key(name), stale.mapping.clone()),
                &secret
            )
            .unwrap()
        );
        drop(peer);
        let mut peer = net(&directory).await;
        peer.secret = secret;
        sync::merge(&mut peer, vec![stale]).unwrap();
        let (reply, response) = tokio::sync::oneshot::channel();
        requests.handle(
            &mut peer,
            Request::Get {
                name: name.into(),
                reply,
            },
        );
        assert!(response.await.unwrap().unwrap().is_none());
        let (reply, response) = tokio::sync::oneshot::channel();
        requests.handle(
            &mut peer,
            Request::Put {
                name: name.into(),
                payload: payload("recreated password"),
                reply,
            },
        );
        response.await.unwrap().unwrap();
        let current = sync::snapshot(&mut peer).unwrap();
        let (_, recreated) = current
            .iter()
            .find_map(|entry| entry.decode(&secret))
            .unwrap();
        assert_eq!(recreated.password, "recreated password");
        assert!(recreated.timestamp > timestamp);
        sync::merge(&mut peer, deletion).unwrap();
        assert!(
            sync::snapshot(&mut peer)
                .unwrap()
                .iter()
                .any(|entry| entry.decode(&secret).is_some())
        );
    }

    #[tokio::test]
    async fn api_put_get_list_roundtrip_locally_and_from_remote_peer() {
        use super::super::api::{self, NetApi};
        use tokio::net::TcpListener;

        tokio::time::timeout(Duration::from_secs(20), async {
            let provider_dir = tempfile::tempdir().unwrap();
            let seeker_dir = tempfile::tempdir().unwrap();
            let mut provider = net(&provider_dir).await;
            let mut seeker = net(&seeker_dir).await;
            seeker.secret = provider.secret;
            let invalid_key = shared::Record::new(payload("wrong-key"), &provider.secret);
            let wrong_secret = shared::Record::new(payload("wrong-secret"), &[0; 32]);
            let mapping = shared::Provider::new("orphan", &invalid_key, &provider.secret);
            let store = provider.swarm.behaviour_mut().rpws_kad.store_mut();
            for record in [
                kad::Record::new(b"malformed".to_vec(), vec![0xff]),
                kad::Record::new(b"wrong-key".to_vec(), invalid_key.encode()),
                kad::Record::new(wrong_secret.hash().to_vec(), wrong_secret.encode()),
                kad::Record::new(shared::Provider::key("orphan"), mapping.encode()),
            ] {
                store.try_put(record).unwrap();
            }
            let address = loop {
                if let SwarmEvent::NewListenAddr { address, .. } =
                    provider.swarm.select_next_some().await
                {
                    break address;
                }
            };
            let name = "example / account ? # ü";
            let provider_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let seeker_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let provider_api = NetApi::new(format!(
                "http://{}",
                provider_listener.local_addr().unwrap()
            ));
            let seeker_api =
                NetApi::new(format!("http://{}", seeker_listener.local_addr().unwrap()));
            let provider_router = api::build(provider.api());
            let seeker_router = api::build(seeker.api());
            let provider_server = async {
                axum::serve(provider_listener, provider_router)
                    .await
                    .unwrap()
            };
            let seeker_server =
                async { axum::serve(seeker_listener, seeker_router).await.unwrap() };
            tokio::pin!(provider_server, seeker_server);

            tokio::select! {
                result = provider.run(true, b"api-test") => panic!("provider stopped: {result:?}"),
                _ = &mut provider_server => panic!("API stopped"),
                _ = async {
                    provider_api.connect().await.unwrap();
                    assert!(provider_api.list().await.unwrap().is_empty());
                    assert!(provider_api.get("missing").await.unwrap().is_none());
                    assert!(provider_api.get("").await.is_err());
                    assert!(provider_api.put("", payload("invalid")).await.is_err());
                    provider_api.put(name, payload("first")).await.unwrap();
                    assert_eq!(provider_api.get(name).await.unwrap(), Some(payload("first")));
                    assert_eq!(provider_api.list().await.unwrap(), HashMap::from([(name.to_owned(), payload("first"))]));
                    provider_api.put(name, payload("updated")).await.unwrap();
                    assert_eq!(provider_api.get(name).await.unwrap(), Some(shared::RecordPayload { timestamp: 2, ..payload("updated") }));
                    provider_api.put("second", payload("second")).await.unwrap();
                    let passwords = provider_api.list().await.unwrap();
                    assert_eq!(passwords, HashMap::from([
                        (name.to_owned(), shared::RecordPayload { timestamp: 2, ..payload("updated") }),
                        ("second".to_owned(), payload("second")),
                    ]));
                    assert!(provider_api.delete("").await.is_err());
                    provider_api.delete("second").await.unwrap();
                    assert_eq!(provider_api.list().await.unwrap(), HashMap::from([(name.to_owned(), shared::RecordPayload { timestamp: 2, ..payload("updated") })]));
                    provider_api.delete("second").await.unwrap();
                } => {},
            }

            let key = kad::RecordKey::new(&shared::Provider::key(name));
            let removed_key = kad::RecordKey::new(&shared::Provider::key("second"));
            let store = provider.swarm.behaviour_mut().rpws_kad.store_mut();
            assert!(super::super::records::state(store, &removed_key, &provider.secret).unwrap().unwrap().version().1);
            assert!(store.try_provided().unwrap().iter().all(|record| record.key != removed_key));
            let index = provider
                .swarm
                .behaviour_mut()
                .rpws_kad
                .store_mut()
                .try_get(&key)
                .unwrap()
                .unwrap();
            let mapping = shared::Provider::decode(index.value).unwrap();
            let record = provider
                .swarm
                .behaviour_mut()
                .rpws_kad
                .store_mut()
                .try_get(&kad::RecordKey::new(mapping.hash()))
                .unwrap()
                .unwrap();
            assert!(
                shared::Record::decode(record.value)
                    .unwrap()
                    .verify(&provider.secret)
            );
            assert!(
                seeker
                    .swarm
                    .behaviour_mut()
                    .rpws_kad
                    .store_mut()
                    .try_get(&key)
                    .unwrap()
                    .is_none()
            );

            let peer = *provider.swarm.local_peer_id();
            seeker
                .swarm
                .behaviour_mut()
                .rpws_kad
                .add_address(&peer, address);
            tokio::select! {
                result = provider.run(true, b"api-test") => panic!("provider stopped: {result:?}"),
                result = seeker.run(true, b"api-test") => panic!("seeker stopped: {result:?}"),
                _ = &mut provider_server => panic!("provider API stopped"),
                _ = &mut seeker_server => panic!("seeker API stopped"),
                _ = async {
                    assert_eq!(seeker_api.get(name).await.unwrap(), Some(shared::RecordPayload { timestamp: 2, ..payload("updated") }));
                    assert!(seeker_api.get("missing").await.unwrap().is_none());
                } => {},
            }
        })
        .await
        .expect("API put/get/list timed out");
    }

    #[tokio::test]
    async fn run_keeps_discovering_providers_and_preserves_kad() {
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
            provider.swarm.behaviour_mut().ipfs_kad.start_providing(key.clone()).unwrap();
            seeker.swarm.behaviour_mut().ipfs_kad.add_address(&peer, address);
            seeker.swarm.behaviour_mut().ipfs_kad.bootstrap().unwrap();
            {
                let run = seeker.run(true, secret);
                tokio::pin!(run);
                let mut searches = 0;
                loop {
                    tokio::select! {
                        result = &mut run => panic!("network loop stopped: {result:?}"),
                        event = provider.swarm.select_next_some() => {
                            if matches!(&event, SwarmEvent::Behaviour(BehaviourEvent::IpfsKad(kad::Event::InboundRequest {
                                request: kad::InboundRequest::GetProvider { .. },
                            }))) {
                                searches += 1;
                            }
                            handle_shared(&mut provider, event);
                            if searches >= 2 { break; }
                        },
                    }
                }
            }
            assert!(seeker.swarm.is_connected(&peer));
            assert!(seeker.swarm.behaviour_mut().rpws_kad.kbuckets().any(|bucket| bucket.iter().any(|entry| entry.node.key.preimage() == &peer)));
            assert_eq!(seeker.provider_key, Some(key));
            assert!(seeker.swarm.behaviour_mut().ipfs_kad.kbuckets().any(|bucket| bucket.iter().any(|entry| entry.node.key.preimage() == &peer)));
        }).await.expect("provider lookup and connection timed out");
    }

    #[derive(libp2p::swarm::NetworkBehaviour)]
    struct RelayServer {
        relay: libp2p::relay::Behaviour,
        identify: identify::Behaviour,
    }

    #[tokio::test]
    #[ignore = "requires Node.js and npm run build:wasm in firefox-extension"]
    async fn browser_sync_enumerates_daemon_passwords_over_websocket() {
        let directory = tempfile::tempdir().unwrap();
        let mut config: toml::Value = toml::from_str(&Config::new().encode().unwrap()).unwrap();
        config["kademlia_store_path"] =
            toml::Value::String(directory.path().join("kad.redb").to_str().unwrap().into());
        config["listen_addrs"] =
            toml::Value::Array(vec![toml::Value::String("/ip4/127.0.0.1/tcp/0/ws".into())]);
        config["bootstrap_addrs"] = toml::Value::Array(Vec::new());
        use base64::Engine;
        config["secret"] =
            toml::Value::String(base64::engine::general_purpose::STANDARD.encode([7; 32]));
        let mut provider = Net::new(&Config::decode(toml::to_string(&config).unwrap()).unwrap())
            .await
            .unwrap();
        provider.swarm.behaviour_mut().disable_mdns();
        for (name, password) in [
            ("daemon account", "daemon-password"),
            ("second account", "second-password"),
        ] {
            for (key, value) in shared::sync::Entry::new(name, payload(password), &provider.secret)
                .records(&provider.secret)
                .unwrap()
            {
                provider
                    .swarm
                    .behaviour_mut()
                    .rpws_kad
                    .store_mut()
                    .try_put(kad::Record::new(key, value))
                    .unwrap();
            }
        }
        let address = loop {
            if let SwarmEvent::NewListenAddr { address, .. } =
                provider.swarm.select_next_some().await
            {
                break address;
            }
        }
        .with(Protocol::P2p(*provider.swarm.local_peer_id()));
        let script = std::env::var_os("RPWS_BROWSER_SYNC_SCRIPT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../firefox-extension/tests/net-sync.mjs")
            });
        // Reproduce the previous browser Save's postcard layout so the real
        // WASM loader also proves that direct-key caches migrate without loss.
        #[derive(serde::Serialize)]
        struct LegacyRecord {
            key: Vec<u8>,
            value: Vec<u8>,
            publisher: Option<String>,
            expires_in_ms: Option<u64>,
        }
        #[derive(serde::Serialize)]
        struct LegacyConfig {
            max_records: usize,
            max_value_bytes: usize,
            max_providers_per_key: usize,
            max_provided_keys: usize,
        }
        #[derive(serde::Serialize)]
        struct LegacyStore {
            local_id: String,
            config: LegacyConfig,
            records: Vec<LegacyRecord>,
            providers: Vec<()>,
        }
        #[derive(serde::Serialize)]
        struct LegacySave {
            identity: Vec<u8>,
            secret: [u8; 32],
            bootstrap_addrs: Vec<()>,
            store: Option<LegacyStore>,
        }
        let identity = libp2p::identity::Keypair::generate_ed25519();
        let entry =
            shared::sync::Entry::new("legacy account", payload("legacy-password"), &[7; 32]);
        let legacy = LegacySave {
            identity: identity.to_protobuf_encoding().unwrap(),
            secret: [7; 32],
            bootstrap_addrs: Vec::new(),
            store: Some(LegacyStore {
                local_id: identity.public().to_peer_id().to_string(),
                config: LegacyConfig {
                    max_records: 2,
                    max_value_bytes: 65 * 1024,
                    max_providers_per_key: 20,
                    max_provided_keys: 1024,
                },
                records: entry
                    .records(&[7; 32])
                    .unwrap()
                    .into_iter()
                    .map(|(key, value)| LegacyRecord {
                        key,
                        value,
                        publisher: None,
                        expires_in_ms: None,
                    })
                    .collect(),
                providers: Vec::new(),
            }),
        };
        let legacy_path = directory.path().join("legacy.rpws");
        std::fs::write(&legacy_path, postcard::to_stdvec(&legacy).unwrap()).unwrap();
        let mut child = std::process::Command::new("node")
            .arg(script)
            .arg(address.to_string())
            .arg(legacy_path)
            .spawn()
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(45), async {
            tokio::select! {
                result = provider.run(true, [7; 32]) => panic!("provider stopped: {result:?}"),
                status = async {
                    loop {
                        if let Some(status) = child.try_wait().unwrap() { break status; }
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                } => status,
            }
        })
        .await;
        if result.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(result.expect("Browser sync timed out").success());
        let entries = super::super::sync::snapshot(&mut provider).unwrap();
        assert!(
            entries
                .iter()
                .filter_map(|entry| entry.decode(&provider.secret))
                .any(|(name, payload)| name == "browser account"
                    && payload.password == "browser-password")
        );
        assert!(
            entries
                .iter()
                .filter_map(|entry| entry.decode_state(&provider.secret))
                .any(|(name, state)| name == "daemon account" && state.version().1)
        );
        assert!(
            entries
                .iter()
                .filter_map(|entry| entry.decode(&provider.secret))
                .any(|(name, payload)| name == "second account"
                    && payload.password == "recreated-password")
        );
    }

    #[tokio::test]
    async fn relay_reservation_allows_inbound_connections() {
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
        }).await.expect("relay reservation or inbound connection timed out");
    }
}
