// Invoked by the native daemon integration test with an ephemeral WS listener.
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import initialize, * as wasm from '../wasm/web.js';

globalThis.window = globalThis;
globalThis.self = globalThis;
globalThis.Window = Object;

try {
  await initialize({ module_or_path: await readFile(new URL('../wasm/web_bg.wasm', import.meta.url)) });
  const seed = new Uint8Array(32).fill(7);
  const address = process.argv[2];
  const legacy = await readFile(process.argv[3]);
  await wasm.init(legacy);
  assert.equal(JSON.parse(await wasm.list_json())['legacy account'].password, 'legacy-password');
  const migrated = await wasm.kad_snapshot(legacy);
  assert.deepEqual(wasm.view_secret(migrated), seed);
  await wasm.init(migrated);
  assert.equal(JSON.parse(await wasm.list_json())['legacy account'].password, 'legacy-password');
  wasm.unload();
  let save = wasm.set_bootstrap_addrs(wasm.new_save_with_secret(seed), [address]);
  await wasm.init(save);
  await wasm.sync();
  let entries = JSON.parse(await wasm.list_json());
  assert.deepEqual(Object.keys(entries).sort(), ['daemon account', 'second account']);
  assert.equal(entries['daemon account'].password, 'daemon-password');
  assert.equal(entries['second account'].password, 'second-password');
  save = await wasm.kad_snapshot(save);
  await wasm.init(save);
  assert.equal(JSON.parse(await wasm.list_json())['daemon account'].password, 'daemon-password');
  await wasm.put_json('browser account', JSON.stringify({ url: 'https://browser.example', username: 'browser', password: 'browser-password', timestamp: 1000, extra: '' }));
  // The daemon enumerates the browser's cache over the existing connection.
  await new Promise((resolve) => setTimeout(resolve, 6500));
  await wasm.sync();
  entries = JSON.parse(await wasm.list_json());
  assert.equal(entries['browser account'].password, 'browser-password');
  const staleSave = save;
  await wasm.delete_record('daemon account');
  await wasm.delete_record('second account');
  await wasm.sync();
  entries = JSON.parse(await wasm.list_json());
  assert.equal(entries['daemon account'], undefined);
  assert.equal(entries['second account'], undefined);
  save = await wasm.kad_snapshot(save);
  await wasm.init(save);
  await wasm.sync();
  assert.equal(JSON.parse(await wasm.list_json())['daemon account'], undefined);
  await wasm.put_json('second account', JSON.stringify({ url: 'https://example.com', username: 'alice', password: 'recreated-password', timestamp: 1, extra: '' }));
  assert.equal(JSON.parse(await wasm.list_json())['second account'].password, 'recreated-password');
  await new Promise((resolve) => setTimeout(resolve, 6500));
  // An old browser snapshot still contains both original passwords. It must
  // learn the deletion/newer re-creation instead of restoring the old copies.
  await wasm.init(staleSave);
  await wasm.sync();
  entries = JSON.parse(await wasm.list_json());
  assert.equal(entries['daemon account'], undefined);
  assert.equal(entries['second account'].password, 'recreated-password');
  const wrongSave = wasm.set_bootstrap_addrs(wasm.new_save_with_secret(new Uint8Array(32).fill(8)), [address]);
  await wasm.init(wrongSave);
  await assert.rejects(wasm.sync(), (error) => String(error).includes('different vault secret'));
  assert.deepEqual(JSON.parse(await wasm.list_json()), {});
  wasm.unload();
  console.log('Live daemon/WASM WebSocket sync passed: bidirectional records, persistent deletion, stale snapshots, re-creation, secret isolation.');
  process.exit(0);
} catch (error) {
  console.error(error);
  process.exit(1);
}
