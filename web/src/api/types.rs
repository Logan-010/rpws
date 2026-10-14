use crate::{
    api::{requests::Request, store::SerializableMemoryStore, swarm, task},
    save::Save,
};
use color_eyre::eyre::{bail, eyre};
use libp2p::futures::{
    FutureExt,
    channel::{mpsc, oneshot},
};
use std::collections::HashMap;

pub struct Api {
    sender: mpsc::UnboundedSender<Request>,
}

impl Api {
    pub async fn new(mut save: Save) -> color_eyre::Result<Self> {
        // Earlier browser saves encrypted records directly with the seed.
        // Rewrap these cached records using the daemon's derived key, retaining
        // the seed and save format so importing/copying a secret stays stable.
        if let Some(store) = &mut save.store {
            use libp2p::kad::store::RecordStore;
            let legacy = shared::sync::entries(
                store.records().map(|r| (r.key.to_vec(), r.value.clone())),
                &save.secret,
            );
            let key = shared::derive_key(save.secret);
            for entry in legacy {
                let (name, payload) = entry.decode(&save.secret).expect("Verified legacy record");
                let old =
                    shared::Provider::decode(&entry.mapping).expect("Verified legacy mapping");
                store.remove(&libp2p::kad::RecordKey::new(old.hash()));
                for (record_key, value) in shared::sync::Entry::new(&name, payload, &key)
                    .records(&key)
                    .unwrap()
                {
                    store.put(libp2p::kad::Record::new(record_key, value))?;
                }
            }
        }
        let swarm = swarm::new_swarm(save.clone()).await?;
        let (sender, receiver) = mpsc::unbounded();

        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = task::task(swarm, save, receiver).await {
                tracing::error!("Task exited: {e:?}");
            }
        });

        Ok(Self { sender })
    }

    async fn request<T>(
        &self,
        request: Request,
        response: oneshot::Receiver<color_eyre::Result<T>>,
    ) -> color_eyre::Result<T> {
        self.sender
            .unbounded_send(request)
            .map_err(|_| eyre!("Network unavailable"))?;
        let response = response.fuse();
        let timeout = gloo_timers::future::TimeoutFuture::new(180_000).fuse();
        libp2p::futures::pin_mut!(response, timeout);
        libp2p::futures::select! {
            result = response => result.map_err(|_| eyre!("Network unavailable"))?,
            _ = timeout => Err(eyre!("Network request timed out")),
        }
    }

    pub async fn connect(&self) -> color_eyre::Result<()> {
        let (reply, response) = oneshot::channel();
        self.request(Request::Connect { reply }, response).await
    }

    pub async fn sync(&self) -> color_eyre::Result<u32> {
        let (reply, response) = oneshot::channel();
        self.request(Request::Sync { reply }, response).await
    }

    pub async fn get<N: AsRef<str>>(
        &self,
        name: N,
    ) -> color_eyre::Result<Option<shared::RecordPayload>> {
        let name = Self::name(name)?;
        let (reply, response) = oneshot::channel();
        self.request(Request::Get { name, reply }, response).await
    }

    pub async fn put<N: AsRef<str>>(
        &self,
        name: N,
        payload: shared::RecordPayload,
    ) -> color_eyre::Result<()> {
        let name = Self::name(name)?;
        let (reply, response) = oneshot::channel();
        self.request(
            Request::Put {
                name,
                payload,
                reply,
            },
            response,
        )
        .await
    }

    pub async fn list(&self) -> color_eyre::Result<HashMap<String, shared::RecordPayload>> {
        let (reply, response) = oneshot::channel();
        self.request(Request::List { reply }, response).await
    }

    pub async fn delete<N: AsRef<str>>(&self, name: N) -> color_eyre::Result<()> {
        let name = Self::name(name)?;
        let (reply, response) = oneshot::channel();
        self.request(Request::Delete { name, reply }, response)
            .await
    }

    pub async fn kad_snapshot(&self) -> color_eyre::Result<SerializableMemoryStore> {
        let (reply, response) = oneshot::channel();
        self.request(Request::KadSnapshot { reply }, response).await
    }

    fn name<N: AsRef<str>>(name: N) -> color_eyre::Result<String> {
        if name.as_ref().is_empty() {
            bail!("Name must not be empty");
        }
        Ok(name.as_ref().to_owned())
    }
}
