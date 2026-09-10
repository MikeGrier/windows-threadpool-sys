# Completed checklists: windows-platform-probes

Append-only. Newest groups at the bottom.

## Moved 2026-09-09 19:00:17 -04:00 -- M1: a probe's report streams as it is measured

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

- [x] **M1.1** -- Decide how a formatted line reaches the sink, and build it. **Option (b): `LineSink`,
  an adapter implementing `std::fmt::Write` over a `&mut dyn Report`.** Decision and reasoning in
  [DESIGN-NOTES.md](DESIGN-NOTES.md#d-streaming-report).

  **The estimate in this item was wrong, and re-measuring it decided the question.** It said "upwards
  of 160" `writeln!` sites; there are **332** across this crate's production renderers, written into
  the `&mut String` of 18 functions. Option (a) -- a `Report` method taking `fmt::Arguments` plus a
  macro -- is the most explicit and would have rewritten all 332; that is affordable at 160 and is not
  at 332. Option (b) moves the 18 signatures and leaves the 332 untouched, because `String`
  implements `fmt::Write` too and a call site cannot tell the difference. Option (c) was declined as a
  half-measure that keeps two buffers.

  The cost this item predicted for (b) is real and is now paid: `fmt::Write` is line-agnostic, so
  `LineSink` holds a partial line and emits completed ones, and `Captured`'s one-line-per-entry
  guarantee is re-established by test rather than assumed. Seven tests pin it, including the two
  properties that are easy to get wrong -- a final `write!` with no trailing newline still emits its
  line, and `split('\n')` rather than `lines()` because only the former distinguishes a finished line
  from a partial one. Both were verified by sabotage (failing three tests and two respectively), not
  by reading.

- [x] **M1.2** -- Convert every renderer to write into the sink as it measures, and simplify
  `emit_report` accordingly. All thirteen probes now take `out: &mut dyn std::fmt::Write`; the
  `catch_unwind`/`resume_unwind` pair is deleted, because with lines leaving as they are produced
  there is no buffer to rescue and keeping it would imply partial output still depends on the panic
  unwinding. `Captured` is unchanged and its tests pass untouched.

  **Every probe in this crate needed only the signature change**, because each already went through
  `emit_report` rather than composing a `String` and calling `emit` itself. That is what the
  one-sink refactor bought, and it is why converting thirteen probes is one function plus one line
  per renderer. Three further probes under development on a branch do not hold that property and
  are converted where they land, since they are not in this crate yet.

  **Verified with a control, because several of these probes are not deterministic.** A direct
  before/after comparison flagged four of the thirteen reports, which is not evidence -- they print
  measured nanoseconds and branch their verdicts on them. Running the *same* build twice differs in
  **five**, by the same amount or more in every case: `probe-doorbell-cost` 34 lines against 30,
  `probe-request-cost` 32 against 32, `probe-pool-growth` 14 against 14, `probe-device-map` 4
  against 4, and `probe-cancel-io` 2 against **0** -- that last one being the sharpest, since a
  probe whose output varies run to run happened to match across the change and would have counted
  as evidence of no change had the control not existed. The eight reports the control showed to be
  genuinely deterministic were byte-identical. A before/after diff on a probe means nothing without
  that control.

- [x] **M1.3** -- Verify by interruption, not by reasoning. Both halves done, and the in-process half
  needed a test this item did not describe.

  **The unit test as specified would not have caught a regression.** "A renderer that panics
  mid-report still has its finished lines in a `Captured`" passes under a *buffered* report too --
  emit the buffer after catching the unwind and it holds, which is exactly what the pre-M1.2 code
  did. What distinguishes streaming is not what a reader has at the end but **when** the sink
  receives it, so `a_line_reaches_the_sink_before_the_renderer_returns` observes the sink from
  *inside* the renderer through a shared `Rc<RefCell<..>>`. Restoring the old buffered
  `emit_report_to` fails it with its own message; the panic test alone would have stayed green on
  the mechanism and gone red only on the missing catch.

  **The interruption half is measured, with a control**, and recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md). `probe-doorbell-cost` (~0.8 s, the longest-running probe
  here), stdout redirected, killed at 300 ms, six runs of each build with every run confirmed still
  alive at the kill: the streaming build captured **129 characters** (banner and heading) on all
  six, the pre-conversion build **0** on all six. The control is what makes it evidence rather than
  an observation.

  `TerminateProcess` was used rather than Ctrl-C deliberately: it runs no handler at all, where
  Ctrl-C still lets the runtime unwind its exit path, so surviving it subsumes the interactive case.
  The reason any of it works is that Rust's `Stdout` wraps a `LineWriter` and flushes at each
  newline even when redirected -- had stdout been block-buffered this milestone would have needed a
  per-line flush too.

## Moved 2026-09-09 22:54:01 -04:00 -- M2.6: what `GetFullPathNameW` does, and whether it stays

### <a id="m26"></a>M2.6 -- Say what `GetFullPathNameW` does, in the crate that owns it, and whether it stays. *(completed 2026-09-09 22:54:01 UTC-04:00)*

**Resolved.** The correction and the decision both landed in the owning crate as `D-18` in
[../windows-namespace-request-sys/DESIGN-NOTES.md](../windows-namespace-request-sys/DESIGN-NOTES.md):
`GetFullPathNameW` collapses `.`/`..` lexically but roots most paths that are not fully qualified against
process state, so it is not a lexical call as a whole; `PathCchCanonicalizeEx` does not root, and is
the wrong call for that reason, because rooting at submission is the property being bought. No cost
comparison is claimed -- the item below asked whether the alternative "would be cheaper", and the
answer recorded in D-18 is that nothing measures it, so the decision rests on semantics alone.
The mechanism question this item raised is answered rather than left open: resolving a
drive-relative path for another drive checks that drive's recorded entry against the filesystem
and writes the entry back, so the call does touch the filesystem on that form. The item's body below is the
request as it was written, and quotes the module doc as it read before the correction.

- [x] **M2.6** -- Say precisely what `GetFullPathNameW` does, in the crate that owns it, and decide
  whether it is still the call `prepare` wants. Two successive descriptions in the cost probe were
  each wrong in the same direction: *a syscall cost*, which a timing loop cannot establish, and then
  *lexical*, which it also is not. The probe now states the cost and declines the mechanism, which is
  honest but leaves the question open one layer down.

  [../windows-namespace-request-sys/src/full_path.rs](../windows-namespace-request-sys/src/full_path.rs)
  carries the same imprecision, and is the crate that owns the answer: its module doc says "This call
  is **lexical**. It resolves relative components and `.`/`..` against the process current
  directory". Those two sentences disagree -- consulting the current directory is process state, and
  for a drive-relative path (`C:foo`) it also reads the per-drive current directory held in the
  `=C:` environment variables. "Touches no filesystem" is the claim that holds; "lexical" is not.

  **The mono-repo rule says fix the layer, so the correction belongs in
  `windows-namespace-request-sys`, not in the probe that consumes it.** It is queued rather than
  taken because that crate is outside this peel and is release-managed, so a docs change there is its
  own commit with its own scope.

  The decision half is the part worth an engineer's attention rather than a sweep. A genuinely
  lexical canonicalizer exists -- `PathCchCanonicalizeEx`, or `PathAllocCanonicalize` -- and would be
  cheaper, with no process state read at all. **It is very likely the wrong call anyway**, because
  resolving against the current directory *at submission* is the property the namespace design is
  buying: the CWD is shared mutable state, so a relative path means something different depending on
  when it is resolved, and pinning that on the submitting thread is the whole point. Record that
  conclusion explicitly, with the alternative named, so the next reader does not re-derive it -- and
  if it is wrong, the cheaper call is sitting there.

  Also worth settling while the question is open: whether `GetFullPathNameW` can enter the kernel at
  all on any path this crate takes. The probe measured ~212 ns for a build on x86_64 and declines to
  say what that is made of; the owning crate could say, and a reader of either would then stop
  guessing.
