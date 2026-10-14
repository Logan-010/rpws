import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import test from 'node:test';

test('Chrome WASM passes the full shared vault integration suite', async () => {
  const shared = new URL('../../firefox-extension/tests/wasm-smoke.mjs', import.meta.url);
  const wasm = new URL('../wasm/web.js', import.meta.url).href;
  const bytes = new URL('../wasm/web_bg.wasm', import.meta.url).href;
  const source = (await readFile(shared, 'utf8'))
    .replace("from '../wasm/web.js'", `from ${JSON.stringify(wasm)}`)
    .replaceAll("new URL('../wasm/web.js', import.meta.url)", `new URL(${JSON.stringify(wasm)})`)
    .replaceAll("new URL('../wasm/web_bg.wasm', import.meta.url)", `new URL(${JSON.stringify(bytes)})`)
    .replaceAll('import.meta.url', JSON.stringify(shared.href));
  const result = spawnSync(process.execPath, ['--input-type=module', '--eval', source], { encoding: 'utf8', timeout: 60_000 });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
});
