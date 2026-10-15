// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const toml = require('smol-toml');
const { check, unversioned } = require('./check-dependency-versions.cjs');

test('every released crate in the repository pins the dependencies it names by path', () => {
  assert.deepEqual(check(path.resolve(__dirname, '../..')), []);
});

test('a path with no version is refused, in every table that is published', () => {
  const found = unversioned(toml.parse([
    '[dependencies]',
    'a = { path = "../a" }',
    'b = { version = "0.1.0", path = "../b" }',
    'c = "1"',
    '[dependencies.d]',
    'path = "../d"',
    '[build-dependencies]',
    'e = { path = "../e", optional = true }',
    '[target.\'cfg(windows)\'.dependencies]',
    'f = { path = "../f" }',
  ].join('\n')));
  assert.deepEqual(found, [
    'a in [dependencies]',
    'd in [dependencies]',
    'e in [build-dependencies]',
    'f in [target.cfg(windows).dependencies]',
  ]);
});

test('what is stripped on publish, or already versioned, is accepted', () => {
  const manifest = toml.parse([
    '[dependencies]',
    'g = { workspace = true }',
    'h = { version = "1", path = "../h", features = ["x"] }',
    '[dev-dependencies]',
    'self = { path = ".", features = ["seam"] }',
    'i = { path = "../i" }',
  ].join('\n'));
  assert.deepEqual(unversioned(manifest, { g: { version: '1', path: '../g' } }), []);
  assert.deepEqual(unversioned(manifest, { g: { path: '../g' } }), ['g in [dependencies]']);
});
