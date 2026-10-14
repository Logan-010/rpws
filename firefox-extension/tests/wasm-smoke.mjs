import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import initialize, * as wasm from '../wasm/web.js';

// The browser API uses window timers; Node supplies equivalent timers for this
// offline integration check. This does not test Firefox permissions or networking.
globalThis.window = globalThis;
globalThis.self = globalThis;
globalThis.Window = Object;

try {
  await initialize({ module_or_path: await readFile(new URL('../wasm/web_bg.wasm', import.meta.url)) });
  let save = wasm.new_save();
  const generatedSecret = wasm.view_secret(save);
  assert.equal(generatedSecret.length, 32);
  assert.notDeepEqual(wasm.view_secret(wasm.new_save()), generatedSecret);
  const knownSecret = Uint8Array.from({ length: 32 }, (_, index) => index * 7);
  const base64Secret = Buffer.from(knownSecret).toString('base64');
  assert.throws(() => wasm.new_save_with_secret(new Uint8Array(31)));
  assert.throws(() => wasm.new_save_with_secret(new Uint8Array(33)));
  save = wasm.new_save_with_secret(knownSecret);
  assert.deepEqual(wasm.view_secret(save), knownSecret);
  assert.deepEqual(wasm.view_bootstrap_addrs(save), []);
  assert.throws(() => wasm.view_bootstrap_addrs(new Uint8Array([255])));
  assert.throws(() => wasm.set_bootstrap_addrs(save, ['not-a-multiaddress']));
  const addr = '/dns4/localhost/tcp/443/wss/p2p/12D3KooWJ5c7VJxJBSCmNMSZPDRHpNfzHd3YXQAEfpWcEJcWJjYQ';
  save = wasm.set_bootstrap_addrs(save, [addr]);
  assert.deepEqual(wasm.view_bootstrap_addrs(save), [addr]);
  await wasm.init(save);
  await wasm.connect();
  assert.deepEqual(JSON.parse(await wasm.list_json()), {});
  const name = 'Example <&" vault';
  const payload = { url: 'https://example.com', username: 'user', password: 'secret<&"', timestamp: 1791849600, extra: 'Notes\nsecond line' };
  await wasm.put_json(name, JSON.stringify(payload));
  assert.deepEqual(JSON.parse(await wasm.list_json())[name], payload);
  const snapshot = await wasm.kad_snapshot(save);
  assert.deepEqual(wasm.view_secret(snapshot), knownSecret);
  await wasm.init(snapshot);
  assert.deepEqual(JSON.parse(await wasm.list_json())[name], payload);
  payload.password = 'updated';
  await wasm.put_json(name, JSON.stringify(payload));
  assert.equal(JSON.parse(await wasm.list_json())[name].password, 'updated');
  await wasm.delete_record(name);
  const deletedSnapshot = await wasm.kad_snapshot(snapshot);
  await wasm.init(deletedSnapshot);
  assert.deepEqual(JSON.parse(await wasm.list_json()), {});
  assert.deepEqual(wasm.view_bootstrap_addrs(deletedSnapshot), [addr]);
  // Exercise the extension's message handler with real WASM and mocked Firefox
  // storage. Only Vite's two asset imports are adapted for Node execution.
  let handler;
  let commandHandler;
  let activeUrl = 'https://example.com/login';
  const fills = [];
  let popupOpens = 0;
  const cache = {};
  let failRemoval = false;
  const backgroundSnapshots = [];
  const originalSetInterval = globalThis.setInterval;
  globalThis.setInterval = (callback, delay, ...args) => {
    if (delay === 30_000) backgroundSnapshots.push(callback);
    return originalSetInterval(callback, delay, ...args);
  };
  globalThis.browser = {
    commands: { onCommand: { addListener(listener) { commandHandler = listener; } } },
    browserAction: { async openPopup() { popupOpens++; } },
    tabs: {
      async query() { return [{ id: 1, url: activeUrl }]; },
      async executeScript() {},
      async sendMessage(id, message, options) { fills.push({ id, message, options }); return { ok: true, passwordFilled: true }; },
    },
    runtime: { id: 'test-rpws', onMessage: { addListener(listener) { handler = listener; } } },
    storage: { local: {
      async get(key) { return { [key]: cache[key] }; },
      async set(values) { Object.assign(cache, structuredClone(values)); },
      async remove(key) {
        if (failRemoval) throw new Error('Storage removal failed');
        delete cache[key];
      },
    } },
  };
  globalThis.testWasmBytes = await readFile(new URL('../wasm/web_bg.wasm', import.meta.url));
  const background = (await readFile(new URL('../src/background.js', import.meta.url), 'utf8'))
    .replace("'../wasm/web.js'", JSON.stringify(new URL('../wasm/web.js', import.meta.url).href))
    .replace("'./autofill.js'", JSON.stringify(new URL('../src/autofill.js', import.meta.url).href))
    .replace("import wasmUrl from '../wasm/web_bg.wasm?url';", 'const wasmUrl = globalThis.testWasmBytes;');
  await import(`data:text/javascript;base64,${Buffer.from(background).toString('base64')}`);
  const message = (action, data = {}) => handler({ type: 'rpws', action, ...data }, { id: browser.runtime.id });
  assert.equal((await message('state')).data.loaded, false);
  assert.equal((await message('setup', { addresses: ['bad'] })).ok, false);
  assert.equal((await message('setup', { addresses: [addr] })).ok, true);
  assert.ok(cache.rpwsSave.length);
  const backgroundSecret = (await message('secret')).data;
  assert.equal(Buffer.from(backgroundSecret, 'base64').length, 32);
  assert.equal(Buffer.from(wasm.view_secret(new Uint8Array(cache.rpwsSave))).toString('base64'), backgroundSecret);
  assert.equal((await message('put', { name, payload })).ok, true);
  const exported = (await message('export')).data;
  assert.deepEqual((await message('list')).data[name], payload);
  assert.equal(handler({ type: 'rpws', action: 'secret' }, { id: browser.runtime.id, tab: { id: 1 } }), undefined);
  assert.equal((await message('autofill-context')).data.origin, 'https://example.com');
  assert.equal((await message('autofill', { name })).ok, true);
  assert.equal(fills.at(-1).message.password, payload.password);
  assert.deepEqual(fills.at(-1).options, { frameId: 0 });
  activeUrl = 'https://example.com.evil.test';
  assert.equal((await message('autofill', { name })).ok, false);
  assert.equal(fills.length, 1);
  activeUrl = 'https://example.com/login';
  commandHandler('autofill');
  await message('state');
  assert.equal(fills.length, 2);
  await message('put', { name: 'Second login', payload });
  commandHandler('autofill');
  await message('state');
  assert.equal(popupOpens, 1);
  assert.equal(fills.length, 2);
  await message('delete', { name: 'Second login' });
  const beforeUnload = structuredClone(cache.rpwsSave);
  const oldRevision = (await message('ui-state')).data.revision;
  const draftUi = { view: 'editor', drafts: { 'entry-form': { password: 'draft' } } };
  await message('ui-state-set', { revision: oldRevision, state: draftUi });
  assert.deepEqual((await message('ui-state')).data.state, draftUi);
  failRemoval = true;
  assert.equal((await message('unload')).ok, false);
  assert.equal((await message('state')).data.loaded, true);
  assert.deepEqual((await message('ui-state')).data.state, draftUi);
  assert.deepEqual(cache.rpwsSave, beforeUnload);
  assert.deepEqual((await message('list')).data[name], payload);
  failRemoval = false;
  assert.equal((await message('unload')).ok, true);
  assert.equal((await message('state')).data.loaded, false);
  assert.equal((await message('ui-state')).data.state, null);
  await message('ui-state-set', { revision: oldRevision, state: draftUi });
  assert.equal((await message('ui-state')).data.state, null);
  assert.equal(cache.rpwsSave, undefined);
  assert.equal((await message('list')).ok, false);
  assert.equal((await message('secret')).ok, false);
  await assert.rejects(wasm.list_json(), (error) => String(error).includes('API not initialized'));
  backgroundSnapshots.at(-1)();
  await message('state'); // Wait for the serialized periodic snapshot.
  assert.equal(cache.rpwsSave, undefined);
  await import(`data:text/javascript;base64,${Buffer.from(`${background}\n// unloaded-restart-context`).toString('base64')}`);
  assert.equal((await message('state')).data.loaded, false);
  assert.equal((await message('setup', { addresses: [addr] })).ok, true);
  assert.deepEqual((await message('list')).data, {});
  assert.equal((await message('unload')).ok, true);
  assert.equal((await message('import', { bytes: exported })).ok, true);
  assert.deepEqual((await message('list')).data[name], payload);
  assert.equal((await message('import', { bytes: [255] })).ok, false);
  assert.deepEqual((await message('list')).data[name], payload);
  assert.equal((await message('addresses', { addresses: [] })).ok, true);
  assert.deepEqual((await message('state')).data.addresses, []);
  assert.deepEqual((await message('list')).data[name], payload);
  assert.equal((await message('delete', { name })).ok, true);
  assert.deepEqual((await message('list')).data, {});
  assert.equal((await message('import', { bytes: exported })).ok, true);
  assert.deepEqual((await message('list')).data[name], payload);
  assert.deepEqual((await message('state')).data.addresses, [addr]);
  assert.equal((await message('secret')).data, backgroundSecret);
  // A new background context with no cached save exercises secret-based setup.
  delete cache.rpwsSave;
  await import(`data:text/javascript;base64,${Buffer.from(`${background}\n// secret-import-context`).toString('base64')}`);
  for (const secret of ['', 'not base64!', Buffer.alloc(31).toString('base64'), Buffer.alloc(33).toString('base64'), `${'A'.repeat(42)}B=`]) {
    assert.equal((await message('setup', { addresses: [addr], secretMode: 'import', secret })).ok, false);
    assert.equal((await message('state')).data.loaded, false);
    assert.equal(cache.rpwsSave, undefined);
  }
  assert.equal((await message('setup', { addresses: [addr], secretMode: 'import', secret: ` ${base64Secret.replace(/=+$/, '')} ` })).ok, true);
  assert.equal((await message('secret')).data, base64Secret);
  assert.deepEqual(wasm.view_secret(new Uint8Array(cache.rpwsSave)), knownSecret);
  assert.equal((await message('put', { name, payload })).ok, true);
  const secretExport = (await message('export')).data;
  assert.equal((await message('addresses', { addresses: [] })).ok, true);
  assert.equal((await message('secret')).data, base64Secret);
  assert.equal((await message('import', { bytes: secretExport })).ok, true);
  assert.equal((await message('secret')).data, base64Secret);
  assert.deepEqual((await message('list')).data[name], payload);
  console.log('WASM integration passed: save validation, addresses, CRUD, and snapshot restoration.');
  console.log('Background integration passed: message handling, local storage, import/export, and bootstrap changes.');
  console.log('Secret integration passed: generation, base64 validation/import, and persistence.');
  console.log('Unload integration passed: storage failure recovery, cache removal, WASM shutdown, restart, and reimport.');
  process.exit(0); // The persistent WASM swarm intentionally owns browser timers.
} catch (error) {
  console.error(error);
  process.exit(1);
}
