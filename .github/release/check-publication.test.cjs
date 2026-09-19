// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const { load, check } = require('./check-publication.cjs');

test('actual manifests and workflows have executable binary release routes', () => check(load(path.resolve(__dirname, '../..'))));

test('publication guards reject missing routes, unsafe triggers and oracle builds', () => {
  for (let defect = 0; defect < 12; defect++) {
    const data = load(path.resolve(__dirname, '../..'));
    const probe = data.probes[defect % 2];
    switch (defect) {
      case 0: delete data.config.packages[probe.directory]; break;
      case 1: probe.workflow.on.push.tags = []; break;
      case 2: probe.workflow.jobs.release.if = 'true'; break;
      case 3: delete probe.workflow.jobs.build.steps.find(step => step.uses?.startsWith('actions/upload-artifact@')).if; break;
      case 4: probe.manifest.package.publish = true; break;
      case 5: data.registry.on.push.tags.push(`${probe.spec.component}-v*`); break;
      case 6: probe.workflow.jobs.release.steps.reverse(); break;
      case 7: probe.workflow.jobs.build.steps.find(step => step.name === 'Build and verify archive').run += '\ncargo build --all-features'; break;
      case 8: data.versions[probe.directory] = '0.0.0'; break;
      case 9: probe.workflow.jobs.build.strategy.matrix.target.pop(); break;
      case 10: data.releasePlease.jobs['release-please'].steps.find(step => step.id === 'release').env.RELEASE_PLEASE_TOKEN = '${{ github.token }}'; break;
      case 11: data.config.packages[probe.directory].versioning = 'default'; break;
    }
    assert.throws(() => check(data), `defect ${defect} survived`);
  }
});