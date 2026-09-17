// Copyright (c) Mike Grier.
// Summarise the drained tables of a queue-contention capture.
//
// Reads the probe's own report text rather than re-deriving anything: the
// medians and bounds are whatever the instrument printed. What this adds is the
// across-run median per producer count, and the same-code control span, which
// no single run states because it is a relation between two tables.

const fs = require("fs");

// One writer for the generated artifact, per this repository's output rule: no
// formatting site picks a destination, so retargeting the report to a file is a
// change here and nowhere else. ail is the diagnostic path and stays separate
// from the artifact, which is why they are two sinks rather than one with a flag.
let sink = (text) => process.stdout.write(text + "\n");
const out = (text = "") => sink(text);
const fail = (text) => process.stderr.write(text + "\n");

// A ratio cell, with all three numbers required to be plain decimals. The
// looser `[0-9.]+` also matched a run of dots, so a malformed `...x [..-..]`
// parsed and `Number("...")` became NaN -- which compares false against every
// guard downstream and would surface in the output rather than be rejected.
const RATIO = /(\d+\.\d+)x \[(\d+\.\d+)-(\d+\.\d+)\]/g;

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

// Every quantity this script reads is a cost or a ratio of costs, so zero and
// negative are as invalid as NaN -- and more dangerous, because `0.00x` is
// finite and renders as an ordinary-looking measurement. The probe's own
// did-not-run sentinel used to be `0.0`, so this is the shape a legacy or
// malformed capture actually takes.
function positive(text, what) {
  const value = finite(text, what);
  if (value === null) return null;
  if (value <= 0) {
    problems.push(`${what}: ${JSON.stringify(text)} is not positive`);
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
  // The raw table's marker is `-- drained: ... --`, which this does not match;
  // the one it finds is the claim-layout interpretation table, which is the one
  // carrying ratios. A missing marker must stop the run rather than silently
  // slice from line 1, which would summarise whatever happened to be there.
  if (start < 0) {
    problems.push(`${path}: no drained layout table (expected a "-- drained --" marker)`);
    return new Map();
  }
  const rows = new Map();
  for (const line of lines.slice(start + 2, start + 8)) {
    const fields = line.trim().split(/\s+/);
    const producers = finite(fields[0], `${path}: a drained layout producer count`);
    if (producers === null) continue;
    const where = `${path}, drained layout, ${producers} producers`;
    const narrowNanos = positive(fields[1], `${where}: the 32/32 cost`);
    // Pass every value through `positive`, like every other captured quantity,
    // and check all three numbers of each cell -- the two bounds are not used by
    // this script, but a capture carrying an unreadable bound is not a capture
    // this script should certify as summarised.
    const ratios = [...line.matchAll(RATIO)].flatMap((m, i) => [
      positive(m[1], `${where}: layout ratio ${i + 1}`),
      positive(m[2], `${where}: layout ratio ${i + 1} lower bound`),
      positive(m[3], `${where}: layout ratio ${i + 1} upper bound`),
    ]);
    // A row that did not run renders `--`, which the ratio pattern does not
    // match, so a short list is the signal that this row cannot be summarised.
    if (ratios.length !== 9) {
      problems.push(
        `${where}: found ${ratios.length / 3} layout ratios, expected 3`,
      );
      continue;
    }
    if (ratios.some((r) => r === null)) continue;
    if (narrowNanos === null) continue;
    rows.set(producers, { narrowNanos, ratios: [ratios[0], ratios[3], ratios[6]] });
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
    const reserving = positive(
      fields[2],
      `${path}, drained comparison, ${producers} producers: the reserving_mpsc cost`,
    );
    if (reserving === null) continue;
    rows.set(producers, reserving);
  }
  return rows;
}

const paths = process.argv.slice(2);
// An empty argument list summarises nothing: every completeness check below
// iterates the captures, so with none of them there is nothing to complain
// about, `median([])` is NaN, and the script prints an empty summary and exits
// successfully. Rejected here, as `isolated.js` does.
if (paths.length === 0) {
  fail("usage: node summarise.js <run.txt> [run.txt ...]");
  process.exit(2);
}
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

// The sweep the probe runs, stated rather than inferred. Deriving the expected
// set from the first capture makes the completeness check circular: three runs
// all truncated at the same producer count agree with each other, `rows` stays
// non-empty, and the script reports a partial capture as a whole one.
const EXPECTED_PRODUCERS = [1, 2, 4, 8, 16, 32];

layouts.forEach((layout, i) => {
  const seen = [...layout.keys()].sort((a, b) => a - b);
  if (seen.join(",") !== EXPECTED_PRODUCERS.join(",")) {
    problems.push(
      `${paths[i]}: drained layout covers producers [${seen}], expected [${EXPECTED_PRODUCERS}]`,
    );
  }
});

const producers = EXPECTED_PRODUCERS;
out(`runs: ${paths.length}`);

// **Reported per producer count, and deliberately without a verdict.**
//
// An earlier version pooled every control observation into one band and asked
// whether each layout median fell inside it. That answered `true`, and the
// answer was an artifact of the pooling: the control is not independent of
// producer count -- it spans about 0.82-0.98x at one producer against
// 0.95-1.23x at thirty-two on this capture -- so pooling builds a band wider
// than any count's own and containment follows from the method rather than
// from the data.
//
// Comparing per count instead does not rescue a verdict either. Three runs
// give three control observations per count, and the range of three samples is
// not a band: a fresh independent draw falls outside the range of three priors
// about half the time. So neither comparison is strong enough to say a median
// is inside or outside, and this script says so rather than picking whichever
// framing yields an answer.
const rows = [];
for (const p of producers) {
  const present = layouts.filter((layout) => layout.has(p));
  if (present.length !== layouts.length) {
    problems.push(
      `${p} producers is missing from ${layouts.length - present.length} run(s)`,
    );
    continue;
  }
  const control = controls.map((c) => c.get(p)).filter((v) => Number.isFinite(v));
  if (control.length !== controls.length) {
    problems.push(`${p} producers has no control ratio in every run`);
    continue;
  }
  rows.push({
    producers: p,
    control,
    medians: [0, 1, 2].map((column) =>
      median(present.map((layout) => layout.get(p).ratios[column])),
    ),
  });
}

// **Refuse to summarise a capture that could not be read.** A report renders
// `--` for a shape that did not run, `Number("--")` is `NaN`, and every
// comparison against `NaN` is false -- so an unreadable cell used to empty the
// "outside the band" list and print `true`.
if (problems.length > 0 || rows.length === 0) {
  out("");
  out("CAPTURE INCOMPLETE -- not summarised:");
  if (rows.length === 0) out("  - no complete producer counts were read");
  for (const problem of problems) out(`  - ${problem}`);
  process.exitCode = 1;
  return;
}

out("");
out(
  "drained, per producer count: the same-code control's observed range, then",
);
out("each layout's median ratio against 32/32, across runs.");
out("");
// Derived, with the table's original width as a floor: a control band is built
// from measured ratios and has no upper bound, so a fixed field would shift the
// layout columns the first time one outgrew it.
const bands = rows.map((row) => {
  const low = Math.min(...row.control);
  const high = Math.max(...row.control);
  return `${low.toFixed(2)}-${high.toFixed(2)}(${row.control.length})`;
});
const bandWidth = Math.max(16, "control(n)".length, ...bands.map((b) => b.length));
out(`producers   ${"control(n)".padEnd(bandWidth)}  16/48   8/56   64/64`);
rows.forEach((row, i) => {
  out(
    `${String(row.producers).padStart(9)}   ${bands[i].padEnd(bandWidth)}  ` +
      row.medians.map((m) => `${m.toFixed(2)}x`).join("  "),
  );
});

const everyControl = rows.flatMap((row) => row.control);
out("");
out(
  `control observations: ${everyControl.length} across ${rows.length} producer counts, ` +
    `${Math.min(...everyControl).toFixed(2)}x to ${Math.max(...everyControl).toFixed(2)}x pooled`,
);
out(
  "Pooled only to show the spread; it is not a band to judge a median against,",
);
out("for the reason recorded in this script beside the table above.");
