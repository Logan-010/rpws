import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';

test('service worker creates one vault and preserves sender identity; privileged RPCs require the offscreen sender', async () => {
  let handler;
  let command;
  let exists = false;
  let creations = 0;
  const forwarded = [];
  const injected = [];
  const url = (path) => `chrome-extension://rpws/${path}`;
  const chrome = {
    runtime: {
      id: 'rpws', getURL: url,
      async getContexts() { return exists ? [{}] : []; },
      async sendMessage(message) { forwarded.push(message); return { ok: true, data: 'vault result' }; },
      onMessage: { addListener(listener) { handler = listener; } },
      onStartup: { addListener() {} }, onInstalled: { addListener() {} },
    },
    commands: { onCommand: { addListener(listener) { command = listener; } } },
    offscreen: { async createDocument() { creations++; await new Promise((resolve) => setTimeout(resolve, 5)); exists = true; } },
    storage: { local: { async get(key) { return { [key]: [1, 2] }; } } },
    scripting: { async executeScript(details) { injected.push(details); } },
  };
  vm.runInNewContext(await readFile(new URL('../src/service-worker.js', import.meta.url), 'utf8'), { chrome, console });
  const sender = { id: 'rpws', tab: { id: 7 }, frameId: 0, url: 'https://example.com/login' };
  const request = (message, from = sender) => new Promise((resolve) => {
    assert.equal(handler(message, from, resolve), true);
  });
  const replies = await Promise.all([request({ type: 'rpws-inline', action: 'suggestions' }), request({ type: 'rpws-inline', action: 'select', name: 'Example' })]);
  assert.equal(creations, 1);
  assert.equal(replies[0].data, 'vault result');
  assert.equal(forwarded[0].sender, sender);
  assert.equal(handler({ type: 'rpws', action: 'secret' }, sender, () => {}), undefined);
  await request({ type: 'rpws', action: 'state' }, { ...sender, url: url('popup.html') });
  assert.equal(forwarded.at(-1).sender.tab, undefined);
  assert.equal(forwarded.at(-1).sender.url, url('popup.html'));
  assert.equal(handler({ type: 'rpws-api', method: 'storage.get' }, sender, () => {}), undefined);
  assert.equal(handler({ type: 'rpws-api', method: 'storage.get' }, { id: 'rpws', url: url('popup.html') }, () => {}), undefined);
  const vaultSender = { id: 'rpws', url: url('offscreen.html') };
  const stored = await request({ type: 'rpws-api', method: 'storage.get', args: ['rpwsSave'] }, vaultSender);
  assert.equal(stored.ok, true);
  assert.deepEqual(stored.data, { rpwsSave: [1, 2] });
  await request({ type: 'rpws-api', method: 'tabs.executeScript', args: [7, { file: 'autofill.js', frameId: 0 }] }, vaultSender);
  assert.equal(JSON.stringify(injected[0]), JSON.stringify({ target: { tabId: 7, frameIds: [0] }, files: ['autofill.js'] }));
  command('autofill');
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.equal(forwarded.at(-1).message.type, 'rpws-command');
});

test('offscreen adapter uses callback responses and rejects forged forwarding envelopes', async () => {
  let handler;
  let originalSender;
  const chrome = { runtime: {
    id: 'rpws', getURL: (path) => `chrome-extension://rpws/${path}`,
    onMessage: { addListener(listener) { handler = listener; } },
  } };
  const context = vm.createContext({ chrome, console });
  vm.runInContext(await readFile(new URL('../src/vault-browser.js', import.meta.url), 'utf8'), context);
  context.browser.runtime.onMessage.addListener(async (message, sender) => { originalSender = sender; return { ok: true, data: message.action }; });
  const envelope = { type: 'rpws-vault', message: { type: 'rpws', action: 'list' }, sender: { id: 'rpws', url: 'chrome-extension://rpws/popup.html' } };
  for (const sender of [{ id: 'other' }, { id: 'rpws', tab: { id: 1 } }, { id: 'rpws', url: 'chrome-extension://rpws/popup.html' }]) {
    assert.equal(handler(envelope, sender, () => assert.fail('Forged message was answered')), undefined);
  }
  const result = await new Promise((resolve) => {
    assert.equal(handler(envelope, { id: 'rpws' }, resolve), true);
  });
  assert.equal(result.data, 'list');
  assert.equal(originalSender, envelope.sender);
});
