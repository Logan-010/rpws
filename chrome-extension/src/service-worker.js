let creating;
const offscreenUrl = chrome.runtime.getURL('offscreen.html');

async function ensureVault() {
  if (creating) return creating;
  creating = (async () => {
    const contexts = await chrome.runtime.getContexts({
      contextTypes: ['OFFSCREEN_DOCUMENT'], documentUrls: [offscreenUrl],
    });
    if (!contexts.length) {
      await chrome.offscreen.createDocument({
        url: 'offscreen.html', reasons: ['WORKERS'],
        justification: 'Run the vault snapshot worker alongside the WASM peer network.',
      });
    }
  })();
  try { await creating; }
  finally { creating = null; }
}

async function vaultRequest(message, sender) {
  await ensureVault();
  return chrome.runtime.sendMessage({ type: 'rpws-vault', message, sender });
}

async function apiRequest(message) {
  const { args = [] } = message;
  switch (message.method) {
    case 'storage.get': return chrome.storage.local.get(...args);
    case 'storage.set': return chrome.storage.local.set(...args);
    case 'storage.remove': return chrome.storage.local.remove(...args);
    case 'tabs.query': return chrome.tabs.query(...args);
    case 'tabs.get': return chrome.tabs.get(...args);
    case 'tabs.sendMessage': return chrome.tabs.sendMessage(...args);
    case 'tabs.executeScript': {
      const [tabId, details] = args;
      return chrome.scripting.executeScript({
        target: { tabId, frameIds: [details.frameId ?? 0] }, files: [details.file],
      });
    }
    case 'action.openPopup':
      try { return await chrome.action.openPopup(); }
      catch {
        // openPopup is generally available starting in Chrome 127.
        return chrome.windows.create({ url: chrome.runtime.getURL('popup.html'), type: 'popup', width: 480, height: 640 });
      }
    default: throw new Error('Unknown vault browser API.');
  }
}

chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id) return;
  let operation;
  if (message?.type === 'rpws-api') {
    if (sender.tab || sender.url !== offscreenUrl) return;
    operation = apiRequest(message).then((data) => ({ ok: true, data }));
  } else if (['rpws', 'rpws-inline'].includes(message?.type)) {
    if (message.type === 'rpws' && sender.tab) {
      // Chrome's popup-window fallback has a tab; authenticate the packaged
      // popup URL before forwarding it as an extension UI sender.
      if (sender.url !== chrome.runtime.getURL('popup.html')) return;
      const { tab, ...uiSender } = sender;
      operation = vaultRequest(message, uiSender);
    } else operation = vaultRequest(message, sender);
  } else return;
  operation.then(respond, (error) => respond({ ok: false, error: String(error?.message || error) }));
  return true;
});

chrome.commands.onCommand.addListener((command) => {
  if (command !== 'autofill') return;
  vaultRequest({ type: 'rpws-command', command }, { id: chrome.runtime.id }).catch(console.error);
});

chrome.runtime.onStartup.addListener(() => { ensureVault().catch(console.error); });
chrome.runtime.onInstalled.addListener(() => { ensureVault().catch(console.error); });
