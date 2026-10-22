// Copyright (c) Mike Grier.
'use strict';
const fs = require('node:fs');
const path = require('node:path');

// release-please derives what to release from the body of the merged release pull request, and a
// component whose section it cannot read is skipped with a log line while the run still succeeds. The
// manifest on main already carries that component's new version, so a missing tag is the only trace.
//
// A version that has no tag is not always a skipped release: a crate is registered at a bootstrap
// version in a chore commit, before its first release exists. What separates the two is the crate's
// CHANGELOG.md, which the release pull request writes in the same commit that sets the version, so
// only a version that the changelog already records is owed a tag.
function expectedTags(config, versions) {
  return Object.entries(config.packages).map(([directory, spec]) => {
    const version = versions[directory];
    if (!version) throw new Error(`${directory} is in release-please-config.json but has no version in .release-please-manifest.json`);
    const name = config['include-component-in-tag'] === false ? '' : (spec.component || spec['package-name'] || '');
    return { directory, tag: name ? `${name}-v${version}` : `v${version}` };
  });
}

function changelogRecords(text, version) {
  return new RegExp(`^## \\[?${version.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}[\\] ]`, 'm').test(text);
}

function missingTags(config, versions, existing, recorded) {
  return expectedTags(config, versions).filter(({ directory, tag }) => !existing.has(tag) && recorded(directory, versions[directory]));
}

async function listTagNames(github) {
  const { owner, repo } = github.repository;
  const tags = await github.octokit.paginate(github.octokit.rest.repos.listTags, { owner, repo, per_page: 100 });
  return new Set(tags.map(tag => tag.name));
}

async function verifyReleased(github, root = path.resolve(__dirname, '../..'), list = listTagNames) {
  const read = name => JSON.parse(fs.readFileSync(path.join(root, name), 'utf8'));
  const recorded = (directory, version) => {
    const file = path.join(root, directory, 'CHANGELOG.md');
    return fs.existsSync(file) && changelogRecords(fs.readFileSync(file, 'utf8'), version);
  };
  const missing = missingTags(read('release-please-config.json'), read('.release-please-manifest.json'), await list(github), recorded);
  if (missing.length) {
    const lines = missing.map(({ directory, tag }) => `  ${tag} (${directory})`).join('\n');
    throw new Error(`The changelogs record versions that have no tag, so release-please skipped their releases:\n${lines}\n` +
      'Create each release and tag at the release pull request merge commit; tag pushes drive publishing.');
  }
}

module.exports = { expectedTags, missingTags, changelogRecords, listTagNames, verifyReleased };
