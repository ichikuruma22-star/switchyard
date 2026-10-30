import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile, stat, readdir } from 'node:fs/promises';
import { dirname, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, 'public');
let localReferences = 0;
for (const name of ['index.html', '404.html']) {
  const html = await readFile(resolve(root, name), 'utf8');
  const allIds = [...html.matchAll(/\bid="([^"]+)"/g)].map(match => match[1]);
  const ids = new Set(allIds);
  assert.equal(ids.size, allIds.length, `${name}: duplicate IDs`);
  assert.match(html, /<html lang="en">/);
  assert.match(html, /name="viewport"/);
  assert.doesNotMatch(html, /\son[a-z]+\s*=/i, `${name}: inline event handler`);
  for (const [, target] of html.matchAll(/\b(?:src|href|aria-controls)="([^"]+)"/g)) {
    if (/^https:\/\//.test(target)) { new URL(target); continue; }
    if (target.startsWith('#')) { assert.ok(ids.has(target.slice(1)), `${name}: missing ${target}`); continue; }
    if (!target.startsWith('/')) { assert.ok(ids.has(target), `${name}: missing control ${target}`); continue; }
    const [path, fragment] = target.split('#');
    const file = resolve(root, `.${path === '/' ? '/index.html' : path}`);
    assert.ok((await stat(file)).isFile(), `${name}: missing ${target}`);
    // A same-site link to a section (/#availability) must name an ID on that page.
    if (fragment) assert.ok((await readFile(file, 'utf8')).includes(` id="${fragment}"`), `${name}: missing ${target}`);
    localReferences++;
  }
}
// Structured data must parse, and the FAQ it describes must be the one on the page.
const home = await readFile(resolve(root, 'index.html'), 'utf8');
const structured = JSON.parse(home.match(/<script type="application\/ld\+json">([\s\S]*?)<\/script>/)[1]);
const faq = structured['@graph'].find(item => item['@type'] === 'FAQPage').mainEntity.map(question => question.name);
const summaries = [...home.matchAll(/<summary>([^<]+)<\/summary>/g)].map(match => match[1]);
assert.deepEqual(faq, summaries, 'index.html: FAQ structured data does not match the visible questions');
const app = structured['@graph'].find(item => item['@type'] === 'SoftwareApplication');
assert.ok(home.includes(`href="${app.downloadUrl}"`), 'index.html: structured downloadUrl is not the page download link');
const css = await readFile(resolve(root, 'site.css'), 'utf8');
for (const [, target] of css.matchAll(/url\(['"]?(\/[^)'"\s]+)['"]?\)/g)) {
  assert.ok((await stat(resolve(root, `.${target}`))).isFile(), `CSS: missing ${target}`);
  localReferences++;
}
const config = JSON.parse(await readFile(resolve(here, 'wrangler.jsonc'), 'utf8'));
assert.equal(config.assets.directory, './public');
assert.equal(config.assets.not_found_handling, '404-page');
assert.ok(!config.main && !config.account_id, 'Static-only config must not contain a Worker entry point or account ID');
assert.equal(config.workers_dev, false);
assert.deepEqual(config.routes, [
  { pattern: 'switchya.com', custom_domain: true },
  { pattern: 'www.switchya.com', custom_domain: true },
], 'Only the two authorized custom domains may be configured');
const pkg = JSON.parse(await readFile(resolve(here, 'package.json'), 'utf8'));
assert.equal(pkg.devDependencies.wrangler, '4.156.0');
const assets = await readdir(root, { recursive: true, withFileTypes: true });
const files = assets.filter(entry => entry.isFile());
// Deployment read-backs are compared with the manifest, so it must describe
// exactly the files that would be published.
const manifest = new Map(JSON.parse(await readFile(resolve(here, 'asset-manifest.json'), 'utf8'))
  .map(entry => [entry.path, entry]));
for (const entry of files) {
  const path = relative(root, resolve(entry.parentPath, entry.name)).replaceAll('\\', '/');
  const listed = manifest.get(path);
  const bytes = await readFile(resolve(root, path));
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  assert.deepEqual(listed && [listed.bytes, listed.sha256], [bytes.length, sha256],
    `asset-manifest.json: ${path} is missing or stale; regenerate the manifest`);
  manifest.delete(path);
}
assert.deepEqual([...manifest.keys()], [], 'asset-manifest.json lists files that are not in public/');
console.log(`Site checks passed: 2 HTML pages, ${localReferences} local references, ${files.length} static files matching asset-manifest.json; Wrangler ${pkg.devDependencies.wrangler} pinned.`);
