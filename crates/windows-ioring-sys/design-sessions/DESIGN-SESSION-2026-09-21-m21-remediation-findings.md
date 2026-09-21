# Design session 2026-09-21: what remediating the epoch-log review taught

A running record of findings produced **while implementing** the M21 checklist, as distinct from
[DESIGN-SESSION-2026-09-19-epoch-log-review.md](DESIGN-SESSION-2026-09-19-epoch-log-review.md),
which recorded the review that produced the items. It exists because the next review pass should
start from what this one learned rather than rediscovering it -- including the places where the
review itself was wrong.

Updated as items complete. Entries are numbered `F-n` and never renumbered.

## The headline: a review that reads code produces claims, not findings

Two of this pass's corrections were to **the review**, not to the code it reviewed. Both were
reachability claims -- statements about which states a program can enter -- and neither could have
been settled by reading. That is the single most useful thing to carry into the next pass.

- `F-5` below: finding `C-3` asserted a latent bug that measurement showed was unreachable at any
  constants.
- `F-8`: an assertion about how an error would surface, wrong in a way only running it revealed.

A reachability claim in a review should be written as a **question with the experiment attached**,
not as a finding. "Is this reachable if `EPOCH_SIZE` exceeds `SLOTS`? -- instrument the retry path
and run it" would have cost the same to write and would not have needed correcting afterwards.

## Findings

### F-1 (M21.1) -- a sweep count taken from tool output, not from a count

The blast-radius sweep was reported as "13 files" because that is what `rg`'s summary line said;
`rg` groups two files sharing a directory prefix under one header, so the real figure was 14. The
item's own prediction ("17 matches across 10 files") was also low on both axes.

**Carry forward:** a sweep count is a claim about the tree, so it comes from a command that counts,
per FAIL FAST rule 6. Reading it off a summary line is the same defect one layer up.

### F-2 (M21.2) -- `SubmitIoRing` rejects a wait with no pending operation

`SubmitIoRing` answers `E_INVALIDARG` (`0x80070057`) -- **not** a timeout -- when asked to wait for a
completion the kernel has no pending operation for. Found because a test drove the new pop loop with
a reservation that had no real SQE behind it. Now documented on `RingWait::block`, where the
precondition holds structurally because `pop_within_with` checks `outstanding()` first.

**Carry forward:** `IoRing::run_down` makes the same call and is reachable from `Drop`. Its
behaviour when the count is non-zero but nothing is genuinely pending is worth a look in the next
pass -- see `F-4`, which is that combination actually occurring.

### F-3 (M21.2) -- a panic path introduced and closed in the same item

The first draft of `pop_within` computed `Instant::now() + timeout`, which panics on overflow, so
`Duration::MAX` -- a reasonable spelling of "no deadline" -- would have aborted the process. Closed
with `checked_add` before the commit, and covered by two tests, one of which reaches the overflow
branch rather than being answered by the early return.

**Carry forward:** the next pass should check every other public entry point that accepts a
`Duration` or a timeout for the same shape.

### F-4 (M21.2) -- a failing test can abort the harness instead of reporting

A test that panics while a reservation is outstanding unwinds into `IoRing::drop`, whose rundown
then fails (per `F-2`) and panics a second time. Rust aborts on a double panic, so the run ends with
`STATUS_STACK_BUFFER_OVERRUN` and **no test name**. Worked around here by settling every phantom
reservation before any assertion.

**Carry forward:** this is a diagnosability defect in the crate's own teardown, not only in the
tests. A `Drop` that can panic turns any unrelated test failure in the same file into an unnamed
abort. Worth a decision in the next pass: whether rundown failure should be reported some way other
than `debug_assert!` while unwinding.

### F-5 (M21.3) -- the review claimed a latent bug that does not exist

Finding `C-3` and item `M21.3` both said the epoch-commit trigger was safe at the sample's constants
but armed for anyone raising `EPOCH_SIZE` past `SLOTS`. Instrumenting the retry path to report when
the old shape would have committed produced **zero** firings at `EPOCH_SIZE` of 6, 8, 12, 16 and 24.

It is unreachable at any constants: the predicate is true at exactly two moments -- before the first
append, and immediately after a commit -- and the arena is empty at both, because the commit waits
for a covering flush that retires every outstanding write.

The change was still worth making, but as a **coupling** change: the old trigger was safe because of
an invariant three blocks away that nothing stated. Corrections landed in `C-3`, the M21 header, and
the item.

### F-6 (M21.4) -- the sample had no tests, and could not have had any

Examples are not test targets by default, so `cargo test` compiled the epoch-log sample and ran
nothing. Every claim its modules made was unbound. `test = true` on the `[[example]]` entry is what
changed that, and it applies to the whole sample rather than to the one item that needed it.

**Carry forward:** the sample's other modules -- `record`, `replay`, `reclaim`, `checkpoint`,
`strategy` -- are now testable and still untested. `replay` is the interesting one: it is the
verifier the sample's own credibility rests on.

### F-7 (M21.4) -- the failure path is unreachable without the injection seam

A commit only fails if its flush fails, and a flush against a healthy temp file does not. So four of
the six new tests are gated on `fault-injection`, following the precedent in
[fault_injection.rs](../tests/fault_injection.rs) -- including its reasoning that CI's
`--all-features` job is what stops a gated test from being a test that never runs.

### F-8 (M21.4) -- an error-surfacing assumption, wrong

The first version of the test expected an injected `ERROR_ACCESS_DENIED` to arrive as
`io::ErrorKind::PermissionDenied`. It arrives as `Other`: the crate preserves the HRESULT in an
`IoRingError` rather than classifying it. Corrected to assert the Win32 code, which is what
`tests/fault_injection.rs` already asserts.

**Carry forward:** the crate does not map Win32 codes onto `io::ErrorKind`. Whether it should is a
question for the next pass, not a defect -- but consumers matching on `ErrorKind` will match `Other`
for everything, and nothing currently says so where a consumer would look.

### F-9 (M21.4) -- the milestone compounded

`commit_and_pop` is three lines because `M21.2` published `IoRing::pop_within`. Every test in the
file would otherwise have carried its own bounded wait -- the exact duplication `M21.2` existed to
remove, reappearing immediately in the next item.

**Carry forward:** worth checking in the next pass whether the remaining hand-written waits in the
sample and in `tests/` can now collapse onto it. `M21.5` covers two of them; there may be more.

## Open questions this pass raised but did not answer

1. Should `IoRing::drop`'s rundown failure be reported some way that does not abort on unwind
   (`F-4`)?
2. Do the crate's other timeout-accepting entry points share `F-3`'s overflow shape?
3. Should Win32 codes map onto `io::ErrorKind`, given that consumers currently see `Other` for
   everything (`F-8`)?
4. Which of the sample's now-testable modules deserve tests, and in what order (`F-6`)?

None of these are queued as checklist items yet. They are inputs to the next review pass, which
should decide whether each is work or a non-issue -- **by measuring, not by reading**, which is the
lesson of `F-5`.
