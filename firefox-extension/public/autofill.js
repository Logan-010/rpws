// Installed only after a user requests autofill. Repeated requests reuse the listener.
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

  browser.runtime.onMessage.addListener((message, sender) => {
    if (sender.id !== browser.runtime.id || message?.type !== 'rpws-fill') return;
    if (location.origin !== message.origin || !['http:', 'https:'].includes(location.protocol)) {
      return Promise.resolve({ ok: false, error: 'The page changed since autofill was requested.' });
    }
    const fields = inputs(document).filter(visible);
    let focused = document.activeElement;
    while (focused?.shadowRoot?.activeElement) focused = focused.shadowRoot.activeElement;
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
    if (user && message.username) setValue(user, message.username);
    if (password) setValue(password, message.password);
    return Promise.resolve({ ok: true, usernameFilled: !!(user && message.username), passwordFilled: !!password });
  });
})();
