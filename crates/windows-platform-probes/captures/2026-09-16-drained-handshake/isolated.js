// Copyright (c) Mike Grier.

// Isolated regime: each claim-word layout against the default, read against a
// same-code control.
//
// `summarise.js` beside this derives the DRAINED tables only. The isolated
// figures are cited too -- by the queue crate's docs, which say the whole push
// path was measured as slower under `Wide` at every producer count -- so they
// need a derivation a reader can run, not a number taken on trust.
//
// The control is the point. `reserving_mpsc` and `reserving(32/32)` are the SAME
// CODE under two names, so the gap between them is what "no difference" looks
// like on this host; a layout ratio only means something read against it.
//
// Three runs give three control observations per count. The range of three
// samples is NOT a band -- a fresh draw falls outside the range of three priors
// about half the time by construction -- so the last column reports what these
// runs did, and is not a claim about what the next run would do.
//
// Usage: node isolated.js run1.txt run2.txt run3.txt
"use strict";

const fs = require("fs");
const crypto = require("crypto");

// One writer for the generated artifact, per this repository's output rule: no
// formatting site picks a destination, so retargeting the report to a file is a
// change here and nowhere else. `fail` is the diagnostic path and stays separate
// from the artifact, which is why they are two sinks rather than one with a flag.
let sink = (text) => process.stdout.write(text + "\n");
const out = (text = "") => sink(text);
const fail = (text) => process.stderr.write(text + "\n");

const files = process.argv.slice(2);
if (files.length === 0) {
  fail("usage: node isolated.js <run.txt> [run.txt ...]");
  process.exit(2);
}
// `files.length` is reported as the number of independent runs, so the same
// path given twice would publish a two-run comparison -- with a zero-width
// control range -- derived from one set of bytes. A typo must not make the
// capture look more reproducible than it is.
{
  // Compared by CONTENT, not by name. A resolved path catches `./run1.txt` and
  // a symlink, but not `copy-of-run1.txt`, which has a different real path and
  // identical bytes -- and the run count would still present it as a second
  // observation, narrowing the reported range without adding data. What is being
  // claimed here is independent runs, so identical bytes cannot be two of them.
  const seen = new Map();
  for (const file of files) {
    const digest = crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
    if (seen.has(digest)) {
      fail(`the same capture was given twice: ${file} is byte-identical to ${seen.get(digest)}`);
      process.exit(2);
    }
    seen.set(digest, file);
  }
}

const COUNTS = [1, 2, 4, 8, 16, 32];
const DEFAULT_LAYOUT = "reserving(32/32)";
// Same code as DEFAULT_LAYOUT, under the shipping type's own name.
const CONTROL_TWIN = "reserving_mpsc";
const LAYOUTS = ["reserving(16/48)", "reserving(8/56)", "reserving(64/64)"];

// The isolated raw table only. The drained table repeats every shape name, so a
// whole-file scan would silently average the two regimes together.
function isolatedRows(file) {
  const lines = fs.readFileSync(file, "utf8").split("\n");
  const start = lines.findIndex((l) => l.startsWith("-- isolated:"));
  const end = lines.findIndex((l) => l.startsWith("-- drained:"));
  if (start < 0 || end < 0 || end <= start) {
    throw new Error(`${file}: expected an isolated marker followed by a drained one`);
  }
  const rows = new Map();
  for (const line of lines.slice(start, end)) {
    const m = line.match(/^(\S+)\s+(\d+)\s+([\d.]+)\s/);
    if (m) {
      const key = `${m[1]}@${m[2]}`;
      // A `Map` keeps the last write. A run carrying the whole expected producer
      // set plus one duplicated row would pass every completeness check while
      // one measurement was silently discarded in favour of another.
      if (rows.has(key)) {
        throw new Error(`${file}: a second isolated row for ${key}`);
      }
      rows.set(key, Number(m[3]));
    }
  }
  return rows;
}

const tables = files.map(isolatedRows);

function ratios(numerator, denominator) {
  const out = new Map();
  for (const n of COUNTS) {
    out.set(
      n,
      tables.map((t, i) => {
        const a = t.get(`${numerator}@${n}`);
        const b = t.get(`${denominator}@${n}`);
        // A missing or unusable row is an error, not a skipped count: silently
        // dropping one would quietly narrow every range printed below.
        // Both operands must be positive, not merely finite and the denominator
        // non-zero: a zero or negative numerator divides cleanly and publishes
        // an ordinary-looking `0.00x`. The probe's did-not-run sentinel used to
        // be `0.0`, so that is the shape a legacy capture actually takes.
        if (!Number.isFinite(a) || !Number.isFinite(b) || a <= 0 || b <= 0) {
          throw new Error(`${files[i]}: no usable ${numerator}/${denominator} at ${n} producers`);
        }
        return a / b;
      }),
    );
  }
  return out;
}

// The middle of an odd set, and the mean of the two middle values otherwise --
// matching `summarise.js`. The CLI takes any number of runs, and picking the
// upper-middle for an even set would report the slower of two runs as their
// median, which is a different statistic under the same name.
const median = (xs) => {
  const sorted = [...xs].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
};
const fmt = (x) => x.toFixed(2);
const cell = (xs) => `${fmt(median(xs))}x [${fmt(Math.min(...xs))}-${fmt(Math.max(...xs))}]`;

// Same code under two names. The denominator is `DEFAULT_LAYOUT`, exactly as it
// is for every layout ratio above -- a control read against ratios computed the
// other way round is not a control, because these ranges are not symmetric about
// 1.00 and the reciprocal of [0.95-1.26] is [0.79-1.05]. `summarise.js` computes
// its control in this same direction.
const control = ratios(CONTROL_TWIN, DEFAULT_LAYOUT);
const measured = new Map(LAYOUTS.map((l) => [l, ratios(l, DEFAULT_LAYOUT)]));

// Widths derived from the cells, not fixed. `cell()` renders a median and a
// range of measured ratios, which have no upper bound, so a fixed field shifts
// every column after it the first time a value outgrows it -- the same argument
// `column_width` makes for the Rust report's columns.
const COLUMNS = ["control", ...LAYOUTS.map((l) => l.replace("reserving", ""))];
const body = COUNTS.map((n) => [cell(control.get(n)), ...LAYOUTS.map((l) => cell(measured.get(l).get(n)))]);
// The widths this table has always used, kept as floors so the committed
// output is unchanged; the derivation only ever widens.
const width = COLUMNS.map((name, column) =>
  Math.max(18, name.length, ...body.map((row) => row[column].length)),
);
const PRODUCERS_WIDTH = Math.max(9, ...COUNTS.map((n) => String(n).length));

out(`isolated regime, ${files.length} run(s): ${files.join(", ")}`);
out(`each layout against ${DEFAULT_LAYOUT}; control is ${CONTROL_TWIN} against ${DEFAULT_LAYOUT}`);
out("median of the per-run ratios, with the observed range beside it\n");

out(
  ["producers".padEnd(PRODUCERS_WIDTH + 2), ...COLUMNS.map((name, i) => name.padEnd(width[i] + 2))].join(""),
);
COUNTS.forEach((n, row) => {
  out(
    [
      String(n).padEnd(PRODUCERS_WIDTH + 2),
      ...body[row].map((c, i) => c.padEnd(width[i] + 2)),
    ].join(""),
  );
});

out("\nwhere every run sat above the control's whole observed range:");
for (const l of LAYOUTS) {
  const above = COUNTS.filter((n) => {
    const top = Math.max(...control.get(n));
    return measured.get(l).get(n).every((x) => x > top);
  });
  out(`  ${l.padEnd(18)} ${above.length ? above.join(", ") + " producers" : "no producer count"}`);
}

out(
  `\nThe control's range here is ${files.length} observation(s) per count, which is not\n` +
    "a band. This reports what these runs did; it does not establish that a fresh\n" +
    "run would land the same way.",
);
