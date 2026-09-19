// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { registerCalendarVersions, releaseAndPropose } = require('./release.cjs');
const { buildVersioningStrategy } = require('release-please/build/src/factories/versioning-strategy-factory.js');
const { buildPlugin } = require('release-please/build/src/factories/plugin-factory.js');
const { Version } = require('release-please/build/src/version.js');
const { nextCalendarVersion } = require('./calver.cjs');

test('release-please uses calendar versions for probes and unchanged semver for libraries', () => {
  registerCalendarVersions(new Date('2026-09-19T00:00:00Z'));
  const github = { repository: { owner: 'test', repo: 'test' } };
  const commits = [{ type: 'feat', scope: null, bareMessage: 'test', message: 'feat: test', notes: [], references: [], breaking: false }];
  const calendar = buildVersioningStrategy({ type: 'probe-calver', github });
  assert.equal(calendar.bump(Version.parse('2026.902.0'), commits).toString(), '2026.919.0');
  assert.equal(calendar.bump(Version.parse('2026.919.0'), commits).toString(), '2026.919.1');
  const normal = buildVersioningStrategy({ type: 'default', github });
  assert.equal(normal.bump(Version.parse('1.2.3'), commits).toString(), '1.3.0');
  const plugin = buildPlugin({ type: 'cargo-workspace', github, targetBranch: 'main', repositoryConfig: {
    'crates/probe': { releaseType: 'rust', versioning: 'probe-calver' },
    'crates/library': { releaseType: 'rust' },
  } });
  assert.equal(plugin.bumpVersion({ path: 'crates/probe', version: '2026.902.0' }).toString(), '2026.919.0');
  assert.equal(plugin.bumpVersion({ path: 'crates/library', version: '1.2.3' }).toString(), '1.2.4');
  const separate = buildPlugin({ type: { type: 'cargo-workspace', merge: false }, github, targetBranch: 'main', repositoryConfig: {} });
  assert.equal(separate.merge, false);
});

test('release phase precedes fresh proposal loading and errors are not swallowed', async () => {
  const events = [];
  const load = async () => {
    events.push('load');
    return { createReleases: async () => { events.push('release'); return [undefined, { path: 'crate', tagName: 'tag' }]; },
      createPullRequests: async () => { events.push('pr'); return [undefined, { number: 1 }]; } };
  };
  const result = await releaseAndPropose({}, load);
  assert.deepEqual(events, ['load', 'release', 'load', 'pr']);
  assert.equal(result.releases.length, 1);
  assert.equal(result.prs.length, 1);
  await assert.rejects(releaseAndPropose({}, async () => { throw new Error('API failed'); }), /API failed/);
});

test('real Rust strategy updates binary-only manifests without changing distribution or tag names', async () => {
  const fs = require('node:fs');
  const path = require('node:path');
  const toml = require('smol-toml');
  const { buildStrategy } = require('release-please/build/src/factory.js');
  const { parseConventionalCommits } = require('release-please/build/src/commit.js');
  const { TagName } = require('release-please/build/src/util/tag-name.js');
  const root = path.resolve(__dirname, '../..');
  const settings = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json'), 'utf8'));
  const versions = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json'), 'utf8'));
  const packages = require('./binary-packages.json');
  registerCalendarVersions(new Date('2026-09-19T00:00:00Z'));
  const github = { repository: { owner: 'example', repo: 'probes', defaultBranch: 'main' },
    getFileContentsOnBranch: async filename => ({ parsedContent: fs.readFileSync(path.join(root, filename), 'utf8') }) };
  for (const [directory, spec] of Object.entries(packages)) {
    const strategy = await buildStrategy({ github, targetBranch: 'main', path: directory, releaseType: 'rust',
      component: spec.component, packageName: spec.package, includeComponentInTag: true,
      versioning: settings.packages[directory].versioning });
    const previous = { tag: new TagName(Version.parse(versions[directory]), spec.component), sha: 'a'.repeat(40), notes: '' };
    const commits = parseConventionalCommits([{ sha: 'b'.repeat(40), message: 'feat: ship probe binaries', files: [`${directory}/Cargo.toml`] }]);
    assert.equal(await strategy.buildReleasePullRequest([], previous), undefined);
    assert.equal(await strategy.buildReleasePullRequest(parseConventionalCommits([{ sha: 'c'.repeat(40), message: 'chore: metadata only', files: [] }]), previous), undefined);
    const candidate = await strategy.buildReleasePullRequest(commits, previous);
    assert(candidate, 'publish=false binary crate must get a release PR');
    const expected = nextCalendarVersion(versions[directory], new Date('2026-09-19T00:00:00Z'));
    assert.equal(candidate.version.toString(), expected);
    const update = candidate.updates.find(update => update.path === `${directory}/Cargo.toml`);
    assert(update);
    const manifest = toml.parse(update.updater.updateContent(fs.readFileSync(path.join(root, update.path), 'utf8')));
    assert.equal(manifest.package.version, expected);
    assert.equal(manifest.package.publish, false);
    assert.equal((await strategy.getComponent()), spec.component);
    assert.equal(new TagName(candidate.version, await strategy.getComponent()).toString(), `${spec.component}-v${expected}`);
  }
});

test('release entry point rejects pull requests and manual dispatch before API work', () => {
  const { spawnSync } = require('node:child_process');
  const path = require('node:path');
  for (const event of ['pull_request', 'workflow_dispatch']) {
    const result = spawnSync(process.execPath, [path.join(__dirname, 'release.cjs')], {
      encoding: 'utf8', env: { ...process.env, GITHUB_EVENT_NAME: event, GITHUB_REF: 'refs/heads/main', RELEASE_PLEASE_TOKEN: '' },
    });
    assert.notEqual(result.status, 0);
    assert.match(result.stdout + result.stderr, /only run on a push to main/);
  }
});

test('a library release propagates calendar bumps to binary dependents and the root lockfile', async () => {
  const fs = require('node:fs');
  const path = require('node:path');
  const toml = require('smol-toml');
  const { buildStrategy } = require('release-please/build/src/factory.js');
  const { parseConventionalCommits } = require('release-please/build/src/commit.js');
  const { TagName } = require('release-please/build/src/util/tag-name.js');
  const root = path.resolve(__dirname, '../..');
  const read = filename => fs.readFileSync(path.join(root, filename), 'utf8');
  const packages = require('./binary-packages.json');
  const library = 'crates/windows-topology-sys';
  const members = [library, ...Object.keys(packages)];
  const settings = JSON.parse(read('release-please-config.json'));
  const versions = JSON.parse(read('.release-please-manifest.json'));
  registerCalendarVersions(new Date('2026-09-19T00:00:00Z'));
  const github = { repository: { owner: 'example', repo: 'probes', defaultBranch: 'main' },
    findFilesByGlobAndRef: async member => [member],
    getFileContentsOnBranch: async filename => ({ parsedContent: filename === 'Cargo.toml'
      ? `[workspace]\nmembers = ${JSON.stringify(members)}\n` : read(filename) }) };
  const repositoryConfig = {};
  const strategies = {};
  const releases = {};
  for (const directory of members) {
    const config = settings.packages[directory];
    repositoryConfig[directory] = { releaseType: 'rust', component: config.component || config['package-name'],
      packageName: config['package-name'], versioning: config.versioning || 'default', includeComponentInTag: true };
    strategies[directory] = await buildStrategy({ ...repositoryConfig[directory], github, targetBranch: 'main', path: directory });
    releases[directory] = { tag: new TagName(Version.parse(versions[directory]), repositoryConfig[directory].component), sha: 'a'.repeat(40), notes: '' };
  }
  const plugin = buildPlugin({ type: 'cargo-workspace', github, targetBranch: 'main', repositoryConfig });
  await plugin.preconfigure(strategies, {}, releases);
  assert.deepEqual(await plugin.run([]), []);
  const commits = parseConventionalCommits([{ sha: 'b'.repeat(40), message: 'fix: topology discovery', files: [`${library}/src/lib.rs`] }]);
  const pullRequest = await strategies[library].buildReleasePullRequest(commits, releases[library]);
  const candidates = await plugin.run([{ path: library, config: repositoryConfig[library], pullRequest }]);
  assert.equal(candidates.length, 1);
  const updates = candidates[0].pullRequest.updates;
  const updated = filename => updates.filter(update => update.path === filename).reduce((content, update) => update.updater.updateContent(content), read(filename));
  for (const [directory, spec] of Object.entries(packages)) {
    const expected = nextCalendarVersion(versions[directory], new Date('2026-09-19T00:00:00Z'));
    assert.equal(toml.parse(updated(`${directory}/Cargo.toml`)).package.version, expected);
    assert.equal(JSON.parse(updated('.release-please-manifest.json'))[directory], expected);
    assert.equal(toml.parse(updated('Cargo.lock')).package.find(pkg => pkg.name === spec.package).version, expected);
    assert(candidates[0].pullRequest.body.releaseData.some(release => release.component === spec.component));
  }
});