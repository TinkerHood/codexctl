import { appendFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

const version = process.env.VERSION;
const repository = process.env.GITHUB_REPOSITORY;
if (!/^\d+\.\d+\.\d+$/.test(version ?? '') || !repository) {
  throw new Error('VERSION and GITHUB_REPOSITORY are required');
}

function manifest(directory) {
  const pkg = JSON.parse(readFileSync(join(directory, 'package.json'), 'utf8'));
  if (pkg.version !== version || pkg.repository?.url !== `git+https://github.com/${repository}.git`) {
    throw new Error(`Release metadata mismatch in ${directory}/package.json`);
  }
  return pkg;
}

async function exists(name) {
  const response = await fetch(`https://registry.npmjs.org/${encodeURIComponent(name)}/${version}`, {
    signal: AbortSignal.timeout(30_000),
  });
  if (response.status === 404) return false;
  if (!response.ok) throw new Error(`Registry check failed for ${name}: HTTP ${response.status}`);
  const published = await response.json();
  if (published.version !== version || published.repository?.url !== `git+https://github.com/${repository}.git`) {
    throw new Error(`Published metadata mismatch for ${name}@${version}`);
  }
  return true;
}

if (process.argv[2] === 'check') {
  const pkg = manifest(process.argv[3]);
  const publish = !(await exists(pkg.name));
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `publish=${publish}\n`);
  console.log(`${pkg.name}@${version}: ${publish ? 'ready to publish' : 'already published'}`);
} else if (process.argv[2] === 'platforms') {
  const pkg = manifest('npm');
  for (const [name, dependencyVersion] of Object.entries(pkg.optionalDependencies)) {
    if (dependencyVersion !== version) throw new Error(`Platform ${name} must use exact release version ${version}`);
    let available = false;
    for (let attempt = 0; attempt < 6; attempt++) {
      if (await exists(name)) { available = true; break; }
      if (attempt < 5) await new Promise(resolve => setTimeout(resolve, 10_000));
    }
    if (!available) throw new Error(`${name}@${version} is not available; refusing to publish wrapper`);
  }
} else {
  throw new Error('Usage: npm-release.mjs check <directory> | platforms');
}
