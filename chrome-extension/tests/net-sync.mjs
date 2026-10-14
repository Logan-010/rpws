// Run via the daemon's browser_sync_enumerates_daemon_passwords_over_websocket
// integration test, with RPWS_BROWSER_SYNC_SCRIPT pointing at this file and
// CHROME_BIN pointing at a Chromium executable.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const profile = await mkdtemp(join(tmpdir(), 'rpws-chrome-sync-'));
const extension = fileURLToPath(new URL('../dist', import.meta.url));
const child = spawn(process.env.CHROME_BIN || 'chromium', [
  '--headless=new', '--no-sandbox', '--no-first-run', '--disable-gpu',
  `--user-data-dir=${profile}`, '--remote-debugging-port=0',
  `--disable-extensions-except=${extension}`, `--load-extension=${extension}`,
  ...JSON.parse(process.env.CHROME_EXTRA_ARGS || '[]'),
  'about:blank',
], { stdio: ['ignore', 'ignore', 'pipe'] });
let socket;
try {
  const debuggerUrl = await new Promise((resolve, reject) => {
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      stderr += chunk;
      const match = stderr.match(/DevTools listening on (ws:\/\/[^\s]+)/);
      if (match) resolve(match[1]);
    });
    child.on('error', reject);
    child.on('exit', (code) => reject(new Error(`Chrome exited (${code}): ${stderr}`)));
  });
  socket = new WebSocket(debuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let id = 0;
  const pending = new Map();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (!message.id) return;
    const request = pending.get(message.id);
    pending.delete(message.id);
    if (message.error) request.reject(new Error(JSON.stringify(message.error)));
    else request.resolve(message.result);
  };
  const cdp = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    pending.set(++id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
  let worker;
  for (let i = 0; i < 100; i++) {
    const { targetInfos } = await cdp('Target.getTargets');
    worker = targetInfos.find((target) => target.type === 'service_worker' && target.url.endsWith('/service-worker.js'));
    if (worker) break;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  assert.ok(worker, 'Chrome did not load the extension service worker');
  const popupUrl = new URL('popup.html', worker.url).href;
  const { targetId } = await cdp('Target.createTarget', { url: popupUrl });
  const { sessionId } = await cdp('Target.attachToTarget', { targetId, flatten: true });
  await new Promise((resolve) => setTimeout(resolve, 500));
  const rpc = async (action, data = {}, expectedOk = true) => {
    const message = JSON.stringify({ type: 'rpws', action, ...data });
    const result = await cdp('Runtime.evaluate', {
      expression: `chrome.runtime.sendMessage(${message})`, awaitPromise: true, returnByValue: true,
    }, sessionId);
    assert.equal(result.exceptionDetails, undefined, JSON.stringify(result.exceptionDetails));
    const response = result.result.value;
    assert.equal(response?.ok, expectedOk, response?.error || 'No vault response');
    return expectedOk ? response.data : response.error;
  };
  const address = process.argv[2];
  const legacy = Array.from(await readFile(process.argv[3]));
  await rpc('import', { bytes: legacy });
  assert.equal((await rpc('list'))['legacy account'].password, 'legacy-password');
  await rpc('unload');
  const wrongPeer = '12D3KooWJ5c7VJxJBSCmNMSZPDRHpNfzHd3YXQAEfpWcEJcWJjYQ';
  const wrongAddress = address.replace(/\/p2p\/[^/]+$/, `/p2p/${wrongPeer}`);
  await rpc('setup', { addresses: [wrongAddress], secretMode: 'import', secret: Buffer.alloc(32, 7).toString('base64') });
  const failure = await rpc('sync', {}, false);
  assert.match(failure, /Unexpected peer ID/);
  assert.ok(failure.includes(address.split('/p2p/')[0]), 'Dial diagnostics must include the failed WebSocket address');
  await rpc('unload');
  await rpc('setup', { addresses: [address], secretMode: 'import', secret: Buffer.alloc(32, 7).toString('base64') });
  await rpc('sync');
  assert.deepEqual(Object.keys(await rpc('list')).sort(), ['daemon account', 'second account']);
  const stale = await rpc('export');
  await rpc('put', { name: 'browser account', payload: { url: 'https://browser.example', username: 'browser', password: 'browser-password', timestamp: 1000, extra: '' } });
  await new Promise((resolve) => setTimeout(resolve, 6500));
  await rpc('sync');
  await rpc('delete', { name: 'daemon account' });
  await rpc('delete', { name: 'second account' });
  await rpc('sync');
  await rpc('put', { name: 'second account', payload: { url: 'https://example.com', username: 'alice', password: 'recreated-password', timestamp: 1, extra: '' } });
  await new Promise((resolve) => setTimeout(resolve, 6500));
  await rpc('import', { bytes: stale });
  await rpc('sync');
  const entries = await rpc('list');
  assert.equal(entries['daemon account'], undefined);
  assert.equal(entries['second account'].password, 'recreated-password');
  assert.equal(entries['browser account'].password, 'browser-password');
  console.log('Live Chrome extension/daemon WebSocket sync passed: offscreen vault, bidirectional records, deletion, stale snapshots, re-creation.');
} finally {
  socket?.close();
  child.kill();
  await new Promise((resolve) => { if (child.exitCode !== null) resolve(); else child.once('exit', resolve); });
  await rm(profile, { recursive: true, force: true });
}
