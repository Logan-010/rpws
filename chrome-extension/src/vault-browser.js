async function api(method, ...args) {
  const response = await chrome.runtime.sendMessage({ type: 'rpws-api', method, args });
  if (!response?.ok) throw new Error(response?.error || 'The service worker did not respond.');
  return response.data;
}

let commandHandler;
globalThis.browser = {
  storage: { local: {
    get: (...args) => api('storage.get', ...args),
    set: (...args) => api('storage.set', ...args),
    remove: (...args) => api('storage.remove', ...args),
  } },
  tabs: Object.fromEntries(['query', 'get', 'sendMessage', 'executeScript'].map((method) =>
    [method, (...args) => api(`tabs.${method}`, ...args)])),
  browserAction: { openPopup: () => api('action.openPopup') },
  commands: { onCommand: { addListener(handler) { commandHandler = handler; } } },
  runtime: {
    id: chrome.runtime.id,
    onMessage: { addListener(handler) {
      chrome.runtime.onMessage.addListener((envelope, sender, respond) => {
        // Only the service worker can forward authenticated original senders.
        if (sender.id !== chrome.runtime.id || sender.tab
          || (sender.url && sender.url !== chrome.runtime.getURL('service-worker.js'))
          || envelope?.type !== 'rpws-vault') return;
        if (envelope.message?.type === 'rpws-command') {
          commandHandler?.(envelope.message.command);
          respond({ ok: true });
          return;
        }
        const result = handler(envelope.message, envelope.sender);
        if (result === undefined) return;
        Promise.resolve(result).then(respond, (error) => respond({ ok: false, error: String(error?.message || error) }));
        return true;
      });
    } },
  },
};
