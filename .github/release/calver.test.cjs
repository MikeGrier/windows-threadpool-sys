// Copyright (c) Mike Grier.
'use strict';
const { test } = require('node:test');
const assert = require('node:assert/strict');
const { nextCalendarVersion } = require('./calver.cjs');

test('calendar releases roll over UTC dates and increment within a day', () => {
  for (const [previous, now, expected] of [
    ['2026.902.0', '2026-09-19T00:00:00Z', '2026.919.0'],
    ['2026.919.0', '2026-09-19T23:59:59Z', '2026.919.1'],
    ['2026.919.8', '2026-09-19T00:00:00Z', '2026.919.9'],
    ['2026.1231.3', '2027-01-01T00:00:00Z', '2027.101.0'],
    ['2026.131.0', '2026-02-01T00:00:00Z', '2026.201.0'],
    ['2028.228.0', '2028-02-29T00:00:00Z', '2028.229.0'],
    ['2028.229.0', '2028-03-01T00:00:00Z', '2028.301.0'],
    ['2026.920.0', '2026-09-19T23:59:59Z', '2026.920.1'],
    ['2027.101.0', '2026-09-19T00:00:00Z', '2027.101.1'],
    ['0.0.1', '2026-09-19T00:00:00Z', '2026.919.0'],
    ['2026.919.0', '2026-09-19T22:00:00-07:00', '2026.920.0'],
  ]) assert.equal(nextCalendarVersion(previous, new Date(now)), expected);
});

test('invalid dates, prereleases and numeric overflow fail before publication', () => {
  for (const previous of ['1.2', '2026.0902.0', '2026.229.0', '2026.230.0', '2026.1301.0', '2026.900.0', '2026.902.-1', '2026.902.0-beta', '2026.902.9007199254740992']) {
    assert.throws(() => nextCalendarVersion(previous, new Date('2026-09-19T00:00:00Z')));
  }
  assert.throws(() => nextCalendarVersion('2026.919.9007199254740991', new Date('2026-09-19T00:00:00Z')));
  assert.throws(() => nextCalendarVersion('2026.902.0', new Date('invalid')));
});