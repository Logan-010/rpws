use super::Net;
use libp2p::{
    PeerId, kad,
    request_response::{Event, Message},
};

pub fn request(net: &mut Net, peer: PeerId) {
    if net.sync_pending.values().any(|pending| *pending == peer) {
        return;
    }
    let id = net
        .swarm
        .behaviour_mut()
        .sync
        .send_request(&peer, shared::sync::Request::new(&net.secret));
    net.sync_pending.insert(id, peer);
}

pub(super) fn snapshot(net: &mut Net) -> color_eyre::Result<Vec<shared::sync::Entry>> {
    let records = net
        .swarm
        .behaviour_mut()
        .rpws_kad
        .store_mut()
        .try_records()?;
    Ok(shared::sync::entries(
        records.into_iter().map(|r| (r.key.to_vec(), r.value)),
        &net.secret,
    ))
}

pub fn handle(net: &mut Net, event: Event<shared::sync::Request, shared::sync::Response>) {
    match event {
        Event::Message {
            message: Message::Request {
                request, channel, ..
            },
            ..
        } => {
            let matched = request.vault == shared::sync::Request::new(&net.secret).vault;
            let entries = if matched {
                snapshot(net)
            } else {
                Ok(Vec::new())
            };
            match entries {
                Ok(entries) => {
                    let _ = net
                        .swarm
                        .behaviour_mut()
                        .sync
                        .send_response(channel, shared::sync::Response { matched, entries });
                }
                Err(error) => tracing::warn!("Cannot read vault for sync: {error}"),
            }
        }
        Event::Message {
            peer,
            message:
                Message::Response {
                    request_id,
                    response,
                },
            ..
        } => {
            net.sync_pending.remove(&request_id);
            if !response.matched {
                tracing::debug!("Peer {peer} uses another vault secret");
                return;
            }
            let result = merge(net, response.entries);
            if let Err(error) = result {
                tracing::warn!("Cannot cache synced vault: {error}");
            }
        }
        Event::OutboundFailure {
            request_id, error, ..
        } => {
            net.sync_pending.remove(&request_id);
            tracing::debug!("Vault sync failed: {error}");
        }
        _ => {}
    }
}

fn merge(net: &mut Net, entries: Vec<shared::sync::Entry>) -> color_eyre::Result<()> {
    let current = snapshot(net)?;
    let mut timestamps: std::collections::HashMap<_, _> = current
        .iter()
        .filter_map(|e| e.decode(&net.secret))
        .map(|(name, p)| (name, p.timestamp))
        .collect();
    for entry in entries {
        let Some((name, payload)) = entry.decode(&net.secret) else {
            continue;
        };
        if timestamps
            .get(&name)
            .is_some_and(|time| *time >= payload.timestamp)
        {
            continue;
        }
        for (key, value) in entry.records(&net.secret).expect("Verified entry") {
            net.swarm
                .behaviour_mut()
                .rpws_kad
                .store_mut()
                .try_put(kad::Record::new(key, value))?;
        }
        timestamps.insert(name, payload.timestamp);
    }
    Ok(())
}
