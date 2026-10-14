import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';

export default defineConfig({
  plugins: [tailwindcss(), {
    name: 'extension-safe-wasm-global',
    transform(code, id) {
      if (!id.endsWith('/wasm/web.js')) return;
      // js-sys includes a legacy `Function("return this")` global fallback.
      // Firefox 115+ has globalThis; replace only this generated fallback,
      // without enabling JavaScript eval in the extension CSP.
      return code.replace(
        'new Function(getStringFromWasm0(arg0, arg1))',
        `(() => {
          if (getStringFromWasm0(arg0, arg1) !== 'return this') {
            throw new Error('Dynamic JavaScript is unavailable in this extension');
          }
          return () => globalThis;
        })()`,
      );
    },
  }],
  base: './',
  build: {
    minify: false,
    target: 'firefox115',
    assetsInlineLimit: 0,
    rollupOptions: {
      input: ['popup.html', 'background.html'],
    },
  },
});
