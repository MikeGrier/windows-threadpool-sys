// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { zipSync, unzipSync } = require('fflate');
const { archive, checkPe, selectExecutables } = require('./package-probes.cjs');

function pe(machine) {
  const bytes = Buffer.alloc(128);
  bytes.write('MZ'); bytes.writeUInt32LE(64, 60); bytes.writeUInt32LE(0x4550, 64); bytes.writeUInt16LE(machine, 68);
  return bytes;
}
function fixture(count) {
  const pkg = { id: 'path+probe', name: 'probe', version: '2026.919.0', targets: Array.from({ length: count }, (_, index) => ({ name: `probe-${index}`, kind: ['bin'] })) };
  const records = pkg.targets.map(target => ({ reason: 'compiler-artifact', package_id: pkg.id, target, executable: `${target.name}.exe`, features: [] }));
  return { pkg, records };
}

test('archives exactly the declared binaries for both architectures and growing target sets', () => {
  for (const count of [1, 2, 3, 4, 5, 8, 10, 16, 17, 32]) {
    for (const [target, machine] of [['x86_64-pc-windows-msvc', 0x8664], ['aarch64-pc-windows-msvc', 0xaa64]]) {
      const { pkg, records } = fixture(count);
      const result = archive(pkg, records, { target, revision: 'a'.repeat(40), component: 'probe', tag: 'probe-v2026.919.0', read: () => pe(machine) });
      const extracted = unzipSync(zipSync(result.files));
      assert.equal(Object.keys(extracted).length, count + 1);
      assert.equal(JSON.parse(Buffer.from(extracted['BUILD-INFO.json']).toString()).binaries.length, count);
      for (const name of result.info.binaries) checkPe(Buffer.from(extracted[name]), target);
    }
  }
});

test('missing, extra, duplicate, wrong architecture and oracle-enabled binaries fail', () => {
  const { pkg, records } = fixture(2);
  assert.throws(() => selectExecutables(pkg, records.slice(1)), /census/);
  assert.throws(() => selectExecutables(pkg, [...records, records[0]]), /Duplicate/);
  assert.throws(() => selectExecutables(pkg, [...records, { ...records[0], target: { kind: ['bin'], name: 'extra' } }]), /census/);
  assert.throws(() => selectExecutables(pkg, [{ ...records[0], features: ['oracle-in-renderer'] }, records[1]]), /oracle/);
  assert.throws(() => checkPe(pe(0x8664), 'aarch64-pc-windows-msvc'), /architecture/);
  assert.throws(() => checkPe(Buffer.alloc(5), 'x86_64-pc-windows-msvc'), /Invalid/);
  assert.throws(() => archive(pkg, records, { target: 'x86_64-pc-windows-msvc', revision: 'a'.repeat(40), component: 'probe', tag: 'probe-v0.0.1' }), /tag/);
  assert.equal(selectExecutables(pkg, [...records, { reason: 'compiler-artifact', package_id: 'dependency', target: { kind: ['bin'], name: 'other' }, executable: 'other.exe' }]).size, 2);
});