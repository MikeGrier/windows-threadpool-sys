// Copyright (c) Mike Grier.
'use strict';
const { GitHub, Manifest, registerVersioningStrategy, registerPlugin } = require('release-please');
const { DefaultVersioningStrategy } = require('release-please/build/src/versioning-strategies/default.js');
const { CargoWorkspace } = require('release-please/build/src/plugins/cargo-workspace.js');
const { Version } = require('release-please/build/src/version.js');
const { nextCalendarVersion } = require('./calver.cjs');

function registerCalendarVersions(date = new Date()) {
  class CalendarStrategy extends DefaultVersioningStrategy {
    determineReleaseType() {
      return { bump: previous => Version.parse(nextCalendarVersion(previous.toString(), date)) };
    }
  }
  class CalendarWorkspace extends CargoWorkspace {
    bumpVersion(pkg) {
      if (this.repositoryConfig[pkg.path]?.versioning === 'probe-calver') {
        return Version.parse(nextCalendarVersion(pkg.version, date));
      }
      return super.bumpVersion(pkg);
    }
  }
  registerVersioningStrategy('probe-calver', options => new CalendarStrategy(options));
  registerPlugin('cargo-workspace', options => new CalendarWorkspace(
    options.github, options.targetBranch, options.repositoryConfig,
    { ...options, ...options.type, merge: options.type.merge ?? !options.separatePullRequests },
  ));
}

async function releaseAndPropose(github, load = () => Manifest.fromManifest(github, github.repository.defaultBranch)) {
  const releases = (await (await load()).createReleases()).filter(Boolean);
  const prs = (await (await load()).createPullRequests()).filter(Boolean);
  return { releases, prs };
}

async function main(core) {
  if (process.env.GITHUB_EVENT_NAME !== 'push' || process.env.GITHUB_REF !== 'refs/heads/main') {
    throw new Error('Release automation may only run on a push to main');
  }
  const token = process.env.RELEASE_PLEASE_TOKEN;
  if (!token) throw new Error('RELEASE_PLEASE_TOKEN is required to trigger downstream tag workflows');
  const [owner, repo] = (process.env.GITHUB_REPOSITORY || '').split('/');
  if (!owner || !repo) throw new Error('GITHUB_REPOSITORY is required');
  registerCalendarVersions();
  const github = await GitHub.create({ owner, repo, token, defaultBranch: 'main' });
  const { releases, prs } = await releaseAndPropose(github);
  core.setOutput('release_created', releases.length > 0);
  core.setOutput('releases_created', releases.length > 0);
  core.setOutput('paths_released', JSON.stringify(releases.map(release => release.path || '.')));
  core.setOutput('pr', prs[0] || '');
  core.setOutput('prs', JSON.stringify(prs));
  for (const release of releases) {
    const prefix = release.path && release.path !== '.' ? `${release.path}--` : '';
    core.setOutput(`${prefix}release_created`, true);
    core.setOutput(`${prefix}tag_name`, release.tagName);
  }
}

module.exports = { registerCalendarVersions, releaseAndPropose };
if (require.main === module) import('@actions/core').then(core => main(core).catch(error => core.setFailed(error.message)));