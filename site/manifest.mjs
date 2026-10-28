// Regenerates asset-manifest.json from public/. `npm run check` fails when the
// manifest no longer matches, and deployment read-backs compare against it.
import { createHash } from 'node:crypto';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, 'public');
const entries = await readdir(root, { recursive: true, withFileTypes: true });
const paths = entries
  .filter(entry => entry.isFile())
  .map(entry => relative(root, resolve(entry.parentPath, entry.name)).split(sep).join('/'))
  .sort((a, b) => a.localeCompare(b));
const manifest = [];
for (const path of paths) {
  const bytes = await readFile(resolve(root, path));
  manifest.push({ path, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
}
await writeFile(resolve(here, 'asset-manifest.json'), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`asset-manifest.json: ${manifest.length} files`);
