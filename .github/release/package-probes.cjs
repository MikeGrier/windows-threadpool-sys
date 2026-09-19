// Copyright (c) Mike Grier.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { createHash } = require('node:crypto');
const { zipSync } = require('fflate');
const packages = require('./binary-packages.json');

const machines = { 'x86_64-pc-windows-msvc': 0x8664, 'aarch64-pc-windows-msvc': 0xaa64 };

function checkPe(bytes, target) {
  if (!machines[target] || bytes.length < 64 || bytes.toString('ascii', 0, 2) !== 'MZ') throw new Error('Invalid executable or target');
  const offset = bytes.readUInt32LE(60);
  if (offset > bytes.length - 6 || bytes.readUInt32LE(offset) !== 0x4550 || bytes.readUInt16LE(offset + 4) !== machines[target]) {
    throw new Error('PE architecture does not match target');
  }
}

function selectExecutables(pkg, records) {
  const expected = pkg.targets.filter(target => target.kind.includes('bin')).map(target => target.name).sort();
  if (!expected.length) throw new Error('Package declares no binaries');
  const found = new Map();
  for (const record of records) {
    if (record.reason !== 'compiler-artifact' || record.package_id !== pkg.id || !record.executable || !record.target.kind.includes('bin')) continue;
    if (record.features.includes('oracle-in-renderer')) throw new Error('Shipping build enables the test-only renderer oracle');
    if (found.has(record.target.name)) throw new Error('Duplicate executable artifact');
    found.set(record.target.name, record.executable);
  }
  if (JSON.stringify([...found.keys()].sort()) !== JSON.stringify(expected)) throw new Error('Compiled executable census differs from Cargo targets');
  return found;
}

function archive(pkg, records, { target, revision, component, tag, read = fs.readFileSync }) {
  if (tag && tag !== `${component}-v${pkg.version}`) throw new Error('Release tag does not match package version');
  if (!/^[0-9a-f]{40}$/.test(revision)) throw new Error('Expected full build commit');
  const files = {};
  for (const [name, executable] of selectExecutables(pkg, records)) {
    if (!/^[a-zA-Z0-9_-]+$/.test(name)) throw new Error('Invalid binary filename');
    const bytes = read(executable);
    checkPe(bytes, target);
    files[`${name}.exe`] = bytes;
  }
  const info = { package: pkg.name, version: pkg.version, target, git_revision: revision,
    features: 'default', binaries: Object.keys(files).sort() };
  files['BUILD-INFO.json'] = Buffer.from(JSON.stringify(info, null, 2) + '\n');
  return { files, info };
}

function main(args) {
  const [packageName, target, output, tag] = args;
  const entry = Object.entries(packages).find(([, spec]) => spec.package === packageName);
  if (!entry || !machines[target] || !output) throw new Error('usage: package-probes <package> <Windows target> <new scratch output directory> [tag]');
  const root = path.resolve(__dirname, '../..');
  const destination = path.resolve(output);
  if (!destination.startsWith(path.join(root, '.scratch') + path.sep)) throw new Error('Output must be under .scratch');
  if (fs.existsSync(destination)) throw new Error('Output directory already exists');
  const run = (program, argv, extra = {}) => execFileSync(program, argv, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024, ...extra });
  const metadata = JSON.parse(run('cargo', ['metadata', '--locked', '--no-deps', '--format-version', '1']));
  const pkg = metadata.packages.find(pkg => pkg.name === packageName);
  if (!pkg) throw new Error('Package is not in workspace');
  if (tag && tag !== `${entry[1].component}-v${pkg.version}`) throw new Error('Release tag does not match package version');
  const revision = run('git', ['--no-pager', 'rev-parse', 'HEAD']).trim();
  if (tag && run('git', ['--no-pager', 'status', '--porcelain']).trim()) throw new Error('Tagged publication requires a clean checkout');
  const built = run('cargo', ['build', '--release', '--locked', '--target', target, '-p', packageName, '--bins', '--message-format=json'], {
    env: { ...process.env, PLACEMENT_PROBE_COMMIT: revision, PLACEMENT_PROBE_SOURCE: 'ci' },
  });
  const records = built.split(/\r?\n/).filter(Boolean).map(line => JSON.parse(line));
  if (!records.some(record => record.reason === 'build-finished' && record.success)) throw new Error('Cargo did not finish successfully');
  const { files, info } = archive(pkg, records, { target, revision, component: entry[1].component, tag });
  files['README.md'] = fs.readFileSync(path.join(root, entry[0], 'README.md'));
  files['LICENSE'] = fs.readFileSync(path.join(root, 'LICENSE'));
  const name = `${packageName}-${target.split('-')[0]}.zip`;
  const bytes = zipSync(files);
  fs.mkdirSync(destination, { recursive: true });
  fs.writeFileSync(path.join(destination, name), bytes, { flag: 'wx' });
  fs.writeFileSync(path.join(destination, `${name}.sha256`), `${createHash('sha256').update(bytes).digest('hex')}  ${name}\n`, { flag: 'wx' });
  return { archive: name, ...info };
}

module.exports = { archive, selectExecutables, checkPe, main };
if (require.main === module) {
  let result;
  try { result = { status: 'success', ...main(process.argv.slice(2)) }; }
  catch (error) { process.exitCode = 1; result = { status: 'error', error: error.message }; }
  process.stdout.write(JSON.stringify(result) + '\n');
}