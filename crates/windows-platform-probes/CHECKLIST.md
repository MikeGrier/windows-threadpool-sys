# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M1 -- Stream a probe's report as it is measured

The report sink introduced with [src/report.rs](src/report.rs) has each renderer compose its whole
report into a `String`, which `emit_report` then hands to a [`Report`]. That buys the seam the crate
wanted -- a probe's findings can be asserted rather than eyeballed -- and it gave up a property the
previous line-by-line `println!` had for free: output appearing as it is measured.

`emit_report` recovers it for an **unwinding panic** only, by catching, emitting what was composed, and
resuming. A termination that does not unwind still discards the buffer:

- **Ctrl-C.** The default Windows console handler terminates the process; no unwind runs.
- **Abort from a panic raised while already unwinding.**

The case that costs most is `probe-cancel-io`: four attempts against a five-second watchdog, so about
twenty seconds, and it runs that long *precisely when the wedge it hunts for occurs* -- which is
precisely when a reader gives up and interrupts. The measurement most worth having is the one most
likely to be thrown away.

See [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [The report is buffered, and what that
costs](DESIGN-NOTES.md#d-buffered-report) for why it was built this way and why the fix is a separate
piece of work rather than a correction to that one.

- [ ] **M1.1** -- Decide how a formatted line reaches the sink, because that choice is what makes the
  rest mechanical. Every renderer today writes through `let _ = writeln!(out, ...)` against a
  `String`'s `fmt::Write` -- roughly 156 sites across the eight probes -- so the sink must accept
  *formatted* output, not just `&str`, or every site grows a `format!` and an allocation per line.
  The options differ in what they cost callers, and the choice is the engineer's:
  (a) give `Report` a method taking `fmt::Arguments` plus a `report_line!` macro, so a call site stays
  one line and reads almost as it does now;
  (b) implement `fmt::Write` for the sink types, so `writeln!(out, ...)` keeps working verbatim against
  a `&mut dyn Report` -- smallest diff at the call sites, but `fmt::Write` is line-agnostic, so the sink
  must split on newlines internally and `Captured`'s one-line-per-entry guarantee has to be re-established
  rather than assumed;
  (c) leave the renderers writing to a `String` and flush it to the sink at each line boundary, which
  streams without touching the call sites but keeps two buffers.

- [ ] **M1.2** -- Convert the eight renderers to write into the sink as they measure, and simplify
  `emit_report` accordingly: once lines leave as they are produced, catching the unwind is no longer
  what makes partial output work, and the `catch_unwind`/`resume_unwind` pair should be removed rather
  than left as machinery that no longer earns its place. Keep `Captured` working -- it is what every
  in-process test asserts against.

- [ ] **M1.3** -- Verify by interruption, not by reasoning. Sending Ctrl-C to a probe part-way through
  must leave the already-measured lines on the terminal; today it leaves nothing. Assert the in-process
  half (a renderer that panics mid-report still has its finished lines in a `Captured`) as a unit test,
  and record the Ctrl-C observation in [DESIGN-NOTES.md](DESIGN-NOTES.md) -- an interactive signal is not
  something to assert in CI, but it is the property the milestone exists for, so it must be measured
  once rather than assumed.
