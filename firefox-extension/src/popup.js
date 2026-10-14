import './style.css';
import Fuse from 'fuse.js';

const app = document.querySelector('#app');
const status = document.querySelector('#status');
const fileInput = document.querySelector('#import-file');
let loaded = false;
let entries = {};
let busy = false;
let hydrating = true;
let uiRevision = 0;
const initialUi = () => ({ view: 'setup', search: '', expanded: [], drafts: {}, scroll: 0 });
let ui = initialUi();
let searchIndex;
let copyOpener = null;

function remember() {
  if (hydrating) return;
  const form = app.querySelector('form');
  if (form) {
    ui.drafts[form.id] = Object.fromEntries([...form.elements]
      .filter((field) => field.name).map((field) => [field.name, field.value]));
  }
  const search = app.querySelector('#search');
  if (search) ui.search = search.value;
  const expanded = new Set(ui.expanded);
  app.querySelectorAll('details[data-name]').forEach((details) => {
    if (details.open) expanded.add(details.dataset.name);
    else expanded.delete(details.dataset.name);
  });
  ui.expanded = [...expanded];
  ui.scroll = window.scrollY;
  const focused = document.activeElement;
  if (focused.matches?.('#copy-dialog [data-field]')) ui.copyField = focused.dataset.field;
  ui.focus = app.contains(focused) && (focused.name || focused.id) ? {
    name: focused.name, id: focused.id,
    start: focused.selectionStart, end: focused.selectionEnd,
  } : null;
  ui.notice = status.classList.contains('hidden') ? null : {
    text: status.textContent, className: status.className,
  };
  rpc('ui-state-set', { revision: uiRevision, state: ui }).catch(console.error);
}

function restoreDraft(formId) {
  const draft = ui.drafts[formId];
  const form = app.querySelector(`#${formId}`);
  if (!draft || !form) return;
  for (const field of form.elements) {
    if (field.name && Object.hasOwn(draft, field.name)) field.value = draft[field.name];
  }
}

function updateSecretField() {
  const mode = app.querySelector('#secret-mode');
  if (!mode) return;
  const importing = mode.value === 'import';
  const field = app.querySelector('#secret-field');
  const input = field.querySelector('input');
  field.classList.toggle('hidden', !importing);
  input.disabled = !importing;
  input.required = importing;
}

async function resetUi() {
  const saved = await rpc('ui-state');
  uiRevision = saved.revision;
  ui = initialUi();
  status.classList.add('hidden');
}

const escape = (value) => String(value ?? '').replace(/[&<>"']/g, (character) => ({
  '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
})[character]);

// Templates below contain only fixed markup and HTML-escaped field values.
// Parse into an inert document, then insert nodes rather than assigning HTML.
function render(element, html) {
  const document = new DOMParser().parseFromString(html, 'text/html');
  element.replaceChildren(...document.body.childNodes);
}

async function rpc(action, data = {}) {
  const response = await browser.runtime.sendMessage({ type: 'rpws', action, ...data });
  if (!response?.ok) throw new Error(response?.error || 'The background page did not respond.');
  return response.data;
}

function notify(text, error = false) {
  status.textContent = text;
  status.className = `mx-6 mt-4 rounded-lg border p-3 text-sm ${error
    ? 'border-white bg-neutral-900 text-white font-semibold'
    : 'border-neutral-700 bg-neutral-950 text-neutral-200'}`;
  const modalStatus = app.querySelector('#copy-status');
  if (modalStatus) modalStatus.textContent = text;
}

async function run(task) {
  if (busy) return;
  busy = true;
  const buttons = [...document.querySelectorAll('button')].filter((button) => !button.disabled);
  buttons.forEach((button) => { button.disabled = true; });
  app.setAttribute('aria-busy', 'true');
  try { await task(); }
  catch (error) { notify(String(error?.message || error), true); }
  finally {
    busy = false;
    app.removeAttribute('aria-busy');
    buttons.forEach((button) => { if (button.isConnected) button.disabled = false; });
    if (copyOpener?.isConnected) copyOpener.focus();
    copyOpener = null;
    remember();
  }
}

function addressField(addrs = [], required = true) {
  return `<label>Bootstrap addresses
    <textarea name="addresses" rows="5" ${required ? 'required' : ''} spellcheck="false" placeholder="/dns4/peer.example/tcp/443/wss/p2p/&lt;peer-id&gt;">${escape(addrs.join('\n'))}</textarea>
  </label><p class="hint mt-2">One libp2p multiaddress per line. Use WebSocket peers (/ws or /wss), ending with /p2p/&lt;peer-id&gt;.</p>`;
}

function setup() {
  ui.view = 'setup';
  loaded = false;
  entries = {};
  render(app, `<section class="space-y-5">
    <div><h2 class="text-lg font-semibold">Welcome to your vault</h2><p class="mt-2 text-sm text-neutral-400">Add your bootstrap peers to create a save. It will be loaded automatically next time.</p></div>
    <form id="setup-form" class="space-y-4">
      <label>Vault secret
        <select id="secret-mode" name="secretMode" class="mt-1 w-full rounded-lg border border-neutral-700 bg-neutral-950 px-3 py-2 text-sm text-white">
          <option value="generate">Generate a new secret</option>
          <option value="import">Import an existing secret (base64)</option>
        </select>
      </label>
      <p class="hint">Generate creates a random 32-byte secret. Import uses an existing vault's secret with a new peer identity.</p>
      <label id="secret-field" class="hidden">Existing secret (base64)
        <input name="secret" type="password" disabled autocomplete="off" spellcheck="false" autocapitalize="off" placeholder="Base64-encoded 32-byte secret">
      </label>
      ${addressField()}<button class="primary w-full" type="submit">Create vault</button>
    </form>
    <div class="border-t border-neutral-800 pt-4"><p class="hint mb-3">Already have an RPWS save?</p><button class="secondary w-full" data-action="import">Import save</button></div>
  </section>`);
  restoreDraft('setup-form');
  updateSecretField();
}

async function vault() {
  entries = await rpc('list');
  searchIndex = new Fuse(Object.entries(entries).map(([name, record]) => ({ name, record })), {
    keys: [
      { name: 'name', weight: 0.5 },
      { name: 'record.username', weight: 0.25 },
      { name: 'record.url', weight: 0.2 },
      { name: 'record.extra', weight: 0.05 },
    ],
    threshold: 0.4,
    ignoreLocation: true,
  });
  loaded = true;
  ui.view = 'vault';
  render(app, `<div class="mb-4 flex items-center justify-between"><h2 class="text-lg font-semibold">Passwords <span class="text-sm text-neutral-400">(${Object.keys(entries).length})</span></h2>
    <div class="flex gap-2"><button class="secondary" data-action="refresh">Refresh</button><button class="primary" data-action="add">Add</button></div></div>
    <label class="mb-4"><span class="sr-only">Fuzzy search passwords</span><input id="search" type="search" placeholder="Fuzzy search names, usernames, URLs, or notes" autocomplete="off"></label>
    <div id="entries" class="space-y-3"></div>`);
  app.querySelector('#search').value = ui.search;
  renderEntries(ui.search);
}

function renderEntries(search) {
  const filtered = search.trim()
    ? searchIndex.search(search.trim()).map(({ item }) => [item.name, item.record])
    : Object.entries(entries).sort(([a], [b]) => a.localeCompare(b));
  render(document.querySelector('#entries'), filtered.length ? filtered.map(([name, record]) => `
    <article class="panel">
      <h3 class="break-words font-semibold">${escape(name)}</h3>
      <p class="mt-1 break-words text-sm text-neutral-400">${escape(record.username || 'No username')}</p>
      ${record.url ? `<p class="mt-1 break-words text-xs text-neutral-300">${escape(record.url)}</p>` : ''}
      <details class="mt-3" data-name="${escape(name)}" ${ui.expanded.includes(name) ? 'open' : ''}><summary class="cursor-pointer text-sm text-neutral-300">Details</summary>
        <div class="mt-3 space-y-2">
          <label>Password<input type="password" value="${escape(record.password)}" readonly aria-label="Password for ${escape(name)}"></label>
          <button class="secondary" data-action="reveal">Show</button>
          ${record.extra ? `<p class="whitespace-pre-wrap break-words text-sm text-neutral-300">${escape(record.extra)}</p>` : ''}
          <p class="hint">Updated ${escape(new Date(record.timestamp * 1000).toLocaleString())}</p>
        </div>
      </details>
      <div class="mt-3 flex flex-wrap gap-2"><button class="secondary" data-action="open" data-name="${escape(name)}" ${record.url.trim() ? '' : 'disabled'}>Open</button><button class="secondary" data-action="copy" data-name="${escape(name)}">Copy</button><button class="secondary" data-action="edit" data-name="${escape(name)}">Edit</button><button class="danger" data-action="remove" data-name="${escape(name)}">Remove</button></div>
    </article>`).join('') : `<div class="panel text-center text-sm text-neutral-400">${Object.keys(entries).length ? 'No matching passwords.' : 'Your vault is empty. Add your first password.'}</div>`);
}

function copyFields(name) {
  const record = entries[name];
  return [
    { key: 'name', label: 'Name', value: name },
    { key: 'url', label: 'URL', value: record.url },
    { key: 'username', label: 'Username', value: record.username },
    { key: 'password', label: 'Password', value: record.password },
    { key: 'extra', label: 'Notes', value: record.extra },
    { key: 'timestamp', label: 'Updated time', value: new Date(record.timestamp * 1000).toLocaleString() },
  ];
}

function closeCopy() {
  const name = ui.copyName;
  delete ui.copyName;
  delete ui.copyField;
  const dialog = app.querySelector('#copy-dialog');
  if (dialog?.open) dialog.close();
  dialog?.remove();
  const opener = [...app.querySelectorAll('[data-action="copy"]')].find((button) => button.dataset.name === name);
  if (busy) copyOpener = opener;
  else opener?.focus();
  remember();
}

function openCopy(name) {
  const selectedField = ui.copyName === name ? ui.copyField : 'name';
  const dialog = document.createElement('dialog');
  dialog.id = 'copy-dialog';
  dialog.className = 'copy-dialog';
  dialog.setAttribute('aria-labelledby', 'copy-title');
  render(dialog, `<h2 id="copy-title" class="text-lg font-semibold">Copy to clipboard</h2>
    <p class="hint mt-1 break-words">${escape(name)}</p>
    <div class="mt-4 space-y-2">${copyFields(name).map((field) => `
      <button class="copy-field secondary w-full text-left" data-action="copy-field" data-name="${escape(name)}" data-field="${field.key}">
        <span class="block">Copy ${field.label}</span>
        <span class="mt-1 block truncate text-xs text-neutral-400">${field.key === 'password' ? '••••••••' : escape(field.value || '(empty)')}</span>
      </button>`).join('')}</div>
    <p id="copy-status" class="mt-3 text-sm text-white" role="status" aria-live="polite"></p>
    <button class="secondary mt-4 w-full" data-action="close-copy">Cancel</button>`);
  ui.copyName = name;
  app.append(dialog);
  dialog.addEventListener('cancel', (event) => {
    event.preventDefault();
    if (!busy) closeCopy();
  });
  dialog.addEventListener('keydown', (event) => {
    if (busy || !['ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const fields = [...dialog.querySelectorAll('.copy-field')];
    const index = fields.indexOf(document.activeElement);
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? fields.length - 1
      : (index + (event.key === 'ArrowDown' ? 1 : -1) + fields.length) % fields.length;
    fields[next].focus();
  });
  dialog.showModal();
  const fields = [...dialog.querySelectorAll('.copy-field')];
  (fields.find((button) => button.dataset.field === selectedField) || fields[0]).focus();
}

function editor(name) {
  const sameEditor = ui.editorName === name && ui.editorEditing === (name !== undefined);
  if (!sameEditor) delete ui.drafts['entry-form'];
  ui.view = 'editor';
  ui.editorName = name;
  ui.editorEditing = name !== undefined;
  const editing = name !== undefined;
  const record = editing ? entries[name] : {};
  render(app, `<h2 class="mb-4 text-lg font-semibold">${editing ? 'Edit' : 'Add'} password</h2>
    <form id="entry-form" data-editing="${editing}" class="space-y-3">
      <label>Name<input name="name" value="${escape(name)}" required ${editing ? 'readonly' : ''} autocomplete="off"></label>
      ${editing ? '<p class="hint">Entry names are fixed. Create a new entry to use a different name.</p>' : ''}
      <label>URL<input name="url" value="${escape(record.url)}" placeholder="https://example.com" autocomplete="off"></label>
      <label>Username<input name="username" value="${escape(record.username)}" autocomplete="off"></label>
      <label>Password<input name="password" type="password" value="${escape(record.password)}" required autocomplete="new-password"></label>
      <button type="button" class="secondary" data-action="reveal">Show password</button>
      <label>Notes<textarea name="extra" rows="3">${escape(record.extra)}</textarea></label>
      <div class="flex gap-2 pt-2"><button class="primary" type="submit">Save password</button><button class="secondary" type="button" data-action="back">Cancel</button></div>
    </form>`);
  restoreDraft('entry-form');
  if (!hydrating) app.querySelector(editing ? '[name="username"]' : '[name="name"]').focus();
}

async function settings() {
  const state = await rpc('state');
  ui.view = 'settings';
  render(app, `<div class="mb-4 flex items-center justify-between"><h2 class="text-lg font-semibold">Settings</h2><button class="secondary" data-action="back">Back</button></div>
    ${state.loaded ? `<form id="addresses-form" class="panel space-y-4">${addressField(state.addresses, false)}<p class="hint">The list may be empty if you do not want bootstrap peers.</p><button class="primary" type="submit">Save bootstrap addresses</button></form>` : '<p class="hint">Create or import a save to configure bootstrap peers.</p>'}
    ${state.loaded ? `<section class="panel mt-4"><h3 class="font-semibold">Vault secret</h3><p class="hint mt-2">Copy your 32-byte secret as base64 to use it when setting up another peer. Keep it private.</p><button class="secondary mt-3" data-action="copy-secret">Copy secret as base64</button></section>` : ''}
    <section class="panel mt-4"><h3 class="font-semibold">Save file</h3>
      <p class="hint mt-2">Your save contains your identity, vault secret, bootstrap addresses, and cached records. Keep exports private; anyone with the save can access the vault.</p>
      <div class="mt-4 flex gap-2">${state.loaded ? '<button class="secondary" data-action="export">Export save</button>' : ''}<button class="secondary" data-action="import">Import save</button></div>
      <p class="hint mt-3">Import replaces the current vault. Export it first if you want to keep a backup.</p>
    </section>
    ${state.loaded ? `<section class="panel mt-4"><h3 class="font-semibold">Unload vault</h3><p class="hint mt-2">Remove the cached save from this Firefox profile and return to setup.</p><button class="danger mt-3" data-action="unload">Unload save</button></section>` : ''}`);
  restoreDraft('addresses-form');
}

function confirmUnload() {
  ui.view = 'unload';
  render(app, `<section class="panel space-y-4"><h2 class="text-lg font-semibold">Unload the current save?</h2>
    <p class="text-sm font-semibold text-white">Unsaved data will be lost. This also removes the locally cached save, including your identity, secret, and cached passwords.</p>
    <p class="text-sm text-neutral-400">Export a save first to keep a backup. You will return to setup and can create or import a save again.</p>
    <div class="flex flex-wrap gap-2"><button class="secondary" data-action="export">Export save</button><button class="danger" data-action="confirm-unload">Unload save</button><button class="secondary" data-action="cancel-unload">Cancel</button></div></section>`);
}

function confirmRemoval(name) {
  ui.view = 'remove';
  ui.removeName = name;
  render(app, `<section class="panel space-y-4"><h2 class="text-lg font-semibold">Remove ${escape(name)}?</h2>
    <p class="text-sm text-neutral-400">This removes the local name mapping and stops advertising it. Copies on other peers are not deleted.</p>
    <div class="flex gap-2"><button class="danger" data-action="delete" data-name="${escape(name)}">Remove password</button><button class="secondary" data-action="back">Cancel</button></div></section>`);
}

document.querySelector('#settings').addEventListener('click', () => { remember(); run(settings); });
app.addEventListener('input', (event) => {
  remember();
  if (event.target.id === 'search') renderEntries(event.target.value);
});
app.addEventListener('change', (event) => {
  if (event.target.id === 'secret-mode') {
    updateSecretField();
    if (event.target.value === 'import') app.querySelector('[name="secret"]').focus();
  }
  remember();
});
app.addEventListener('toggle', remember, true);
app.addEventListener('focusin', remember);
app.addEventListener('select', remember);
window.addEventListener('scroll', remember);
window.addEventListener('pagehide', remember);
app.addEventListener('submit', (event) => {
  event.preventDefault();
  const form = event.target;
  const fields = new FormData(form);
  run(async () => {
    if (form.id === 'entry-form') {
      const name = fields.get('name');
      if (form.dataset.editing === 'false' && Object.hasOwn(entries, name)) throw new Error('An entry with this name already exists.');
      await rpc('put', { name, payload: {
        url: fields.get('url'), username: fields.get('username'), password: fields.get('password'),
        extra: fields.get('extra'), timestamp: Math.floor(Date.now() / 1000),
      } });
      delete ui.drafts['entry-form'];
      await vault();
      notify('Password saved to your local vault.');
    } else {
      const action = form.id === 'setup-form' ? 'setup' : 'addresses';
      await rpc(action, {
        addresses: fields.get('addresses').split('\n'),
        ...(action === 'setup' ? { secretMode: fields.get('secretMode'), secret: fields.get('secret') } : {}),
      });
      if (action === 'setup') await resetUi();
      else delete ui.drafts['addresses-form'];
      await vault();
      notify(action === 'setup' ? 'Vault created and cached locally.' : 'Bootstrap addresses updated.');
    }
  });
});

app.addEventListener('click', (event) => {
  const button = event.target.closest('button[data-action]');
  if (!button || busy) return;
  const { action, name } = button.dataset;
  if (action === 'reveal') {
    const input = button.closest('form, details').querySelector('input[type="password"], input[data-revealed]');
    const show = input.type === 'password';
    input.type = show ? 'text' : 'password';
    input.dataset.revealed = 'true';
    button.textContent = show ? 'Hide password' : 'Show password';
    return;
  }
  if (action === 'add' || action === 'edit') { editor(action === 'edit' ? name : undefined); remember(); return; }
  if (action === 'remove') { confirmRemoval(name); remember(); return; }
  if (action === 'unload') { confirmUnload(); remember(); return; }
  if (action === 'import') { fileInput.click(); return; }
  if (action === 'copy') { openCopy(name); remember(); return; }
  if (action === 'close-copy') { closeCopy(); return; }
  run(async () => {
    if (action === 'cancel-unload') await settings();
    if (action === 'confirm-unload') {
      await rpc('unload');
      await resetUi();
      fileInput.value = '';
      setup();
      notify('Save unloaded and local cache cleared.');
    }
    if (action === 'back') {
      if (ui.view === 'editor') delete ui.drafts['entry-form'];
      if (ui.view === 'import') delete ui.pendingImport;
      if (loaded) await vault(); else setup();
    }
    if (action === 'refresh') {
      notify('Syncing passwords with peers…');
      const count = await rpc('sync');
      await vault();
      notify(`Vault synced. ${count} password${count === 1 ? '' : 's'} added or updated.`);
    }
    if (action === 'delete') { await rpc('delete', { name }); await vault(); notify('Password removed.'); }
    if (action === 'open') {
      let url;
      try { url = new URL(entries[name].url); } catch { throw new Error('This entry needs a valid URL to open.'); }
      if (!['http:', 'https:'].includes(url.protocol)) throw new Error('Use an http:// or https:// URL for this entry.');
      await browser.tabs.create({ url: url.href });
    }
    if (action === 'copy-field') {
      const field = copyFields(name).find((field) => field.key === button.dataset.field);
      if (!field) throw new Error('Unknown clipboard field.');
      await navigator.clipboard.writeText(field.value);
      closeCopy();
      notify(`${field.label} copied.`);
    }
    if (action === 'copy-secret') {
      await navigator.clipboard.writeText(await rpc('secret'));
      notify('Vault secret copied as base64.');
    }
    if (action === 'export') {
      const bytes = await rpc('export');
      const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: 'application/octet-stream' }));
      const link = document.createElement('a');
      link.href = url;
      link.download = `rpws-${new Date().toISOString().slice(0, 10)}.rpws`;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 60_000);
      notify('Save exported. Keep this file private.');
    }
  });
});

function confirmImport(name, bytes) {
  ui.view = 'import';
  ui.pendingImport = { name, bytes };
  render(app, `<section class="panel space-y-4"><h2 class="text-lg font-semibold">Import ${escape(name)}?</h2>
    <p class="text-sm text-neutral-400">${loaded ? 'This replaces your current vault. Export a backup from Settings first if needed.' : 'This loads the identity, peers, and cached passwords from the save.'}</p>
    <div class="flex gap-2"><button id="confirm-import" class="primary">Import save</button><button class="secondary" data-action="back">Cancel</button></div></section>`);
  document.querySelector('#confirm-import').addEventListener('click', () => run(async () => {
    await rpc('import', { bytes });
    await resetUi();
    await vault();
    notify('Save imported and cached locally.');
  }));
}

fileInput.addEventListener('change', () => {
  const file = fileInput.files[0];
  fileInput.value = '';
  if (!file) return;
  run(async () => {
    const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
    // Keep the existing vault intact until the user explicitly confirms replacement.
    confirmImport(file.name, bytes);
  });
});

run(async () => {
  const state = await rpc('state');
  const saved = await rpc('ui-state');
  uiRevision = saved.revision;
  ui = saved.state || initialUi();
  const view = ui.view;
  const focus = ui.focus;
  const scroll = ui.scroll;
  loaded = state.loaded;
  if (loaded) {
    entries = await rpc('list');
    if (view === 'settings') await settings();
    else if (view === 'editor' && (!ui.editorEditing || Object.hasOwn(entries, ui.editorName))) editor(ui.editorEditing ? ui.editorName : undefined);
    else if (view === 'unload') confirmUnload();
    else if (view === 'remove' && Object.hasOwn(entries, ui.removeName)) confirmRemoval(ui.removeName);
    else if (view === 'import' && ui.pendingImport) confirmImport(ui.pendingImport.name, ui.pendingImport.bytes);
    else await vault();
  } else if (view === 'settings') await settings();
  else if (view === 'import' && ui.pendingImport) confirmImport(ui.pendingImport.name, ui.pendingImport.bytes);
  else setup();
  if (ui.notice) {
    status.textContent = ui.notice.text;
    status.className = ui.notice.className;
  }
  if (ui.view === 'vault' && ui.copyName && Object.hasOwn(entries, ui.copyName)) openCopy(ui.copyName);
  else delete ui.copyName;
  if (focus && !ui.copyName) {
    const field = [...app.querySelectorAll('input, textarea, select')].find((element) =>
      focus.name ? element.name === focus.name : element.id === focus.id);
    if (field && !field.disabled) {
      field.focus({ preventScroll: true });
      if (focus.start != null && ['text', 'password', 'search', 'textarea'].includes(field.type)) {
        field.setSelectionRange(focus.start, focus.end);
      }
    }
  }
  window.scrollTo(0, scroll || 0);
  hydrating = false;
  if (state.error) notify(state.error, true);
});
