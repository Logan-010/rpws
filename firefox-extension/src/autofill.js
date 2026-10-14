export function loginOrigin(value) {
  try {
    const url = new URL(value);
    return ['http:', 'https:'].includes(url.protocol) ? url.origin : null;
  } catch { return null; }
}

export async function activeLoginTab() {
  const [tab] = await browser.tabs.query({ active: true, currentWindow: true });
  if (!tab?.id || !loginOrigin(tab.url)) throw new Error('Open an HTTP or HTTPS login page first.');
  return tab;
}

export async function fillTab(tab, record) {
  const origin = loginOrigin(record.url);
  if (!origin || origin !== loginOrigin(tab.url)) {
    throw new Error('This login URL must match the current page’s exact origin (scheme, hostname, and port).');
  }
  try {
    await browser.tabs.executeScript(tab.id, { file: 'autofill.js', frameId: 0 });
    const result = await browser.tabs.sendMessage(tab.id, {
      type: 'rpws-fill', origin, username: record.username, password: record.password,
    }, { frameId: 0 });
    if (!result?.ok) throw new Error(result?.error || 'No login fields found on this page.');
    return result;
  } catch (error) {
    throw new Error(`Could not autofill: ${String(error?.message || error)}`);
  }
}
