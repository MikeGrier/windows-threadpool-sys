// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { PullRequestBody } = require('release-please/build/src/util/pull-request-body.js');
const { Version } = require('release-please/build/src/version.js');
const { expectedTags, missingTags, verifyReleased } = require('./check-release-tags.cjs');
const { rawMarkup, offenders } = require('./check-commit-text.cjs');

const root = path.resolve(__dirname, '../..');
const config = JSON.parse(fs.readFileSync(path.join(root, 'release-please-config.json'), 'utf8'));
const versions = JSON.parse(fs.readFileSync(path.join(root, '.release-please-manifest.json'), 'utf8'));
const allTags = () => new Set(expectedTags(config, versions).map(({ tag }) => tag));

test('every configured package has a tag name, and packages outside the config are not expected', () => {
  const tags = expectedTags(config, versions);
  assert.equal(tags.length, Object.keys(config.packages).length);
  assert(tags.some(({ tag }) => tag === `placement-probe-v${versions['crates/windows-placement-probe']}`), 'component name, not package name');
  assert(Object.keys(versions).length >= tags.length);
  assert(!tags.some(({ directory }) => directory === 'crates/win-job-launcher'));
});

test('a manifest version with no tag is reported, and a complete set of tags is not', () => {
  assert.deepEqual(missingTags(config, versions, allTags()), []);
  for (const { directory, tag } of expectedTags(config, versions)) {
    const without = allTags();
    without.delete(tag);
    assert.deepEqual(missingTags(config, versions, without), [{ directory, tag }]);
  }
  assert.equal(missingTags(config, versions, new Set()).length, Object.keys(config.packages).length);
});

test('a configured package with no manifest version is an error rather than a silent pass', () => {
  const [first] = Object.keys(config.packages);
  const { [first]: dropped, ...rest } = versions;
  assert.throws(() => expectedTags(config, rest), new RegExp(`${first}.*no version`));
});

test('the include-component-in-tag setting decides the tag shape', () => {
  const plain = { 'include-component-in-tag': false, packages: { a: { component: 'c' } } };
  assert.deepEqual(expectedTags(plain, { a: '1.2.3' }), [{ directory: 'a', tag: 'v1.2.3' }]);
  const named = { packages: { a: { 'package-name': 'p' } } };
  assert.deepEqual(expectedTags(named, { a: '1.2.3' }), [{ directory: 'a', tag: 'p-v1.2.3' }]);
});

test('verification of this tree fails naming the missing tag, and passes when every tag exists', async () => {
  const github = {};
  await verifyReleased(github, root, async () => allTags());
  const without = allTags();
  const [{ tag }] = expectedTags(config, versions);
  without.delete(tag);
  await assert.rejects(verifyReleased(github, root, async () => without), error => error.message.includes(tag));
});

test('markup is found in a BREAKING CHANGE footer or inside backticks in a subject, and nowhere release-please escapes or ignores', () => {
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: From<Event> was added'), ['<Event']);
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: Key<A> and Vec<u8>'), ['<A', '<u8']);
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: `From<Event>` and ``Key<A>``'), ['<Event', '<A'], 'an HTML parser ignores backticks');
  assert.deepEqual(rawMarkup('fix: add `From<Event>`'), ['<Event'], 'a subject is escaped except inside backticks');
  assert.deepEqual(rawMarkup('fix: add From<Event>\n\nthe body can say Vec<u8>'), [], 'a bare subject is escaped and the body is not copied');
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: fine\n\nCo-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>'), []);
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: fine\n\nCo-authored-by: Mike <mike@example.com>'), []);
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING-CHANGE: a </div> and <!-- c'), ['</div', '<!--']);
  assert.deepEqual(rawMarkup('fix!: x\n\nBREAKING CHANGE: 1 < 2, a<3 and Key[A]'), []);
  assert.deepEqual(offenders(['feat: ok', 'fix!: bad\n\nBREAKING CHANGE: Key<A>']), [{ subject: 'fix!: bad', found: ['<A'] }]);
});

test('the lint agrees with release-please: flagged footers lose components from the body, clean ones keep them', () => {
  const components = ['alpha', 'beta', 'gamma', 'delta', 'epsilon'];
  // An unclosed element inside one section is harmless; it takes two or more to swallow later sections.
  const parsed = footer => PullRequestBody.parse(new PullRequestBody(components.map((component, index) => ({
    component, version: Version.parse('1.0.0'),
    notes: `### Features\n\n* thing\n${index < 3 ? `\n### BREAKING CHANGES\n\n* ${footer}\n` : ''}`,
  })), { useComponents: true }).toString(), { warn() {}, error() {} }).releaseData.map(entry => entry.component);
  const flags = footer => rawMarkup(`fix!: x\n\nBREAKING CHANGE: ${footer}`).length > 0;
  for (const footer of ['Key<A> was added', '`Key<A>` was added']) {
    assert(flags(footer), footer);
    assert.notDeepEqual(parsed(footer), components, `${footer} must really lose a component`);
  }
  const bracketed = '`Key[A]` was added';
  assert(!flags(bracketed));
  assert.deepEqual(parsed(bracketed), components);
});
