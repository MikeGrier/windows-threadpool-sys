// Copyright (c) Mike Grier.
'use strict';
const { spawnSync } = require('node:child_process');
const { parseConventionalCommits } = require('release-please/build/src/commit.js');

// release-please copies a breaking-change note into the changelog verbatim and escapes a subject
// except inside backticks, and then reads the release pull request body as HTML. An HTML parser takes
// no notice of backticks, so a Type<Param> there opens an element that is never closed, and two of
// them swallow the sections after them: those components get no tag and the run still succeeds.
// The notes are what release-please's own parser produces, not a re-reading of the footer syntax: a
// `!` in the subject makes the subject itself a note, and an explicit footer replaces that note.
// An address in angle brackets, such as a Co-authored-by trailer, is not an element and is allowed.
const MARKUP = /<(?![^<>\s]*@)[A-Za-z/!?][^\s>]*/g;
const quiet = { warn() {}, error() {}, info() {}, debug() {}, trace() {} };

function rawMarkup(message) {
  return parseConventionalCommits([{ sha: 'x', message }], quiet).flatMap(commit => {
    const quoted = commit.bareMessage.match(/``[^`].*[^`]``|`[^`]*`/g) ?? [];
    const notes = commit.notes.filter(note => note.title === 'BREAKING CHANGE').map(note => note.text);
    return [...quoted, ...notes].join('\n').match(MARKUP) ?? [];
  });
}

function offenders(messages) {
  return messages.map(message => ({ subject: message.split('\n', 1)[0], found: rawMarkup(message) }))
    .filter(entry => entry.found.length);
}

function messagesIn(range) {
  const log = spawnSync('git', ['log', '--format=%B%x00', range], { encoding: 'utf8', maxBuffer: 1 << 28 });
  if (log.error) throw log.error;
  if (log.status !== 0) throw new Error(`git log ${range} failed: ${log.stderr}`);
  return log.stdout.split('\0').map(message => message.trim()).filter(Boolean);
}

module.exports = { rawMarkup, offenders, messagesIn };
if (require.main === module) {
  const range = process.argv[2];
  if (!range) { console.error('usage: check-commit-text.cjs <revision-range>'); process.exit(2); }
  const bad = offenders(messagesIn(range));
  for (const { subject, found } of bad) {
    console.error(`${subject}\n  has markup release-please cannot carry: ${found.join(' ')}; write it without angle brackets, for example Key[A]`);
  }
  process.exitCode = bad.length ? 1 : 0;
}
