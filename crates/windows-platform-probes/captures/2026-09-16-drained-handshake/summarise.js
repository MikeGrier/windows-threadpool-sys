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

// producers -> { narrowNanos, ratios: [16/48, 8/56, 64/64] }
function drainedLayout(lines) {
  let start = -1;
  lines.forEach((line, i) => {
    if (line.includes("-- drained --")) start = i;
  });
  const rows = new Map();
  for (const line of lines.slice(start + 2, start + 8)) {
    const fields = line.trim().split(/\s+/);
    const ratios = [...line.matchAll(RATIO)].map((m) => Number(m[1]));
    rows.set(Number(fields[0]), { narrowNanos: Number(fields[1]), ratios });
  }
  return rows;
}

// producers -> reserving ns/op, from the comparison table
function drainedComparison(lines) {
  const start = lines.findIndex((line) => line.includes("reserving/slotwise"));
  const rows = new Map();
  for (const line of lines.slice(start + 2, start + 8)) {
    const fields = line.trim().split(/\s+/);
    rows.set(Number(fields[0]), Number(fields[2]));
  }
  return rows;
}

const paths = process.argv.slice(2);
const layouts = [];
const controls = [];
for (const path of paths) {
  const lines = fs.readFileSync(path, "utf8").split(/\r?\n/);
  const layout = drainedLayout(lines);
  const comparison = drainedComparison(lines);
  layouts.push(layout);
  // The same code measured twice in one run: `reserving_mpsc` in the comparison
  // table against `32/32` in the layout table.
  const control = new Map();
  for (const [producers, row] of layout) {
    if (row.narrowNanos > 0) {
      control.set(producers, comparison.get(producers) / row.narrowNanos);
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
  const row = [0, 1, 2].map((column) =>
    median(layouts.map((layout) => layout.get(p).ratios[column])),
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
console.log(
  `  span:         ${Math.min(...every).toFixed(2)}x to ${Math.max(...every).toFixed(2)}x`,
);
console.log(`  median:       ${median(every).toFixed(2)}x`);
console.log("");
const widest = Math.max(...medians);
console.log(`widest layout median: ${widest.toFixed(2)}x`);
console.log(
  `inside the control band: ${Math.min(...every) <= widest && widest <= Math.max(...every)}`,
);
