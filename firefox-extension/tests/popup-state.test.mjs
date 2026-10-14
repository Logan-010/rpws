import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { JSDOM } from 'jsdom';

const html = await readFile(new URL('../popup.html', import.meta.url), 'utf8');
const source = (await readFile(new URL('../src/popup.js', import.meta.url), 'utf8')).replace("import './style.css';", '');
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };

function background(loaded = true) {
  const calls = [];
  let state = null;
  let revision = 0;
  const entries = { 'Example <&"': { url: 'https://example.com', username: 'alice', password: 'secret', extra: 'notes', timestamp: 1000 } };
  return { calls, runtime: { async sendMessage(message) {
    message = structuredClone(message);
    calls.push(message.action);
    let data;
    switch (message.action) {
      case 'ui-state': data = { state, revision }; break;
      case 'ui-state-set': if (revision === message.revision) state = message.state; break;
      case 'state': data = { loaded, addresses: ['/dns4/peer/tcp/443/wss/p2p/peer'] }; break;
      case 'list': data = entries; break;
      case 'put': entries[message.name] = message.payload; break;
      case 'sync': data = 0; break;
      case 'setup': case 'import': loaded = true; state = null; revision++; break;
      case 'unload': loaded = false; state = null; revision++; break;
      default: throw new Error(`Unexpected message: ${message.action}`);
    }
    return structuredClone({ ok: true, data });
  } } };
}

async function open(browser) {
  const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'https://extension.test/popup.html' });
  const { window } = dom;
  window.browser = browser;
  window.scrollTo = (_, y) => { window.scrollY = y; };
  window.eval(source);
  await settle();
  return {
    window,
    query: (selector) => window.document.querySelector(selector),
    async click(selector) { window.document.querySelector(selector).click(); await settle(); },
    async input(selector, value) {
      const field = window.document.querySelector(selector);
      field.value = value;
      field.dispatchEvent(new window.Event('input', { bubbles: true }));
      await settle();
    },
    close() { window.dispatchEvent(new window.Event('pagehide')); window.close(); },
  };
}

test('reopening restores list state, editor drafts, settings and confirmation; unload resets state', async () => {
  const browser = background();
  let popup = await open(browser);
  try {
    await popup.input('#search', 'alice');
    const details = popup.query('details');
    details.open = true;
    details.dispatchEvent(new popup.window.Event('toggle'));
    popup.window.scrollTo(0, 120);
    popup.window.dispatchEvent(new popup.window.Event('scroll'));
    await settle();
    popup.close();
    popup = await open(browser);
    assert.equal(popup.query('#search').value, 'alice');
    assert.equal(popup.query('details').open, true);
    assert.equal(popup.window.scrollY, 120);
    await popup.click('[data-action="edit"]');
    await popup.input('[name="password"]', 'unfinished<&" password');
    await popup.input('[name="extra"]', '\nDraft notes\nsecond line');
    popup.query('[name="extra"]').focus();
    popup.query('[name="extra"]').setSelectionRange(3, 8);
    await settle();
    popup.close();
    popup = await open(browser);
    assert.equal(popup.query('#entry-form').dataset.editing, 'true');
    assert.equal(popup.query('[name="password"]').value, 'unfinished<&" password');
    assert.equal(popup.query('[name="password"]').type, 'password');
    assert.equal(popup.query('[name="extra"]').value, '\nDraft notes\nsecond line');
    assert.equal(popup.window.document.activeElement.name, 'extra');
    assert.equal(popup.query('[name="extra"]').selectionStart, 3);
    assert.equal(popup.query('[name="extra"]').selectionEnd, 8);
    await popup.click('[data-action="back"]');
    await popup.click('[data-action="add"]');
    await popup.input('[name="name"]', 'new draft');
    await popup.input('[name="password"]', 'draft password');
    popup.close();
    popup = await open(browser);
    assert.equal(popup.query('#entry-form').dataset.editing, 'false');
    assert.equal(popup.query('[name="name"]').value, 'new draft');
    assert.equal(popup.query('[name="password"]').value, 'draft password');
    await popup.click('#settings');
    await popup.input('[name="addresses"]', 'unfinished bootstrap address');
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('#addresses-form'));
    assert.equal(popup.query('[name="addresses"]').value, 'unfinished bootstrap address');
    await popup.click('[data-action="unload"]');
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('[data-action="confirm-unload"]'));
    await popup.click('[data-action="confirm-unload"]');
    assert.ok(popup.query('#setup-form'));
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('#setup-form'));
    assert.equal(popup.query('[name="addresses"]').value, '');
    const saved = (await browser.runtime.sendMessage({ action: 'ui-state' })).data.state;
    assert.equal(saved.drafts['entry-form'], undefined);
  } finally { popup.close(); }
});

test('reopening restores setup mode and secret draft, then discards them after setup', async () => {
  const browser = background(false);
  let popup = await open(browser);
  try {
    const mode = popup.query('#secret-mode');
    mode.value = 'import';
    mode.dispatchEvent(new popup.window.Event('change', { bubbles: true }));
    await popup.input('[name="secret"]', 'A'.repeat(43) + '=');
    await popup.input('[name="addresses"]', '/dns4/peer/tcp/443/wss/p2p/peer');
    popup.close();
    popup = await open(browser);
    assert.equal(popup.query('#secret-mode').value, 'import');
    assert.equal(popup.query('[name="secret"]').value, 'A'.repeat(43) + '=');
    assert.equal(popup.query('[name="secret"]').disabled, false);
    assert.equal(popup.query('#secret-field').classList.contains('hidden'), false);
    popup.query('#setup-form').dispatchEvent(new popup.window.Event('submit', { bubbles: true, cancelable: true }));
    await settle();
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('#search'));
    const saved = (await browser.runtime.sendMessage({ action: 'ui-state' })).data.state;
    assert.equal(saved.drafts['setup-form'], undefined);
  } finally { popup.close(); }
});

test('pending save import survives popup close and clears state on import', async () => {
  const browser = background();
  let popup = await open(browser);
  try {
    const input = popup.query('#import-file');
    Object.defineProperty(input, 'files', { value: [{ name: 'backup.rpws', async arrayBuffer() { return new Uint8Array([1, 2, 3]).buffer; } }] });
    input.dispatchEvent(new popup.window.Event('change'));
    await settle();
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('#confirm-import'));
    assert.match(popup.query('#app').textContent, /backup\.rpws/);
    await popup.click('#confirm-import');
    popup.close();
    popup = await open(browser);
    assert.ok(popup.query('#search'));
    assert.equal((await browser.runtime.sendMessage({ action: 'ui-state' })).data.state.pendingImport, undefined);
  } finally { popup.close(); }
});

test('Refresh requests network sync before rereading the password cache', async () => {
  const browser = background();
  const popup = await open(browser);
  try {
    browser.calls.length = 0;
    await popup.click('[data-action="refresh"]');
    const sync = browser.calls.indexOf('sync');
    assert.ok(sync >= 0);
    assert.ok(browser.calls.indexOf('list') > sync);
    assert.match(popup.query('#status').textContent, /Vault synced/);
  } finally { popup.close(); }
});
