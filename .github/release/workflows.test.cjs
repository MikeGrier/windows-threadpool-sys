// Copyright (c) Mike Grier.

// Every workflow file is valid YAML. GitHub does not run a workflow it cannot parse, and says so only as a failed
// run with no jobs: the pull request's checks simply never appear, so a required one is missing and nothing says why.
// An unquoted `run:` value holding `: ` is enough (`--skip long_haul:: --skip probation::`), and no other check here
// reads a workflow as YAML.

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const YAML = require('yaml');

const workflows = path.join(__dirname, '..', 'workflows');

function problems(text) {
  const document = YAML.parseDocument(text);
  return [...document.errors, ...document.warnings].map((e) => e.message);
}

test('the check reports a plain scalar that holds ": "', () => {
  const broken = 'jobs:\n  t:\n    steps:\n      - run: cargo test -- --skip long_haul:: --skip probation::\n';
  assert.notEqual(problems(broken).length, 0);
});

test('the check accepts the same command quoted', () => {
  const quoted = "jobs:\n  t:\n    steps:\n      - run: 'cargo test -- --skip long_haul:: --skip probation::'\n";
  assert.deepEqual(problems(quoted), []);
});

for (const file of fs.readdirSync(workflows).filter((f) => /\.ya?ml$/.test(f))) {
  test(`${file} is valid YAML`, () => {
    const found = problems(fs.readFileSync(path.join(workflows, file), 'utf8'));
    assert.deepEqual(found, [], `${file}: ${found.join('; ')}`);
  });
}
