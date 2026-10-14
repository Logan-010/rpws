import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { JSDOM } from 'jsdom';
import { fillTab, loginOrigin } from '../src/autofill.js';

const source = await readFile(new URL('../public/autofill.js', import.meta.url), 'utf8');
const credentials = { type: 'rpws-fill', origin: 'https://example.com', username: 'alice<&"', password: 'secret<&"' };

function page(html) {
  const dom = new JSDOM(html, { runScripts: 'outside-only', url: 'https://example.com/login' });
  const { window } = dom;
  const listeners = [];
  window.browser = { runtime: { id: 'rpws-test', onMessage: { addListener(fn) { listeners.push(fn); } } } };
  // jsdom has no layout. Supply a rectangle; CSS visibility is still checked.
  window.HTMLInputElement.prototype.getClientRects = () => [{ width: 100, height: 20 }];
  window.eval(source);
  return { window, document: window.document, listeners,
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
    assert.equal((await p.fill()).passwordFilled, true);
    assert.equal(shadow.querySelector('[type="password"]').value, credentials.password);
    assert.equal(shadow.querySelector('[name="username"]').value, credentials.username);
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
