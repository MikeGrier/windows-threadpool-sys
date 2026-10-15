// Copyright (c) Mike Grier.
'use strict';
const fs = require('node:fs');
const path = require('node:path');
const toml = require('smol-toml');
const packages = require('./binary-packages.json');

// `cargo publish` refuses a crate with a normal or build dependency that has a `path` and no `version`,
// since the published manifest drops the path and would name nothing. A dev-dependency is stripped on
// publish, so a path alone is fine there, and a package may list itself as one.
const KINDS = ['dependencies', 'build-dependencies'];

function dependencyTables(manifest) {
  const tables = KINDS.map(kind => [kind, manifest[kind]]);
  for (const [target, body] of Object.entries(manifest.target ?? {})) {
    for (const kind of KINDS) tables.push([`target.${target}.${kind}`, body[kind]]);
  }
  return tables.filter(([, table]) => table);
}

// The workspace's own dependency table, which `workspace = true` entries are read through.
function inherited(root) {
  const manifest = toml.parse(fs.readFileSync(path.join(root, 'Cargo.toml'), 'utf8'));
  return manifest.workspace?.dependencies ?? {};
}

function unversioned(manifest, workspaceDependencies = {}) {
  const found = [];
  for (const [table, dependencies] of dependencyTables(manifest)) {
    for (const [name, spec] of Object.entries(dependencies)) {
      const resolved = spec?.workspace === true ? workspaceDependencies[name] : spec;
      if (typeof resolved === 'object' && resolved !== null && resolved.path && !resolved.version) {
        found.push(`${name} in [${table}]`);
      }
    }
  }
  return found;
}

// Every released crate that cannot publish for want of a version, as `crate: dependency in [table]`.
function check(root) {
  const read = name => fs.readFileSync(path.join(root, name), 'utf8');
  const config = JSON.parse(read('release-please-config.json'));
  const workspaceDependencies = inherited(root);
  const problems = [];
  for (const directory of Object.keys(config.packages).filter(directory => !packages[directory])) {
    const manifest = toml.parse(read(`${directory}/Cargo.toml`));
    for (const found of unversioned(manifest, workspaceDependencies)) {
      problems.push(`${manifest.package.name}: ${found}`);
    }
  }
  return problems;
}

module.exports = { check, unversioned };
if (require.main === module) {
  const problems = check(process.argv[2] || path.resolve(__dirname, '../..'));
  if (problems.length) {
    process.stdout.write(JSON.stringify({ status: 'error', problems }) + '\n');
    process.exitCode = 1;
  } else {
    process.stdout.write(JSON.stringify({ status: 'success' }) + '\n');
  }
}
