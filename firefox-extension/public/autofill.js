// Shared by the inline picker and popup/shortcut autofill; install only once.
(() => {
  if (globalThis.rpwsAutofillInstalled) return;
  globalThis.rpwsAutofillInstalled = true;

  function inputs(root) {
    const result = [...root.querySelectorAll('input')];
    for (const element of root.querySelectorAll('*')) {
      if (element.shadowRoot) result.push(...inputs(element.shadowRoot));
    }
    return result;
  }

  function visible(input) {
    if (input.disabled || input.readOnly || input.type === 'hidden' || !input.getClientRects().length) return false;
    for (let element = input; element; element = element.parentElement || element.getRootNode().host) {
      const style = getComputedStyle(element);
      if (element.hidden || element.inert || style.display === 'none' || style.visibility === 'hidden' || style.opacity === '0') return false;
    }
    return true;
  }

  function autocomplete(input, token) {
    return input.autocomplete.toLowerCase().split(/\s+/).includes(token);
  }

  function username(input) {
    if (!['text', 'email', 'tel'].includes(input.type)) return false;
    return autocomplete(input, 'username') || autocomplete(input, 'email') || input.type === 'email'
      || /user|login|email|e-mail|account|identifier/i.test(`${input.name} ${input.id} ${input.getAttribute('aria-label') || ''}`);
  }

  function setValue(input, value) {
    input.focus();
    // Use the native setter so controlled React/Vue forms observe the change.
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    input.dispatchEvent(new Event('change', { bubbles: true, composed: true }));
  }

  let anchor = null;
  let host = null;
  let panel = null;
  let suggestions = [];
  let request = 0;
  let selecting = false;
  const anchorObserver = new MutationObserver(() => {
    if (anchor && !anchor.isConnected) closePicker();
  });

  function loginField(input) {
    if (!(input instanceof HTMLInputElement) || !visible(input) || autocomplete(input, 'new-password')) return false;
    const group = inputs(input.getRootNode()).filter((field) => field.form === input.form && visible(field));
    const passwords = group.filter((field) => field.type === 'password');
    if (passwords.length && !passwords.some((field) => autocomplete(field, 'current-password'))
      && (passwords.length > 1 || passwords.some((field) => autocomplete(field, 'new-password')))) return false;
    return input.type === 'password' || username(input)
      || (input.type === 'text' && passwords.length === 1);
  }

  function closePicker() {
    request++;
    anchorObserver.disconnect();
    host?.remove();
    host = panel = anchor = null;
    suggestions = [];
  }

  function positionPicker() {
    if (!host || !anchor) return;
    const rect = anchor.getBoundingClientRect();
    if (!anchor.isConnected || !visible(anchor) || rect.bottom < 0 || rect.top > innerHeight) {
      closePicker();
      return;
    }
    const width = Math.min(Math.max(rect.width, 280), innerWidth - 16);
    host.style.setProperty('width', `${width}px`, 'important');
    host.style.setProperty('left', `${Math.max(8, Math.min(rect.left, innerWidth - width - 8))}px`, 'important');
    const height = Math.min(panel.getBoundingClientRect().height || 240, innerHeight - 16);
    const below = rect.bottom + 4;
    const top = below + height <= innerHeight - 8 ? below : Math.max(8, rect.top - height - 4);
    host.style.setProperty('top', `${top}px`, 'important');
    panel.style.maxHeight = `${Math.max(40, innerHeight - top - 8)}px`;
  }

  async function selectLogin(login) {
    if (selecting || !anchor?.isConnected || !loginField(anchor)) return;
    selecting = true;
    const field = anchor;
    field.focus();
    panel.querySelectorAll('button').forEach((button) => { button.disabled = true; });
    try {
      const response = await browser.runtime.sendMessage({ type: 'rpws-inline', action: 'select', name: login.name });
      if (!response?.ok) throw new Error(response?.error || 'Could not autofill this login.');
      closePicker();
    } catch (error) {
      if (panel) {
        panel.querySelector('[role="status"]').textContent = String(error?.message || error);
        panel.querySelectorAll('button').forEach((button) => { button.disabled = false; });
      }
    } finally { selecting = false; }
  }

  function renderPicker() {
    if (!anchor || selecting) return;
    // Never inspect or filter by text typed in a password field.
    const query = anchor.type === 'password' ? '' : anchor.value.trim().toLocaleLowerCase();
    const matches = suggestions.filter((login) => !query || `${login.username} ${login.name}`.toLocaleLowerCase().includes(query));
    if (!matches.length) {
      host?.remove();
      host = panel = null;
      return;
    }
    if (!host) {
      host = document.createElement('div');
      host.dataset.rpwsInline = '';
      // Isolate picker styles and keep credential metadata out of the page DOM.
      const shadow = host.attachShadow({ mode: 'closed' });
      for (const [property, value] of Object.entries({ all: 'initial', position: 'fixed', display: 'block', 'z-index': '2147483647', margin: '0', padding: '0', border: '0' })) {
        host.style.setProperty(property, value, 'important');
      }
      const style = document.createElement('style');
      style.textContent = `
        :host { color-scheme: dark; }
        section { box-sizing: border-box; overflow: auto; background: #171717; color: #fff; border: 1px solid #525252; border-radius: 8px; box-shadow: 0 6px 24px #0006; padding: 6px; font: 14px/1.4 system-ui, sans-serif; }
        h2 { font: 600 12px/1.4 system-ui, sans-serif; color: #a3a3a3; margin: 4px 8px 6px; }
        button { display: block; width: 100%; border: 0; border-radius: 4px; padding: 8px; background: transparent; color: #fff; text-align: left; cursor: pointer; font: inherit; }
        button:hover, button:focus-visible { background: #404040; outline: 2px solid #a3a3a3; outline-offset: -2px; }
        button:disabled { opacity: .6; cursor: wait; }
        span { display: block; overflow-wrap: anywhere; }
        .username { color: #a3a3a3; font-size: 12px; }
        [role=status]:not(:empty) { padding: 8px; font-size: 12px; }
      `;
      panel = document.createElement('section');
      panel.setAttribute('role', 'region');
      panel.setAttribute('aria-label', 'RPWS saved logins');
      shadow.append(style, panel);
      host.addEventListener('mousedown', (event) => event.preventDefault());
      panel.addEventListener('keydown', (event) => {
        const buttons = [...panel.querySelectorAll('button')];
        const index = buttons.indexOf(shadow.activeElement);
        if (event.key === 'Escape') {
          event.preventDefault();
          selecting = true;
          anchor?.focus();
          selecting = false;
          closePicker();
        } else if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault();
          const next = event.key === 'Home' ? 0 : event.key === 'End' ? buttons.length - 1
            : (index + (event.key === 'ArrowDown' ? 1 : -1) + buttons.length) % buttons.length;
          buttons[next]?.focus();
        }
      });
      document.documentElement.append(host);
    }
    const title = document.createElement('h2');
    title.textContent = 'RPWS · Choose a saved login';
    panel.replaceChildren(title);
    for (const login of matches) {
      const button = document.createElement('button');
      button.type = 'button';
      const name = document.createElement('span');
      name.textContent = login.name;
      const user = document.createElement('span');
      user.className = 'username';
      user.textContent = login.username || 'No username';
      button.append(name, user);
      button.addEventListener('click', () => selectLogin(login));
      panel.append(button);
    }
    const status = document.createElement('div');
    status.setAttribute('role', 'status');
    panel.append(status);
    positionPicker();
  }

  async function showPicker(field) {
    if (selecting) return;
    if (!loginField(field)) { closePicker(); return; }
    if (anchor === field) { renderPicker(); return; }
    closePicker();
    anchor = field;
    anchorObserver.observe(field.getRootNode(), { childList: true, subtree: true });
    const revision = request;
    try {
      const response = await browser.runtime.sendMessage({ type: 'rpws-inline', action: 'suggestions' });
      if (request !== revision || anchor !== field || !field.isConnected) return;
      suggestions = response?.ok && Array.isArray(response.data) ? response.data : [];
      renderPicker();
    } catch { if (request === revision) closePicker(); }
  }

  document.addEventListener('focusin', (event) => {
    if (event.composedPath().includes(host)) return;
    showPicker(event.composedPath()[0]);
  });
  document.addEventListener('input', (event) => {
    if (!selecting) showPicker(event.composedPath()[0]);
  });
  document.addEventListener('keydown', (event) => {
    if (event.composedPath()[0] !== anchor || selecting) return;
    if (event.key === 'Escape') { event.preventDefault(); closePicker(); }
    if (event.key === 'ArrowDown' && host) { event.preventDefault(); panel.querySelector('button')?.focus(); }
  });
  document.addEventListener('pointerdown', (event) => {
    const path = event.composedPath();
    if (!path.includes(host) && !path.includes(anchor)) closePicker();
  }, true);
  document.addEventListener('focusout', () => {
    setTimeout(() => {
      let focused = document.activeElement;
      while (focused?.shadowRoot?.activeElement) focused = focused.shadowRoot.activeElement;
      if (focused !== anchor && focused !== host && !selecting) closePicker();
    }, 0);
  });
  window.addEventListener('scroll', positionPicker, true);
  window.addEventListener('resize', positionPicker);
  window.addEventListener('pagehide', closePicker);
  new MutationObserver(() => { if (anchor && !anchor.isConnected) closePicker(); })
    .observe(document.documentElement, { childList: true, subtree: true });
  // Also cover sites that autofocus a login field before document_idle.
  let initialFocus = document.activeElement;
  while (initialFocus?.shadowRoot?.activeElement) initialFocus = initialFocus.shadowRoot.activeElement;
  if (initialFocus instanceof HTMLInputElement) showPicker(initialFocus);

  browser.runtime.onMessage.addListener((message, sender) => {
    if (sender.id !== browser.runtime.id || message?.type !== 'rpws-fill') return;
    if (selecting && (!anchor?.isConnected || !loginField(anchor))) {
      return Promise.resolve({ ok: false, error: 'The selected login field changed. Focus a login field and try again.' });
    }
    if (location.origin !== message.origin || !['http:', 'https:'].includes(location.protocol)) {
      return Promise.resolve({ ok: false, error: 'The page changed since autofill was requested.' });
    }
    let fields = inputs(document).filter(visible);
    let focused = document.activeElement;
    while (focused?.shadowRoot?.activeElement) focused = focused.shadowRoot.activeElement;
    if (focused instanceof HTMLInputElement && loginField(focused)) {
      fields = fields.filter((input) => input.form === focused.form && input.getRootNode() === focused.getRootNode());
    }
    const passwords = fields.filter((input) => input.type === 'password' && !autocomplete(input, 'new-password'));
    const password = passwords.find((input) => input === focused || (input.form && input.form === focused?.form))
      || passwords.find((input) => autocomplete(input, 'current-password')) || passwords[0];
    // Multiple unmarked passwords usually indicate signup/reset, not a login.
    if (password && !autocomplete(password, 'current-password')
      && fields.filter((input) => input.type === 'password' && input.form === password.form).length > 1) {
      return Promise.resolve({ ok: false, error: 'Select a login form with a current-password field, rather than a signup or password-reset form.' });
    }
    const candidates = fields.filter((input) => password ? input.form === password.form && input.getRootNode() === password.getRootNode() : true);
    const user = candidates.find((input) => username(input) && input === focused)
      || candidates.find((input) => autocomplete(input, 'username'))
      || candidates.find(username)
      || (password ? candidates.slice(0, candidates.indexOf(password)).reverse().find((input) => input.type === 'text') : null);
    if (!password && fields.some((input) => input.type === 'password')) {
      return Promise.resolve({ ok: false, error: 'No current-password login field found.' });
    }
    if (!password && !(user && message.username)) {
      return Promise.resolve({ ok: false, error: 'No visible login fields found. Open the login form and try again.' });
    }
    const wasSelecting = selecting;
    selecting = true;
    try {
      if (user && message.username) setValue(user, message.username);
      if (password) setValue(password, message.password);
      closePicker();
    } finally { selecting = wasSelecting; }
    return Promise.resolve({ ok: true, usernameFilled: !!(user && message.username), passwordFilled: !!password });
  });
})();
