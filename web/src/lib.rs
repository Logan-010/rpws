mod api;
mod save;

use api::Api;
use libp2p::Multiaddr;
use save::Save;
use std::{cell::RefCell, rc::Rc, sync::Once};
use wasm_bindgen::prelude::*;

thread_local! {
    static API: RefCell<Option<Rc<Api>>> = const { RefCell::new(None) };
}

fn active_api() -> Result<Rc<Api>, JsValue> {
    API.with(|s| {
        s.borrow()
            .clone()
            .ok_or_else(|| JsValue::from_str("API not initialized"))
    })
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

#[wasm_bindgen]
pub fn new_save() -> Result<Vec<u8>, JsValue> {
    Save::new()
        .encode()
        .map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Create a fresh peer identity using an existing vault secret.
#[wasm_bindgen]
pub fn new_save_with_secret(secret: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let secret: [u8; 32] = secret
        .try_into()
        .map_err(|_| JsValue::from_str("Secret must be exactly 32 bytes"))?;
    let mut save = Save::new();
    save.secret = secret;
    save.encode().map_err(js_error)
}

#[wasm_bindgen]
pub fn view_secret(save_bytes: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let save = Save::decode(&save_bytes).map_err(js_error)?;
    Ok(save.secret.to_vec())
}

#[wasm_bindgen]
pub fn view_bootstrap_addrs(save_bytes: Vec<u8>) -> Result<Vec<String>, JsValue> {
    let save = Save::decode(&save_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;

    Ok(save
        .bootstrap_addrs
        .into_iter()
        .map(|s| s.to_string())
        .collect::<Vec<String>>())
}

#[wasm_bindgen]
pub fn set_bootstrap_addrs(save_bytes: Vec<u8>, addrs: Vec<String>) -> Result<Vec<u8>, JsValue> {
    let mut save = Save::decode(&save_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;

    save.bootstrap_addrs = addrs
        .into_iter()
        .map(|s| {
            s.parse::<Multiaddr>()
                .map_err(|e| JsValue::from_str(&e.to_string()))
        })
        .collect::<Result<Vec<Multiaddr>, JsValue>>()?;

    save.encode().map_err(|e| JsValue::from_str(&e.to_string()))
}

#[wasm_bindgen]
pub async fn init(save_bytes: Vec<u8>) -> Result<(), JsValue> {
    static TRACING: Once = Once::new();
    TRACING.call_once(|| {
        tracing_wasm::set_as_global_default_with_config(
            tracing_wasm::WASMLayerConfigBuilder::new()
                .set_max_level(tracing::Level::INFO)
                .build(),
        );
    });

    let save = Save::decode(&save_bytes).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let api = Api::new(save)
        .await
        .map_err(|e| JsValue::from_str(&e.to_string()))?;

    API.with(|s| *s.borrow_mut() = Some(Rc::new(api)));

    Ok(())
}

#[wasm_bindgen]
pub fn unload() {
    // Dropping the API closes the request channel, allowing its swarm task to
    // exit and release peer connections and timers.
    API.with(|s| *s.borrow_mut() = None);
}

#[wasm_bindgen]
pub async fn connect() -> Result<(), JsValue> {
    active_api()?.connect().await.map_err(js_error)
}

#[wasm_bindgen]
pub async fn sync() -> Result<u32, JsValue> {
    active_api()?.sync().await.map_err(js_error)
}

#[wasm_bindgen]
pub async fn get(name: String) -> Result<Option<Vec<u8>>, JsValue> {
    let payload = active_api()?.get(name).await.map_err(js_error)?;
    payload
        .map(|payload| postcard::to_stdvec(&payload))
        .transpose()
        .map_err(js_error)
}

#[wasm_bindgen]
pub async fn put(name: String, payload_bytes: Vec<u8>) -> Result<(), JsValue> {
    let api = active_api()?;
    let payload: shared::RecordPayload = postcard::from_bytes(&payload_bytes).map_err(js_error)?;
    api.put(name, payload).await.map_err(js_error)
}

#[wasm_bindgen]
pub async fn list() -> Result<Vec<u8>, JsValue> {
    let payloads = active_api()?.list().await.map_err(js_error)?;
    postcard::to_stdvec(&payloads).map_err(js_error)
}

/// Browser-friendly equivalents of the postcard-based password API.
#[wasm_bindgen]
pub async fn list_json() -> Result<String, JsValue> {
    let payloads = active_api()?.list().await.map_err(js_error)?;
    serde_json::to_string(&payloads).map_err(js_error)
}

#[wasm_bindgen]
pub async fn put_json(name: String, payload_json: String) -> Result<(), JsValue> {
    let payload: shared::RecordPayload = serde_json::from_str(&payload_json).map_err(js_error)?;
    active_api()?.put(name, payload).await.map_err(js_error)
}

#[wasm_bindgen]
pub async fn delete_record(name: String) -> Result<(), JsValue> {
    active_api()?.delete(name).await.map_err(js_error)
}

#[wasm_bindgen]
pub async fn kad_snapshot(save_bytes: Vec<u8>) -> Result<Vec<u8>, JsValue> {
    let api = active_api()?;
    let mut save = Save::decode(&save_bytes).map_err(js_error)?;
    save.store = Some(api.kad_snapshot().await.map_err(js_error)?);
    save.encode().map_err(js_error)
}
