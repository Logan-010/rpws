import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { JSDOM } from 'jsdom';
import { fillTab, loginOrigin } from '../src/autofill.js';

const source = await readFile(new URL('../public/autofill.js', import.meta.url), 'utf8');
const credentials = { type: 'rpws-fill', origin: 'https://example.com', username: 'alice<&"', password: 'secret<&"' };

const settle = async () => { for (let i = 0; i < 4; i++) await new Promise((resolve) => setTimeout(resolve, 0)); };

function page(html, logins = []) {
  const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'https://example.com/login' });
  const { window } = dom;
  const listeners = [];
  const calls = [];
  let pickerRoot;
  const attachShadow = window.Element.prototype.attachShadow;
  window.Element.prototype.attachShadow = function (options) {
    const root = attachShadow.call(this, options);
    if (options.mode === 'closed') pickerRoot = root;
    return root;
  };
  window.browser = { runtime: {
    id: 'rpws-test', onMessage: { addListener(fn) { listeners.push(fn); } },
    async sendMessage(message) {
      calls.push(structuredClone(message));
      if (message.action === 'suggestions') return { ok: true, data: logins };
      if (message.action === 'select') return { ok: true, data: await listeners[0](credentials, { id: 'rpws-test' }) };
      throw new Error('Unknown message');
    },
  } };
  // jsdom has no layout. Supply a rectangle; CSS visibility is still checked.
  window.HTMLInputElement.prototype.getClientRects = () => [{ width: 100, height: 20 }];
  window.eval(source);
  return { window, document: window.document, listeners, calls, picker: () => pickerRoot,
    fill: (message = credentials, sender = { id: 'rpws-test' }) => listeners[0](message, sender),
    close: () => window.close() };
}

test('origin checks reject lookalike hosts, subdomains, HTTP downgrade, ports and invalid URLs', async () => {
  assert.equal(loginOrigin('https://example.com/path'), 'https://example.com');
  for (const url of ['https://example.com.evil.test', 'https://login.example.com', 'http://example.com', 'https://example.com:8443', 'javascript:alert(1)', 'invalid']) {
    await assert.rejects(fillTab({ id: 1, url }, { url: 'https://example.com', ...credentials }), /exact origin/);
  }
  assert.equal(loginOrigin('about:blank'), null);
});

test('fills a login with native setters and events without submitting; skips hidden fields', async () => {
  const p = page('<form><input name="username" style="display:none"><input id="user" autocomplete="username"><input id="pass" type="password" autocomplete="current-password"><button>Log in</button></form>');
  try {
    const events = [];
    p.document.addEventListener('input', (event) => events.push(event.target.id));
    p.document.addEventListener('change', (event) => events.push(event.target.id));
    let submitted = false;
    p.document.querySelector('form').addEventListener('submit', () => { submitted = true; });
    const result = await p.fill();
    assert.equal(result.ok, true);
    assert.equal(p.document.querySelector('#user').value, credentials.username);
    assert.equal(p.document.querySelector('#pass').value, credentials.password);
    assert.equal(p.document.querySelector('[name="username"]').value, '');
    assert.deepEqual(events, ['user', 'user', 'pass', 'pass']);
    assert.equal(submitted, false);
    p.window.eval(source);
    assert.equal(p.listeners.length, 1);
  } finally { p.close(); }
});

test('supports username-first steps and dynamically added forms in open shadow roots', async () => {
  const p = page('<input autocomplete="username"><div id="host"></div>');
  try {
    assert.equal((await p.fill()).passwordFilled, false);
    assert.equal(p.document.querySelector('input').value, credentials.username);
    const shadow = p.document.querySelector('#host').attachShadow({ mode: 'open' });
    shadow.innerHTML = '<form><input name="username"><input type="password" autocomplete="current-password"></form>';
    shadow.querySelector('[type="password"]').focus();
    assert.equal((await p.fill()).passwordFilled, true);
    assert.equal(shadow.querySelector('[type="password"]').value, credentials.password);
    assert.equal(shadow.querySelector('[name="username"]').value, credentials.username);
  } finally { p.close(); }
});

test('inline suggestions appear on focus, filter usernames locally, and fill only on selection', async () => {
  const logins = [{ name: 'Alice <&"', username: 'alice' }, { name: 'Bob', username: 'bob' }];
  const p = page('<form><input name="username"><input type="password"></form>', logins);
  try {
    const user = p.document.querySelector('[name="username"]');
    const password = p.document.querySelector('[type="password"]');
    user.focus();
    await settle();
    assert.equal(p.picker().querySelectorAll('button').length, 2);
    assert.equal(p.picker().querySelector('button span').textContent, 'Alice <&"');
    assert.equal(p.document.querySelector('[data-rpws-inline]').shadowRoot, null);
    assert.equal(user.value, '');
    assert.equal(password.value, '');
    user.value = 'ali';
    user.dispatchEvent(new p.window.Event('input', { bubbles: true }));
    assert.equal(p.picker().querySelectorAll('button').length, 1);
    assert.deepEqual(p.calls, [{ type: 'rpws-inline', action: 'suggestions' }]);
    p.picker().querySelector('button').click();
    await settle();
    assert.deepEqual(p.calls[1], { type: 'rpws-inline', action: 'select', name: 'Alice <&"' });
    assert.equal(user.value, credentials.username);
    assert.equal(password.value, credentials.password);
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
  } finally { p.close(); }
});

test('password typing does not filter suggestions; keyboard and outside clicks dismiss the picker', async () => {
  const p = page('<input type="password"><button id="outside">Elsewhere</button>', [{ name: 'Login', username: 'alice' }]);
  try {
    const password = p.document.querySelector('input');
    password.focus();
    await settle();
    password.value = 'not-a-username';
    password.dispatchEvent(new p.window.Event('input', { bubbles: true }));
    assert.equal(p.picker().querySelectorAll('button').length, 1);
    password.dispatchEvent(new p.window.KeyboardEvent('keydown', { key: 'ArrowDown', bubbles: true }));
    assert.equal(p.picker().activeElement, p.picker().querySelector('button'));
    p.picker().activeElement.dispatchEvent(new p.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, composed: true }));
    await settle();
    assert.equal(p.document.activeElement, password);
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
    password.dispatchEvent(new p.window.Event('input', { bubbles: true }));
    await settle();
    p.document.querySelector('#outside').dispatchEvent(new p.window.Event('pointerdown', { bubbles: true }));
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
    assert.equal(p.calls.every((message) => message.action === 'suggestions'), true);
  } finally { p.close(); }
});

test('inline picker handles dynamic shadow forms and rejects signup/search fields', async () => {
  const p = page('<input type="search"><form><input name="username"><input type="password" autocomplete="new-password"></form><div id="host"></div>', [{ name: 'Login', username: 'alice' }]);
  try {
    for (const field of p.document.querySelectorAll('input')) {
      field.focus();
      await settle();
      assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
    }
    assert.equal(p.calls.length, 0);
    const shadow = p.document.querySelector('#host').attachShadow({ mode: 'open' });
    shadow.innerHTML = '<form><input name="username"><input type="password"></form>';
    shadow.querySelector('input').focus();
    await settle();
    assert.equal(p.picker().querySelectorAll('button').length, 1);
    shadow.querySelector('form').remove();
    await settle();
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
  } finally { p.close(); }
});

test('late suggestion responses cannot reopen a dismissed picker', async () => {
  const p = page('<input name="username"><button>Elsewhere</button>');
  try {
    let resolve;
    p.window.browser.runtime.sendMessage = () => new Promise((done) => { resolve = done; });
    p.document.querySelector('input').focus();
    p.document.querySelector('button').focus();
    await settle();
    resolve({ ok: true, data: [{ name: 'Login', username: 'alice' }] });
    await settle();
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
  } finally { p.close(); }
});

test('Escape cancels pending suggestions and selection errors remain retryable', async () => {
  const p = page('<input name="username">');
  try {
    let resolve;
    p.window.browser.runtime.sendMessage = () => new Promise((done) => { resolve = done; });
    const field = p.document.querySelector('input');
    field.focus();
    field.dispatchEvent(new p.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    resolve({ ok: true, data: [{ name: 'Login', username: 'alice' }] });
    await settle();
    assert.equal(p.document.querySelector('[data-rpws-inline]'), null);
    p.window.browser.runtime.sendMessage = async (message) => message.action === 'suggestions'
      ? { ok: true, data: [{ name: 'Login', username: 'alice' }] }
      : { ok: false, error: 'This login no longer matches the page.' };
    field.dispatchEvent(new p.window.Event('input', { bubbles: true }));
    await settle();
    p.picker().querySelector('button').click();
    await settle();
    assert.match(p.picker().querySelector('[role="status"]').textContent, /no longer matches/);
    assert.equal(p.picker().querySelector('button').disabled, false);
    assert.equal(field.value, '');
  } finally { p.close(); }
});

test('uses the focused login form and leaves other forms alone', async () => {
  const p = page('<form><input name="username"><input type="password"></form><form id="chosen"><input name="username"><input type="password"></form>');
  try {
    p.document.querySelector('#chosen input').focus();
    assert.equal((await p.fill()).ok, true);
    assert.equal(p.document.querySelector('form input').value, '');
    assert.equal(p.document.querySelector('#chosen [type="password"]').value, credentials.password);
  } finally { p.close(); }
});

test('rejects navigation, foreign senders, signup/reset and unavailable fields', async () => {
  for (const html of [
    '<input type="password" autocomplete="new-password">',
    '<form><input name="username"><input type="password"><input type="password"></form>',
    '<input type="password" disabled>',
    '<input type="password" readonly>',
    '<div hidden><input type="password"></div>',
    '<input type="search">',
  ]) {
    const p = page(html);
    try {
      assert.equal((await p.fill()).ok, false);
      assert.equal([...p.document.querySelectorAll('input')].every((input) => !input.value), true);
    } finally { p.close(); }
  }
  const p = page('<input type="password">');
  try {
    assert.equal(p.fill(credentials, { id: 'other-extension' }), undefined);
    assert.equal((await p.fill({ ...credentials, origin: 'https://other.example' })).ok, false);
    assert.equal(p.document.querySelector('input').value, '');
  } finally { p.close(); }
});
