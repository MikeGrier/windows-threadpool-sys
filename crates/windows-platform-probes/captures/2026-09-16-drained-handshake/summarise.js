// Copyright (c) Mike Grier.
// Summarise the drained tables of a queue-contention capture.
//
// Reads the probe's own report text rather than re-deriving anything: the
// medians and bounds are whatever the instrument printed. What this adds is the
// across-run median per producer count, and the same-code control span, which
// no single run states because it is a relation between two tables.

const fs = require("fs");

const RATIO = /([0-9.]+)x \[([0-9.]+)-([0-9.]+)\]/g;

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[middle - 1] + sorted[middle]) / 2
    : sorted[middle];
}

// Problems are collected rather than thrown, so one malformed capture reports
// everything wrong with it instead of only the first thing.
//
// **Nothing here may quietly degrade to `NaN`.** Every comparison against `NaN`
// is false, so a `NaN` bound makes the containment filter below find nothing
// outside it and print `true` -- certifying a capture it could not read. A
// report renders `--` for a shape that did not run, and `Number("--")` is
// exactly that `NaN`.
const problems = [];

function finite(text, what) {
  const value = Number(text);
  if (!Number.isFinite(value)) {
    problems.push(`${what}: ${JSON.stringify(text)} is not a number`);
    return null;
  }
  return value;
}

// producers -> { narrowNanos, ratios: [16/48, 8/56, 64/64] }
function drainedLayout(lines, path) {
  let start = -1;
  lines.forEach((line, i) => {
    if (line.includes("-- drained --")) start = i;
  });
  const rows = new Map();
  for (const line of lines.slice(start + 2, start + 8)) {
    const fields = line.trim().split(/\s+/);
    const producers = finite(fields[0], `${path}: a drained layout producer count`);
    if (producers === null) continue;
    const where = `${path}, drained layout, ${producers} producers`;
    const narrowNanos = finite(fields[1], `${where}: the 32/32 cost`);
    const ratios = [...line.matchAll(RATIO)].map((m) => Number(m[1]));
    // A row that did not run renders `--`, which the ratio pattern does not
    // match, so a short list is the signal that this row cannot be summarised.
    if (ratios.length !== 3) {
      problems.push(
        `${where}: found ${ratios.length} layout ratios, expected 3`,
      );
      continue;
    }
    if (narrowNanos === null) continue;
    rows.set(producers, { narrowNanos, ratios });
  }
  return rows;
}

// producers -> reserving ns/op, from the comparison table
function drainedComparison(lines, path) {
  const start = lines.findIndex((line) => line.includes("reserving/slotwise"));
  const rows = new Map();
  for (const line of lines.slice(start + 2, start + 8)) {
    const fields = line.trim().split(/\s+/);
    const producers = finite(fields[0], `${path}: a comparison producer count`);
    if (producers === null) continue;
    const reserving = finite(
      fields[2],
      `${path}, drained comparison, ${producers} producers: the reserving_mpsc cost`,
    );
    if (reserving === null) continue;
    rows.set(producers, reserving);
  }
  return rows;
}

const paths = process.argv.slice(2);
const layouts = [];
const controls = [];
for (const path of paths) {
  const lines = fs.readFileSync(path, "utf8").split(/\r?\n/);
  const layout = drainedLayout(lines, path);
  const comparison = drainedComparison(lines, path);
  layouts.push(layout);
  // The same code measured twice in one run: `reserving_mpsc` in the comparison
  // table against `32/32` in the layout table.
  const control = new Map();
  for (const [producers, row] of layout) {
    const reserving = comparison.get(producers);
    if (reserving === undefined) {
      problems.push(
        `${path}: ${producers} producers has a layout row but no comparison row`,
      );
      continue;
    }
    if (row.narrowNanos > 0) {
      control.set(producers, reserving / row.narrowNanos);
    }
  }
  controls.push(control);
}

const producers = [...layouts[0].keys()].sort((a, b) => a - b);
console.log(`runs: ${paths.length}`);
console.log("");
console.log("drained layout ratios vs 32/32, median across runs");
console.log("producers  16/48   8/56   64/64");
const medians = [];
for (const p of producers) {
  const present = layouts.filter((layout) => layout.has(p));
  if (present.length !== layouts.length) {
    problems.push(`${p} producers is missing from ${layouts.length - present.length} run(s)`);
    continue;
  }
  const row = [0, 1, 2].map((column) =>
    median(present.map((layout) => layout.get(p).ratios[column])),
  );
  medians.push(...row);
  console.log(
    `${String(p).padStart(9)}  ` + row.map((m) => `${m.toFixed(2)}x`).join("  "),
  );
}

const every = controls.flatMap((control) => [...control.values()]);
console.log("");
console.log("same-code control (reserving_mpsc vs reserving 32/32), drained");
console.log(`  observations: ${every.length}`);

// **Refuse to certify a capture that could not be read.** Everything below
// compares against the control band, and every comparison against `NaN` is
// false -- so a single unreadable cell would empty the "outside the band" list
// and print `true`. An incomplete capture must say so, not pass.
if (problems.length > 0 || every.length === 0 || medians.length === 0) {
  console.log("");
  console.log("CAPTURE INCOMPLETE -- not summarised:");
  if (every.length === 0) console.log("  - no control observations were read");
  if (medians.length === 0) console.log("  - no layout medians were read");
  for (const problem of problems) console.log(`  - ${problem}`);
  console.log("");
  console.log("every layout median inside the control band: UNKNOWN");
  process.exitCode = 1;
  return;
}

console.log(
  `  span:         ${Math.min(...every).toFixed(2)}x to ${Math.max(...every).toFixed(2)}x`,
);
console.log(`  median:       ${median(every).toFixed(2)}x`);
console.log("");
const low = Math.min(...every);
const high = Math.max(...every);
// **Every** layout median, not just the largest. The claim this capture is
// cited for is that all of them sit inside the control band, and an earlier
// version of this check tested `Math.max(...medians)` alone -- which passes
// unchanged while a median below the band's floor goes unreported. The
// committed data happens to clear the floor, so that check was right by luck
// rather than by construction.
const outside = medians.filter((m) => m < low || m > high);
console.log(
  `layout medians: ${medians.length}, spanning ` +
    `${Math.min(...medians).toFixed(2)}x to ${Math.max(...medians).toFixed(2)}x`,
);
console.log(`every layout median inside the control band: ${outside.length === 0}`);
if (outside.length > 0) {
  console.log(`  outside: ${outside.map((m) => `${m.toFixed(2)}x`).join(", ")}`);
}
