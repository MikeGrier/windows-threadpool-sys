// Copyright (c) Mike Grier.
'use strict';
const fs = require('node:fs');
const path = require('node:path');

// release-please derives what to release from the body of the merged release pull request, and a
// component whose section it cannot read is skipped with a log line while the run still succeeds. The
// manifest on main already carries that component's new version, so a missing tag is the only trace.
function expectedTags(config, versions) {
  return Object.entries(config.packages).map(([directory, spec]) => {
    const version = versions[directory];
    if (!version) throw new Error(`${directory} is in release-please-config.json but has no version in .release-please-manifest.json`);
    const name = config['include-component-in-tag'] === false ? '' : (spec.component || spec['package-name'] || '');
    return { directory, tag: name ? `${name}-v${version}` : `v${version}` };
  });
}

function missingTags(config, versions, existing) {
  return expectedTags(config, versions).filter(({ tag }) => !existing.has(tag));
}

async function listTagNames(github) {
  const { owner, repo } = github.repository;
  const tags = await github.octokit.paginate(github.octokit.repos.listTags, { owner, repo, per_page: 100 });
  return new Set(tags.map(tag => tag.name));
}

async function verifyReleased(github, root = path.resolve(__dirname, '../..'), list = listTagNames) {
  const read = name => JSON.parse(fs.readFileSync(path.join(root, name), 'utf8'));
  const missing = missingTags(read('release-please-config.json'), read('.release-please-manifest.json'), await list(github));
  if (missing.length) {
    const lines = missing.map(({ directory, tag }) => `  ${tag} (${directory})`).join('\n');
    throw new Error(`The manifest names versions that have no tag, so release-please skipped their releases:\n${lines}\n` +
      'Create each release and tag at the release pull request merge commit; tag pushes drive publishing.');
  }
}

module.exports = { expectedTags, missingTags, listTagNames, verifyReleased };
