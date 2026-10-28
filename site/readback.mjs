// Reads back the live site after a deployment and writes the evidence file:
//
//   node readback.mjs <wrangler-version-id> "<what changed>" > deployment-readback-<date>-<label>.json
//
// Every file in asset-manifest.json must be served from switchya.com with the
// same bytes; www must serve the same home page; plain HTTP must redirect; an
// unknown path must return the custom 404; the global security headers must be
// present. The exit code is non-zero when anything fails.
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { request } from 'node:https';
import { request as httpRequest } from 'node:http';

const [versionId = null, change = null] = process.argv.slice(2);
const here = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(await readFile(resolve(here, 'asset-manifest.json'), 'utf8'));
const origin = 'https://switchya.com';
const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const fetchBytes = async url => {
  const response = await fetch(url, { cache: 'no-store' });
  return { response, bytes: Buffer.from(await response.arrayBuffer()) };
};
let failures = 0;
const fail = (condition, note) => { if (!condition) { failures++; console.error(`FAIL ${note}`); } };

const results = [];
for (const entry of manifest) {
  // Files whose names start with "_" configure the platform and are never served.
  if (entry.path.startsWith('_')) continue;
  const url = `${origin}/${entry.path}`;
  const { response, bytes } = await fetchBytes(url);
  const matches = response.status === 200 && bytes.length === entry.bytes && sha256(bytes) === entry.sha256;
  fail(matches, `${url} does not match asset-manifest.json`);
  results.push({ url, finalUrl: response.url, status: response.status, bytes: bytes.length, sha256: sha256(bytes), matchesManifest: matches });
}

const home = await fetchBytes(`${origin}/`);
const index = manifest.find(entry => entry.path === 'index.html');
fail(home.response.status === 200 && sha256(home.bytes) === index.sha256, 'https://switchya.com/ does not serve index.html');
const www = await fetchBytes('https://www.switchya.com/');
fail(www.response.status === 200 && sha256(www.bytes) === index.sha256, 'https://www.switchya.com/ does not serve index.html');

const wanted = ['content-security-policy', 'strict-transport-security', 'x-frame-options', 'x-content-type-options',
  'referrer-policy', 'permissions-policy', 'cross-origin-opener-policy', 'cache-control'];
const headers = Object.fromEntries(wanted.map(name => [name, home.response.headers.get(name)]));
for (const [name, value] of Object.entries(headers)) fail(value, `missing ${name} header`);

const missing = await fetchBytes(`${origin}/this-page-does-not-exist-${Date.now()}`);
const notFound = { status: missing.response.status, isCustom404: missing.bytes.includes('That page') };
fail(notFound.status === 404 && notFound.isCustom404, 'unknown paths do not return the custom 404');

const redirect = url => new Promise((done, reject) => {
  const req = (url.startsWith('https') ? request : httpRequest)(url, { method: 'HEAD' }, response => {
    response.resume();
    done({ from: url, status: response.statusCode, location: response.headers.location ?? null });
  });
  req.on('error', reject);
  req.end();
});
const redirects = [await redirect('http://switchya.com/'), await redirect('http://www.switchya.com/')];
for (const r of redirects) fail([301, 308].includes(r.status) && r.location?.startsWith('https://'), `${r.from} does not redirect to HTTPS`);

console.log(JSON.stringify({ checkedAt: new Date().toISOString(), versionId, change, results, wwwMatchesIndex: sha256(www.bytes) === index.sha256, headers, notFound, redirects, failures }, null, 1));
process.exitCode = failures ? 1 : 0;
