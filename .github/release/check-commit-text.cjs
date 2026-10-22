// Copyright (c) Mike Grier.
'use strict';
const { spawnSync } = require('node:child_process');

const BREAKING = /^BREAKING[ -]CHANGE:/m;

// release-please copies a BREAKING CHANGE footer into the changelog verbatim and escapes a subject
// except inside backticks, and then reads the release pull request body as HTML. An HTML parser takes
// no notice of backticks, so a Type<Param> there opens an element that is never closed, and two of
// them swallow the sections after them: those components get no tag and the run still succeeds.
// An address in angle brackets, such as a Co-authored-by trailer, is not an element and is allowed.
const MARKUP = /<(?![^<>\s]*@)[A-Za-z/!?][^\s>]*/g;

function rawMarkup(message) {
  const [subject] = message.split('\n', 1);
  const quoted = (subject.match(/``[^`].*[^`]``|`[^`]*`/g) ?? []).join(' ');
  const start = message.search(BREAKING);
  const footer = start < 0 ? '' : message.slice(start);
  return `${quoted}\n${footer}`.match(MARKUP) ?? [];
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
