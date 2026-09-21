#!/usr/bin/env node
/**
 * CodexCTL - Wrapper that uses platform-specific binary from optionalDependencies
 * 
 * When npm installs this package, it automatically downloads the correct
 * @codexctl/{platform} package based on OS/arch. The binary is then available
 * in node_modules/@codexctl/{platform}/bin/
 */

'use strict';

const os = require('os');
const { spawn } = require('child_process');

const PLATFORMS = {
  'linux-x64': '@codexctl/linux-x64',
  'linux-arm64': '@codexctl/linux-arm64',
  'darwin-x64': '@codexctl/darwin-x64',
  'darwin-arm64': '@codexctl/darwin-arm64',
  'win32-x64': '@codexctl/win32-x64'
};

function getPlatformKey() {
  return `${os.platform()}-${os.arch()}`;
}

function resolvePlatformPackage() {
  const key = getPlatformKey();
  return PLATFORMS[key] || null;
}

const platformPackage = resolvePlatformPackage();
if (!platformPackage) {
  const supported = Object.keys(PLATFORMS).sort().join(', ');
  console.error(`Unsupported platform/architecture: ${getPlatformKey()}`);
  console.error(`Supported targets: ${supported}`);
  process.exit(1);
}

const isWindows = os.platform() === 'win32';
const binaryName = isWindows ? 'codexctl.exe' : 'codexctl';
let binaryPath;
try {
  binaryPath = require.resolve(`${platformPackage}/bin/${binaryName}`);
} catch (error) {
  console.error(`codexctl binary not found in ${platformPackage}: ${error.message}`);
  console.error('Reinstall package to fetch the correct optional dependency for this platform.');
  process.exit(1);
}

const child = spawn(binaryPath, process.argv.slice(2), { 
  stdio: 'inherit', 
  windowsHide: true 
});

// An explicit signal to the Node launcher does not automatically reach its
// Rust child. Keep the launcher alive until the child has restored auth.
const forwardedSignals = isWindows ? [] : ['SIGINT', 'SIGTERM'];
const forwardSignal = new Map(forwardedSignals.map((signal) => [signal, () => {
  if (child.pid) child.kill(signal);
}]));
for (const signal of forwardedSignals) {
  process.on(signal, forwardSignal.get(signal));
}
const removeSignalForwarding = () => {
  for (const signal of forwardedSignals) {
    process.removeListener(signal, forwardSignal.get(signal));
  }
};

child.on('error', (error) => {
  removeSignalForwarding();
  console.error(`Failed to start codexctl: ${error.message}`);
  process.exitCode = 1;
});

child.on('exit', (code, signal) => {
  removeSignalForwarding();
  if (signal) {
    process.kill(process.pid, signal);
  } else {
    process.exitCode = code ?? 1;
  }
});
