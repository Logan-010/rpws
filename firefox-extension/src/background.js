import initialize, * as wasm from '../wasm/web.js';
import wasmUrl from '../wasm/web_bg.wasm?url';
import { activeLoginTab, fillTab, loginOrigin } from './autofill.js';

const KEY = 'rpwsSave';
let save = null;
let queue = Promise.resolve();
let loadError = null;
// Popup state lives only in this persistent background page, not on disk.
let uiState = null;
let uiRevision = 0;

function resetUiState() {
  uiState = null;
  uiRevision += 1;
}

async function persist() {
  if (!save) return;
  const snapshot = wasm.kad_snapshot(save);
  save = new Uint8Array(await snapshot);
  await browser.storage.local.set({ [KEY]: Array.from(save) });
}

async function activate(bytes) {
  // Initialization validates the entire save, including its peer identity.
  await wasm.init(bytes);
  save = new Uint8Array(bytes);
  await wasm.connect();
}

const ready = (async () => {
  await initialize({ module_or_path: wasmUrl });
  const cached = (await browser.storage.local.get(KEY))[KEY];
  if (cached) {
    try { await activate(new Uint8Array(cached)); }
    catch (error) { loadError = `Could not load the cached save: ${String(error?.message || error)}`; }
  }
})();

function serial(operation) {
  const result = queue.then(operation);
  queue = result.catch(() => {});
  return result;
}

function addresses(input, required = false) {
  if (!Array.isArray(input)) throw new Error('Enter bootstrap addresses.');
  const addrs = [...new Set(input.map((value) => value.trim()).filter(Boolean))];
  if (required && !addrs.length) throw new Error('Enter at least one bootstrap address.');
  for (const addr of addrs) {
    if (!/\/(ws|wss)(\/|$)/.test(addr) && !/\/tls\/ws(\/|$)/.test(addr)) {
      throw new Error('Browser bootstrap addresses must use WebSocket (/ws or /wss).');
    }
    if (!/\/p2p\/[^/]+$/.test(addr)) throw new Error('Bootstrap addresses must end with /p2p/<peer-id>.');
  }
  return addrs;
}

function decodeSecret(input) {
  if (typeof input !== 'string') throw new Error('Enter a base64-encoded secret.');
  const value = input.trim();
  // Standard base64 for 32 bytes has 43 data characters and optional padding.
  if (!/^[A-Za-z0-9+/]{43}=?$/.test(value)) {
    throw new Error('Secret must be valid base64 encoding exactly 32 bytes.');
  }
  const decoded = atob(value);
  if (decoded.length !== 32 || btoa(decoded).replace(/=+$/, '') !== value.replace(/=+$/, '')) {
    throw new Error('Secret must be valid base64 encoding exactly 32 bytes.');
  }
  return Uint8Array.from(decoded, (character) => character.charCodeAt(0));
}

async function dispatch(message) {
  await ready;
  switch (message.action) {
    case 'state':
      return { loaded: !!save, addresses: save ? wasm.view_bootstrap_addrs(save) : [], error: loadError };
    case 'setup': {
      if (save) throw new Error('A save already exists.');
      const mode = message.secretMode ?? 'generate';
      if (!['generate', 'import'].includes(mode)) throw new Error('Choose whether to generate or import a secret.');
      const initial = mode === 'import'
        ? wasm.new_save_with_secret(decodeSecret(message.secret))
        : wasm.new_save();
      const bytes = wasm.set_bootstrap_addrs(initial, addresses(message.addresses, true));
      await activate(bytes);
      await persist();
      loadError = null;
      resetUiState();
      return true;
    }
    case 'import': {
      const bytes = new Uint8Array(message.bytes);
      wasm.view_bootstrap_addrs(bytes);
      const previous = save;
      try {
        await activate(bytes);
        await persist();
        loadError = null;
        resetUiState();
      } catch (error) {
        save = previous;
        if (previous) await activate(previous);
        throw error;
      }
      return true;
    }
  }
  if (!save) throw new Error('Create or import a save first.');
  switch (message.action) {
    case 'unload':
      // Keep the active vault intact if removing its cached save fails.
      await browser.storage.local.remove(KEY);
      wasm.unload();
      save = null;
      loadError = null;
      resetUiState();
      return true;
    case 'secret': return btoa(String.fromCharCode(...wasm.view_secret(save)));
    case 'list': return JSON.parse(await wasm.list_json());
    case 'autofill-context': {
      try {
        const tab = await activeLoginTab();
        return { origin: loginOrigin(tab.url) };
      } catch { return { origin: null }; }
    }
    case 'autofill': {
      const tab = await activeLoginTab();
      const entries = JSON.parse(await wasm.list_json());
      if (!Object.hasOwn(entries, message.name)) throw new Error('This login no longer exists.');
      return fillTab(tab, entries[message.name]);
    }
    case 'sync': {
      try { return await wasm.sync(); }
      finally { await persist(); }
    }
    case 'put':
      try {
        await wasm.put_json(message.name, JSON.stringify(message.payload));
      } finally {
        // The API may update the local store before reporting a publish failure.
        await persist();
      }
      return true;
    case 'delete':
      try { await wasm.delete_record(message.name); }
      finally { await persist(); }
      return true;
    case 'export':
      await persist();
      return Array.from(save);
    case 'addresses': {
      await persist();
      const previous = save;
      const bytes = wasm.set_bootstrap_addrs(save, addresses(message.addresses));
      try {
        await activate(bytes);
        await persist();
      } catch (error) {
        await activate(previous);
        throw error;
      }
      return true;
    }
    default: throw new Error('Unknown RPWS action.');
  }
}

async function inlineAutofill(message, sender) {
  await ready;
  if (!sender.tab || sender.frameId !== 0) throw new Error('Inline autofill requires a top-level page.');
  const origin = loginOrigin(sender.url);
  const tab = await browser.tabs.get(sender.tab.id);
  if (!origin || origin !== loginOrigin(tab.url)) throw new Error('The login page changed.');
  if (!['suggestions', 'select'].includes(message.action)) throw new Error('Unknown inline autofill action.');
  if (!save) {
    if (message.action === 'suggestions') return [];
    throw new Error('Create or import a save first.');
  }
  const entries = JSON.parse(await wasm.list_json());
  if (message.action === 'suggestions') {
    // Only site-matched display metadata goes to the inline picker, never passwords.
    return Object.entries(entries)
      .filter(([, record]) => loginOrigin(record.url) === origin)
      .map(([name, record]) => ({ name, username: record.username }))
      .sort((a, b) => a.name.localeCompare(b.name));
  }
  if (!Object.hasOwn(entries, message.name) || loginOrigin(entries[message.name].url) !== origin) {
    throw new Error('This login no longer matches the page.');
  }
  return fillTab(tab, entries[message.name]);
}

browser.runtime.onMessage.addListener((message, sender) => {
  if (sender.id !== browser.runtime.id) return;
  if (message?.type === 'rpws-inline') {
    return serial(async () => {
      try { return { ok: true, data: await inlineAutofill(message, sender) }; }
      catch (error) { return { ok: false, error: String(error?.message || error) }; }
    });
  }
  if (message?.type !== 'rpws') return;
  // Content scripts must never gain access to vault or popup-state RPCs.
  if (sender.tab) return;
  // Store input events immediately, independently of slower vault operations.
  if (message.action === 'ui-state-set') {
    if (message.revision === uiRevision) uiState = structuredClone(message.state);
    return Promise.resolve({ ok: true });
  }
  if (message.action === 'ui-state') {
    return Promise.resolve({ ok: true, data: { revision: uiRevision, state: uiState } });
  }
  return serial(async () => {
    try { return { ok: true, data: await dispatch(message) }; }
    catch (error) { return { ok: false, error: String(error?.message || error) }; }
  });
});

// Cache records received from peers as well as explicit UI mutations.
setInterval(() => {
  serial(async () => { await ready; await persist(); }).catch(console.error);
}, 30_000);
ready.catch(console.error);

browser.commands.onCommand.addListener((command) => {
  if (command !== 'autofill') return;
  serial(async () => {
    await ready;
    if (!save) return browser.browserAction.openPopup();
    const tab = await activeLoginTab();
    const entries = JSON.parse(await wasm.list_json());
    const matches = Object.values(entries).filter((record) => loginOrigin(record.url) === loginOrigin(tab.url));
    if (matches.length !== 1) return browser.browserAction.openPopup();
    await fillTab(tab, matches[0]);
  }).catch(async (error) => {
    console.error(error);
    await browser.browserAction.openPopup();
  });
});
