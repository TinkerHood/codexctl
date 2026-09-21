import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, writeFileSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const script = resolve(import.meta.dirname, 'npm-release.mjs');
function run(mode, status, published = {}, dependencies = {}) {
  const cwd = mkdtempSync(join(tmpdir(), 'codexctl-publish-'));
  try {
    mkdirSync(join(cwd, 'npm'));
    writeFileSync(join(cwd, 'npm/package.json'), JSON.stringify({
      name: 'codexctl', version: '0.10.1',
      repository: { url: 'git+https://github.com/TinkerHood/codexctl.git' },
      optionalDependencies: dependencies,
    }));
    const mock = join(cwd, 'mock.mjs');
    writeFileSync(mock, `globalThis.fetch = async () => new Response(${JSON.stringify(JSON.stringify(published))}, {status: ${status}});`);
    return spawnSync(process.execPath, ['--import', mock, script, mode, 'npm'], {
      cwd, encoding: 'utf8',
      env: { ...process.env, VERSION: '0.10.1', GITHUB_REPOSITORY: 'TinkerHood/codexctl', GITHUB_OUTPUT: '' },
    });
  } finally { rmSync(cwd, { recursive: true, force: true }); }
}

test('an unpublished version is ready only after a registry 404', () => {
  const result = run('check', 404);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /ready to publish/);
});
test('registry failures stop publication instead of pretending version is absent', () => {
  const result = run('check', 503);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /HTTP 503/);
});
test('existing versions require matching repository provenance metadata', () => {
  const result = run('check', 200, { version: '0.10.1', repository: { url: 'git+https://github.com/repohelper/codexctl.git' } });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /Published metadata mismatch/);
});
test('matching published version is skipped safely', () => {
  const result = run('check', 200, { version: '0.10.1', repository: { url: 'git+https://github.com/TinkerHood/codexctl.git' } });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /already published/);
});
test('wrapper refuses platform ranges that could select a different release', () => {
  const result = run('platforms', 200, {}, { '@codexctl/linux-x64': '^0.10.1' });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /must use exact release version/);
});
