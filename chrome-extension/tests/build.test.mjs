import assert from 'node:assert/strict';
import { readFile, readdir } from 'node:fs/promises';
import test from 'node:test';

test('packaged WASM matches the Chrome build rather than a stale Firefox artifact', async () => {
  const assets = new URL('../dist/assets/', import.meta.url);
  const files = await readdir(assets);
  const binary = files.find((name) => name.endsWith('.wasm'));
  assert.ok(binary, 'Build the extension before running tests');
  assert.deepEqual(await readFile(new URL(binary, assets)), await readFile(new URL('../wasm/web_bg.wasm', import.meta.url)));
  const offscreen = files.find((name) => /^offscreen-.*\.js$/.test(name));
  const source = await readFile(new URL(offscreen, assets), 'utf8');
  assert.ok(source.includes(binary), 'Offscreen vault must reference the freshly built binary');
  assert.equal(source.includes('new Function('), false, 'Generated WASM glue must obey the extension CSP');
});
