// Copyright (c) Mike Grier.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const yaml = require('yaml');
const toml = require('smol-toml');
const packages = require('./binary-packages.json');

function load(root) {
  const read = name => fs.readFileSync(path.join(root, name), 'utf8');
  return {
    config: JSON.parse(read('release-please-config.json')),
    versions: JSON.parse(read('.release-please-manifest.json')),
    releasePlease: yaml.parse(read('.github/workflows/release-please.yml')),
    registry: yaml.parse(read('.github/workflows/publish-crate.yml')),
    probes: Object.entries(packages).map(([directory, spec]) => ({ directory, spec,
      manifest: toml.parse(read(`${directory}/Cargo.toml`)),
      workflow: yaml.parse(read(`.github/workflows/${spec.workflow}`)),
    })),
  };
}

function check(data) {
  const steps = data.releasePlease.jobs['release-please'].steps;
  const wrapper = steps.find(step => step.run === 'node .github/release/release.cjs');
  assert(wrapper, 'release-please must load the calendar extension');
  assert.equal(wrapper.env.RELEASE_PLEASE_TOKEN, '${{ secrets.RELEASE_PLEASE_TOKEN }}', 'release tags need the PAT to trigger binary workflows');
  assert(steps.some(step => step.run === 'npm ci --prefix .github/release'), 'release tooling must use the lockfile');
  assert.deepEqual(data.releasePlease.on.push.branches, ['main']);
  for (const { directory, spec, manifest, workflow } of data.probes) {
    const config = data.config.packages[directory];
    assert(config, `${directory} missing from release-please`);
    assert.equal(config.component, spec.component);
    assert.equal(config['package-name'], manifest.package.name);
    assert.equal(config.versioning, 'probe-calver');
    assert.equal(data.versions[directory], manifest.package.version, 'manifest/package baseline mismatch');
    assert.equal(manifest.package.publish, false, 'probe packages must stay off crates.io');
    assert(workflow.on.push.tags.includes(`${spec.component}-v*`), 'missing probe tag trigger');
    assert(!data.registry.on.push.tags.includes(`${spec.component}-v*`), 'binary tag must not publish to crates.io');
    assert.deepEqual(workflow.jobs.build.strategy.matrix.target, ['x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc']);
    assert.equal(workflow.permissions.contents, 'read');
    assert.equal(workflow.jobs.release.permissions.contents, 'write');
    assert.equal(workflow.jobs.release.permissions['id-token'], 'write');
    assert.equal(workflow.jobs.release.permissions.attestations, 'write');
    const guard = `github.event_name == 'push' && startsWith(github.ref, 'refs/tags/${spec.component}-v')`;
    assert.equal(workflow.jobs.release.if, guard, 'release must exclude PR and manual tag dispatch');
    const uploads = workflow.jobs.build.steps.filter(step => step.uses?.startsWith('actions/upload-artifact@'));
    assert(uploads.length, 'missing binary artifact transfer');
    for (const upload of uploads) {
      assert.equal(upload.if, guard, 'PR/manual builds must not distribute stamped artifacts');
      assert.equal(upload.with['if-no-files-found'], 'error');
    }
    const releaseSteps = workflow.jobs.release.steps;
    const attest = releaseSteps.findIndex(step => step.uses?.startsWith('actions/attest-build-provenance@'));
    const publish = releaseSteps.findIndex(step => step.run?.includes('gh release upload'));
    assert(attest >= 0 && publish > attest, 'attest before publishing bytes');
    assert(!releaseSteps.some(step => step.run?.includes('gh release edit')), 'do not overwrite release-please changelogs');
    if (spec.package === 'windows-platform-probes') {
      const build = workflow.jobs.build.steps.find(step => step.name === 'Build and verify archive');
      assert(build?.run.includes('package-probes.cjs'), 'use metadata-derived packaging');
      assert.equal(build.env.IS_RELEASE, `\${{ ${guard} }}`);
      assert(!build.run.includes('--all-features'), 'test-only renderer oracle must remain disabled');
    }
  }
}

module.exports = { load, check };
if (require.main === module) {
  let result;
  try { check(load(process.argv[2] || path.resolve(__dirname, '../..'))); result = { status: 'success', message: 'Binary release routes and publication guards verified' }; }
  catch (error) { process.exitCode = 1; result = { status: 'error', error: error.message }; }
  process.stdout.write(JSON.stringify(result) + '\n');
}