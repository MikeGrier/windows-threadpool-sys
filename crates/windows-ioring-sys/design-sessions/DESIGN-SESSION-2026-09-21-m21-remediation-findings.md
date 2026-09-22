# Design session 2026-09-21: what remediating the epoch-log review taught

A running record of findings produced **while implementing** the M21 checklist, as distinct from
[DESIGN-SESSION-2026-09-19-epoch-log-review.md](DESIGN-SESSION-2026-09-19-epoch-log-review.md),
which recorded the review that produced the items. It exists because the next review pass should
start from what this one learned rather than rediscovering it -- including the places where the
review itself was wrong.

Updated as items complete. Entries are numbered `F-n` and never renumbered. M21 is complete as of
2026-09-21; F-1 to F-12 are its whole record.

## Headline 2: an independent review found what this pass could not

After M21 closed, a fresh reviewer audited the `M21.2` public surface and found a **High**-severity defect
in it, plus a pre-existing one of the same root cause, plus the reason neither was caught. All four are
fixed in `M21.6`; `F-13` to `F-15` are what they taught.

The uncomfortable part is not that the review found defects. It is *which* defect: the author of that API
had written its tests, sabotage-verified them, and reported the sabotage in the commit message -- and the
sabotage that mattered (delete the real wait entirely) was never run, because the tests had been
restructured away from the real wait on purpose. **Self-review could not have found this, and did not.**

## Headline 1: a review that reads code produces claims, not findings

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

### F-10 (M21.5) -- the item named two sites; a census found six

`M21.5` was written as "give strategy.rs's two wait loops a bound". Counting by command over every `.rs`
outside `target/` and the spikes found **four** unbounded wait loops and **two** more of a related
shape. Two of the four were helpers *both named `await_one`*, byte-identical, in
[failure_paths.rs](../tests/failure_paths.rs) and [kernel_span.rs](../tests/kernel_span.rs) -- neither
mentioned by the item or by the review.

The other two were in [batch/tests.rs](../src/batch/tests.rs): registration waits written as a single
`try_pop`, which is the flake shape `pop_within` documents, in a file whose **third** such wait already
used the helper. One predicate, three sites, half-converted -- FAIL FAST rule 1 exactly, inside a single
file.

**Carry forward:** the review found these by reading one sample, so it found what that sample contained.
A census by command is cheap and finds the population. Every item in the next pass whose subject is a
*shape* rather than a specific line should carry its census command.

### F-11 (M21.5) -- the milestone compounded again, and measurably

Sabotaging `Lane::classify` to stop filing flush results leaves `await_flush` waiting for a completion
that is never recorded. An unbounded loop hangs forever there. The new bound reported
`timed out after 30s waiting for a commit's flush` -- **in two seconds**, because `pop_within`'s
nothing-can-arrive early return (`F-9`, `M21.2`) answers immediately once the ring is quiesced.

The bound is what makes the failure possible; `M21.2`'s early return is what makes it quick. Neither was
designed with the other in mind.

### F-12 (M21.5) -- duplicated helpers do not share a name by accident

Two independently written helpers, in two files, both called `await_one`, both the same eight lines. The
name being identical is the tell: it is what people call this operation, which is the argument for the
operation belonging to the library. It now does.
### F-13 (M21.6) -- the crate's tests never exercise asynchronous completion

Measured while building a test that needed a genuinely pending operation. **A file handle opened without
`FILE_FLAG_OVERLAPPED` is synchronous, so a ring operation against it completes inline during submit.**
Every fixture in this crate's tests, examples and samples opens its handle that way.

The consequences are larger than the item that found it:

| Attempt at a slow operation | Measured |
|---|---|
| Buffered read, up to 256 MiB | 3-5 us -- already poppable |
| Flush over 512 MiB of dirty cache | 3 us -- lazy writer got there first |
| Unbuffered read, 256 MiB, synchronous handle | 3 us -- completes during submit |
| Unbuffered **and** overlapped, 64 MiB and up | genuinely pending |

So the suite has been testing the *synchronous* completion path almost exclusively. A counting waiter over
the existing flush pattern was reached in **0 of 50 trials**.

**Carry forward, and this is the big one for the next pass:** every claim this crate makes about ordering,
draining, the completion event, and the barrier was measured against operations that may have completed
inline. D-19, D-23, D-24 and D-47 all deserve re-reading with that in mind. The drain-ordering spike used
`NO_BUFFERING` and pre-written extents deliberately, so it is probably fine -- but *probably* is exactly
the word that needs replacing with a measurement.

### F-14 (M21.6) -- deterministic tests and honest tests are not the same thing

The M21.2 tests were restructured onto a wait that never enters the kernel, precisely to make the loop's
deadline behaviour deterministic. That was reported in the commit message as a virtue. It was also what
let a real defect through: replacing `RingWait::block`'s whole body with an unconditional error left the
entire suite green, because nothing ever reached it.

The isolation was correct for what it tested. The error was not adding anything that drove the real thing
alongside it -- and then describing the isolation as coverage.

**Carry forward:** when a test double is introduced to make something deterministic, the same change owes
a test that exercises the real implementation. A mutation that deletes the real implementation should fail
something.

### F-15 (M21.6) -- an error-vs-timeout mapping is a contract, and Win32 gets it backwards

Every Win32 wait reports an expired bound as a *failure* code -- `ERROR_TIMEOUT` from `SubmitIoRing`,
`WAIT_TIMEOUT` from the `WaitFor*` family. Any wrapper that forwards its underlying result verbatim
therefore turns an ordinary timeout into an error, and any API that documents "`Ok(None)` means the bound
expired" is wrong the moment it does so.

`CompletionWait` had not said which way to report it, so every third-party implementation would have
reproduced the defect independently. It says so now.

**Carry forward:** check every other place this crate converts a Win32 wait result. The `WaitFor*` calls in
`event_loop.rs` and `model_b_multiplexed.rs` already handle `WAIT_TIMEOUT` explicitly; whether anything
else forwards a wait result blindly is worth a census.
### F-16 (M21+.1) -- the checker had a latent bug that only a probe could find

Widening `check-borrow-surface.ps1` was verified with five probes rather than by re-reading the regex, and
one of them crashed it: a one-line body -- `pub fn f() -> &[u8] { &[] }` -- never satisfied the "line ends
with `{`" test, so the signature accumulator ran off the end of the file. The *old* script did not crash on
that shape only because it never indexed the lines again afterwards; it silently swallowed the following
lines instead, which means it could have been skipping real signatures all along.

**Carry forward:** a checker is code, and the argument for testing it is the same as for anything else. The
negative control matters most -- a check that fires on everything is as useless as one that fires on
nothing, and only the plain-`&T` probe establishes that this one still discriminates.

### F-17 (M21+.1) -- the blind spot had already swallowed something real

The widened check immediately reported `IoRingErrorExt::as_ioring_error -> Option<&IoRingError>`, a public
trait method returning a borrow that predates the review by months and had never been inventoried. It is
not a hole -- the borrow is of the `io::Error` the caller owns -- but it was never *put to anyone*, which
is the whole function of the inventory.

**Carry forward:** when a check is found to be narrow, assume it has already been narrow for a while and
look at what it let past, rather than only at the change that exposed it.

### F-18 (M21+.1) -- the count sweep missed the tool built to stop drift

The script's header and its failure message both said **three** shipped defects of this shape, listing
D-35, D-36 and D-43. It has been four since D-45. `M19.3` explicitly swept that count -- its archive records
"that file said 'three defects' in four places and is now four" -- and swept `DESIGN-INSTRUCTIONS.md` while
missing `check-borrow-surface.ps1`.

**Carry forward:** the sweep looked at documentation and not at tooling. A `.ps1` file carrying prose is
still prose, and the next census of any restated fact should include `tools/`.
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
