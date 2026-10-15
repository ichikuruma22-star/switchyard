// Release packaging and publication checks: no dependencies, network, Docker
// or GitHub access. Workflow steps run under bash with a stand-in `gh`.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, posix, resolve } from 'node:path';
import test from 'node:test';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const read = path => readFileSync(join(root, path), 'utf8').replace(/\r\n/g, '\n');

const packager = read('scripts/package-switchya-preview.ps1');
const packagedReadme = packager.match(/\$readme = @'\n([\s\S]*?)\n'@/)[1];
const allowlist = [...packager.match(/foreach \(\$relative in @\(([\s\S]*?)\)\)/)[1].matchAll(/'([^']+)'/g)]
  .map(match => match[1]);

test('every relative Markdown link in the portable archive resolves inside it', () => {
  const entries = new Set([...allowlist, 'README.md', 'RELEASE-NOTES.md', 'switchya.exe', 'switchya-desktop.exe']);
  const documents = [['README.md', packagedReadme], ...allowlist.filter(path => path.endsWith('.md')).map(path => [path, read(path)])];
  for (const [path, text] of documents) {
    for (const [, target] of text.matchAll(/\]\(([^)\s]+)\)/g)) {
      const file = target.split('#')[0];
      if (!file || /^[a-z][a-z0-9+.-]*:/i.test(file)) continue;
      const entry = posix.normalize(posix.join(posix.dirname(path), file));
      assert.ok(entries.has(entry), `${path} links to ${target}, which the archive does not contain`);
    }
  }
});

test('the packaged README describes every supported existing agent', () => {
  const names = { codex: 'Codex', claude: 'Claude Code', antigravity: 'Antigravity' };
  const agents = read('crates/app/src/cli/adapters.rs').match(/value_parser = \[("codex"[^\]]*)\]/)[1];
  for (const [, id] of agents.matchAll(/"([^"]+)"/g)) {
    assert.ok(names[id], `describe the ${id} agent in the packaged README, then name it here`);
    assert.match(packagedReadme, new RegExp(`\\b${names[id]}\\b`), `packaged README omits ${names[id]}`);
  }
  assert.doesNotMatch(packagedReadme, /Saved Codex conversations|Claude launches in an external terminal/);
});

// GitHub runs these steps with bash. On Windows, System32's bash.exe starts
// WSL, so use Git for Windows' bash instead, or skip when there is none.
function findBash() {
  if (process.platform !== 'win32') return 'bash';
  const gitExec = spawnSync('git', ['--exec-path'], { encoding: 'utf8' }).stdout?.trim();
  const bash = gitExec && resolve(gitExec, '../../../bin/bash.exe');
  return bash && existsSync(bash) ? bash : null;
}
const bash = findBash();
const needsBash = { skip: !bash && 'requires bash (Git for Windows on Windows)' };

// Runs one release.yml step's script with stand-ins for GitHub's context.
function runStep(name, env) {
  const workflow = read('.github/workflows/release.yml');
  const step = workflow.match(new RegExp(`- name: ${name}\\n[\\s\\S]*?\\n( +)run: \\|\\n((?:\\1  .*\\n|\\n)+)`));
  assert.ok(step, `release.yml has no ${name} step`);
  const script = step[2].replace(new RegExp(`^${step[1]}  `, 'gm'), '')
    .replaceAll('${{ github.repository }}', 'Example/Switchyard');
  const work = mkdtempSync(join(tmpdir(), 'switchyard-release-step-'));
  try {
    mkdirSync(join(work, 'dist'));
    writeFileSync(join(work, 'dist', 'switchyard.tar.gz'), '');
    writeFileSync(join(work, 'output'), '');
    const gh = 'gh() { printf "%s\\n" "$*" >> gh.log; [[ "$2" != view ]]; }\n: > gh.log\n';
    const result = spawnSync(bash, ['-c', gh + script], {
      cwd: work, encoding: 'utf8', env: { ...process.env, GITHUB_OUTPUT: join(work, 'output'), ...env },
    });
    assert.ifError(result.error);
    return {
      status: result.status,
      output: readFileSync(join(work, 'output'), 'utf8'),
      gh: readFileSync(join(work, 'gh.log'), 'utf8'),
    };
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

test('prerelease tags are not published as the latest release or image', needsBash, () => {
  for (const [tag, version] of [['v0.2.0-rc.1', '0.2.0-rc.1'], ['v1.0.0-preview.3', '1.0.0-preview.3']]) {
    const validate = runStep('Validate tag name', { TAG: tag });
    assert.equal(validate.status, 0);
    assert.match(validate.output, /^prerelease=true$/m);
    const publish = runStep('Create or update release', { TAG: tag, PRERELEASE: 'true' });
    assert.equal(publish.status, 0);
    assert.match(publish.gh, new RegExp(`^release create ${tag} .* --prerelease --latest=false$`, 'm'));
    const image = runStep('Image tags', { TAG: tag, PRERELEASE: 'true' });
    assert.equal(image.output, `tags=ghcr.io/example/switchyard:${version}\n`);
  }
});

test('stable tags remain the latest release and image', needsBash, () => {
  const validate = runStep('Validate tag name', { TAG: 'v0.2.0' });
  assert.equal(validate.status, 0);
  assert.match(validate.output, /^prerelease=false$/m);
  const publish = runStep('Create or update release', { TAG: 'v0.2.0', PRERELEASE: 'false' });
  assert.equal(publish.status, 0);
  assert.match(publish.gh, /^release create v0\.2\.0 --title Switchyard v0\.2\.0 --generate-notes$/m);
  const image = runStep('Image tags', { TAG: 'v0.2.0', PRERELEASE: 'false' });
  assert.equal(image.output, 'tags=ghcr.io/example/switchyard:0.2.0,ghcr.io/example/switchyard:latest\n');
  assert.notEqual(runStep('Validate tag name', { TAG: 'v0.2' }).status, 0);
});

// Docker matches .dockerignore patterns from the context root: `**` spans
// directories, a matching directory excludes its contents, and the last
// matching pattern wins.
function dockerIgnored(path) {
  const rules = read('.dockerignore').split('\n').map(line => line.trim())
    .filter(line => line && !line.startsWith('#'))
    .map(line => {
      const negate = line.startsWith('!');
      const pattern = posix.normalize(line.slice(negate ? 1 : 0)).replace(/^\//, '');
      const source = pattern.replace(/\*\*\/|\*\*|[*?]|[.+^${}()|[\]\\]/g, token => ({
        '**/': '(?:.*/)?', '**': '.*', '*': '[^/]*', '?': '[^/]',
      })[token] ?? `\\${token}`);
      return { negate, regex: new RegExp(`^${source}$`) };
    });
  const parts = path.split('/');
  const prefixes = parts.map((_, index) => parts.slice(0, index + 1).join('/'));
  let ignored = false;
  for (const { negate, regex } of rules) {
    if (prefixes.some(prefix => regex.test(prefix))) ignored = !negate;
  }
  return ignored;
}

test('the source Docker build context leaves out nested build trees', () => {
  for (const path of [
    'target/release/switchyard', 'desktop/target/debug/switchya-desktop', 'desktop/gen/schemas/desktop.json',
    'site/node_modules/wrangler/package.json', 'app-ui/node_modules/x.js', 'site/.wrangler/state/x', 'site/debug.log',
  ]) {
    assert.ok(dockerIgnored(path), `${path} is sent to the Docker build`);
  }
  for (const path of [
    'Cargo.toml', 'Cargo.lock', 'switchyard.example.toml', 'crates/app/src/lib.rs',
    'crates/scheduler/catalog/models.json', 'ui/index.html', 'app-ui/index.html',
  ]) {
    assert.ok(!dockerIgnored(path), `${path} is needed by the Docker build`);
  }
});
