'use strict';

const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const fs = require('node:fs');
const { createRequire } = require('node:module');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../codexctl.js'), 'utf8');

function launch({ platform = 'linux', arch = 'x64', resolve = () => '/binary' } = {}) {
  const child = new EventEmitter();
  const errors = [];
  const result = { child, errors };
  const process = {
    argv: ['node', 'codexctl', '--version'],
    pid: 123,
    exit(code) { result.exit = code; throw new Error('exit'); },
    kill(pid, signal) { result.signal = { pid, signal }; },
  };
  const require = (name) => {
    if (name === 'os') return { platform: () => platform, arch: () => arch };
    if (name === 'child_process') return {
      spawn(binary, args, options) {
        result.spawn = { binary, args: Array.from(args), options };
        return child;
      },
    };
    throw new Error(`Unexpected module: ${name}`);
  };
  require.resolve = (specifier) => { result.specifier = specifier; return resolve(specifier); };
  try {
    vm.runInNewContext(source, { require, process, console: { error: (s) => errors.push(s) } });
  } catch (error) {
    if (result.exit === undefined) throw error;
  }
  result.process = process;
  return result;
}

for (const layout of ['hoisted', 'nested']) {
  test(`resolves the binary from a ${layout} dependency layout`, () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'codexctl-launcher-'));
    try {
      const wrapper = path.join(dir, 'node_modules/codexctl/codexctl.js');
      const modules = layout === 'hoisted'
        ? path.join(dir, 'node_modules')
        : path.join(path.dirname(wrapper), 'node_modules');
      const binary = path.join(modules, '@codexctl/linux-x64/bin/codexctl');
      fs.mkdirSync(path.dirname(binary), { recursive: true });
      fs.writeFileSync(binary, 'fixture');
      const result = launch({ resolve: createRequire(wrapper).resolve });
      assert.equal(result.spawn.binary, fs.realpathSync(binary));
      assert.deepEqual(result.spawn.args, ['--version']);
      assert.equal(result.spawn.options.stdio, 'inherit');
    } finally {
      fs.rmSync(dir, { recursive: true, force: true });
    }
  });
}

test('resolves the Windows executable', () => {
  assert.equal(launch({ platform: 'win32' }).specifier, '@codexctl/win32-x64/bin/codexctl.exe');
});

test('rejects unsupported platforms without launching', () => {
  const result = launch({ arch: 'unsupported' });
  assert.equal(result.exit, 1);
  assert.equal(result.spawn, undefined);
});

test('reports missing optional dependencies without launching', () => {
  const result = launch({ resolve() { throw new Error('missing dependency'); } });
  assert.equal(result.exit, 1);
  assert.match(result.errors.join('\n'), /missing dependency/);
  assert.equal(result.spawn, undefined);
});

test('reports spawn errors as failures', () => {
  const result = launch();
  result.child.emit('error', new Error('permission denied'));
  assert.equal(result.process.exitCode, 1);
  assert.match(result.errors.join('\n'), /permission denied/);
});

test('preserves the binary exit code', () => {
  const result = launch();
  result.child.emit('exit', 23, null);
  assert.equal(result.process.exitCode, 23);
});

test('preserves signal termination instead of reporting success', () => {
  const result = launch();
  result.child.emit('exit', null, 'SIGTERM');
  assert.deepEqual(result.signal, { pid: 123, signal: 'SIGTERM' });
});
