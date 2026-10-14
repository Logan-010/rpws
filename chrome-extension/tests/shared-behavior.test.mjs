import { readFile } from 'node:fs/promises';

// Run the exact shared UI/content-script regression suites with this package's
// dependencies, including when Firefox's node_modules has not been installed.
for (const name of ['popup-state', 'autofill']) {
  const url = new URL(`../../firefox-extension/tests/${name}.test.mjs`, import.meta.url);
  let source = await readFile(url, 'utf8');
  source = source.replaceAll('import.meta.url', JSON.stringify(url.href));
  source = source.replace(/^(import .+ from )'(\.\.?\/[^']+)'/gm,
    (_, prefix, path) => `${prefix}${JSON.stringify(new URL(path, url).href)}`);
  for (const dependency of ['jsdom', 'fuse.js']) {
    source = source.replace(new RegExp(`^(import .+ from )'${dependency.replace('.', '\\.')}'`, 'm'),
      (_, prefix) => `${prefix}${JSON.stringify(import.meta.resolve(dependency))}`);
  }
  await import(`data:text/javascript;base64,${Buffer.from(source).toString('base64')}`);
}
