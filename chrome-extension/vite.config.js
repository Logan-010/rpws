import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';

const local = (path) => fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  plugins: [tailwindcss(), {
    name: 'chrome-extension',
    enforce: 'pre',
    resolveId(source, importer) {
      if (importer?.endsWith('/firefox-extension/src/background.js') && source.startsWith('../wasm/')) {
        const [path, query] = source.split('?');
        return local(path.replace('../wasm/', './wasm/')) + (query ? `?${query}` : '');
      }
      // Resolve shared UI dependencies from this extension's installation.
      if (source === 'fuse.js') return local('./node_modules/fuse.js/dist/fuse.mjs');
      if (source === './style.css' && importer?.endsWith('/firefox-extension/src/popup.js')) return local('./src/style.css');
    },
    load(id) {
      if (id === local('./src/style.css')) {
        return `${readFileSync(local('../firefox-extension/src/style.css'), 'utf8')}\n@source "../../firefox-extension/src/popup.js";`;
      }
    },
    transform(code, id) {
      if (id.endsWith('/wasm/web.js')) {
        return code.replace('new Function(getStringFromWasm0(arg0, arg1))', `(() => {
          if (getStringFromWasm0(arg0, arg1) !== 'return this') {
            throw new Error('Dynamic JavaScript is unavailable in this extension');
          }
          return () => globalThis;
        })()`);
      }
      if (id.endsWith('/firefox-extension/src/popup.js')) return code.replace('this Firefox profile', 'this Chrome profile');
      if (id.endsWith('/firefox-extension/src/background.js')) {
        return `import { scheduleSnapshot } from ${JSON.stringify(local('./src/snapshot.js'))};\n${code.replace('setInterval(() => {', 'scheduleSnapshot(() => {')}`;
      }
    },
    generateBundle() {
      this.emitFile({ type: 'asset', fileName: 'icon.png', source: readFileSync(local('../firefox-extension/public/icon.png')) });
      const autofill = readFileSync(local('../firefox-extension/public/autofill.js'), 'utf8');
      // Use callback responses for compatibility across supported Chrome versions.
      const bridge = `const browser = {
        runtime: {
          id: chrome.runtime.id,
          sendMessage: (...args) => chrome.runtime.sendMessage(...args),
          onMessage: { addListener(handler) {
            chrome.runtime.onMessage.addListener((message, sender, respond) => {
              const result = handler(message, sender);
              if (result === undefined) return;
              Promise.resolve(result).then(respond, error => respond({ ok: false, error: String(error?.message || error) }));
              return true;
            });
          } },
        },
      };`;
      this.emitFile({ type: 'asset', fileName: 'autofill.js', source: `(() => {\n${bridge}\n${autofill}\n})();\n` });
    },
  }],
  base: './',
  build: {
    minify: false,
    target: 'chrome116',
    assetsInlineLimit: 0,
    rollupOptions: {
      input: { popup: local('./popup.html'), offscreen: local('./offscreen.html'), 'service-worker': local('./src/service-worker.js') },
      output: { entryFileNames: (chunk) => chunk.name === 'service-worker' ? 'service-worker.js' : 'assets/[name]-[hash].js' },
    },
  },
});
