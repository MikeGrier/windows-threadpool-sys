# Design notes: windows-platform-probes

Decisions for this crate. Pending work is in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md), milestone M27.

## A probe is a function that returns an observation, never a program that prints one

<a id="d-derived-not-restated"></a>

Each probe's logic lives in a library function returning a structured result.
The binaries print that result and the tests assert it, so the fact has exactly
one implementation.

The alternative -- a binary that prints and a test that separately re-checks --
is the restatement failure this repository has already paid for, in its most
concentrated form: a hand-written second copy of a platform check is not a check
of the platform, it is a check of the copy. When the two disagree, nothing
detects it.

## Three tiers, because "run all the probes" is not a safe instruction

<a id="d-three-tiers"></a>

Some of the behaviour worth measuring is actively hostile to an ordinary test
run. The `CancelSynchronousIo` measurement recorded in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) *never returns* -- that is the finding.
A thread-pool growth measurement spends seconds and 512 threads. An `IoRing`
measurement moves 512 MiB. A device-map measurement needs `subst` drives and a
second logon session.

So each probe declares a tier: **asserted** (a real test), **ignored**
(assertable but too slow, too heavy, or environment-dependent), or **binary
only** (cannot be a test at all). Every tier is compiled by an ordinary build,
which is the floor -- a probe that no longer compiles has already rotted.

Placing a probe in the wrong tier is the failure mode to watch: an asserted probe
that is slow or flaky will be deleted by whoever it blocks, taking the
measurement with it.

## An asserted probe must not outlive its own call

<a id="d-no-lasting-side-effects"></a>

This is the rule that decides tier membership, and it has already changed a
probe's design. The natural way to show that the thread error mode is
independent of the process error mode is to set `SEM_NOALIGNMENTFAULTEXCEPT` at
process scope and observe that it does not appear in the thread mode. That bit
is **sticky**: Windows ignores every later attempt to clear it, so the test
process would be permanently altered, and any other test in the same binary would
inherit the change.

[`thread_mode_independent_of_process`](../../crates/windows-platform-probes/src/error_mode.rs)
therefore demonstrates the same property with a **reversible** bit, which proves
exactly as much. The stickiness itself is real and worth recording, so it is
measured by
[`alignment_bit_is_sticky_at_process_scope`](../../crates/windows-platform-probes/src/error_mode.rs)
-- binary-only, and documented as irreversible at the call site.

## A probe asserts its own controls, and refuses to pass vacuously

<a id="d-controls"></a>

Two guards, both from measurements in this repository that read as passes while
measuring nothing.

**A control is part of the probe, not a separate courtesy.** "A duplicated handle
continued the enumeration" says nothing on its own -- perhaps any handle
continues. The paired measurement, that two *separate opens* do not continue each
other, is what makes the first attributable, so it is asserted as its own test
rather than left to a reader's inference.

**A fixture that cannot exhibit the behaviour is a failure, not a pass.**
[`ground_truth`](../../crates/windows-platform-probes/src/handle_state.rs) panics
if the whole directory fits in one call, because every cursor question below it
would then be vacuous. The buffer is small deliberately, and the assertion is
what keeps it small when someone later "tidies" the constant.

## Scope

Measurements of **platform** behaviour that a design decision rests on. Not a
test suite for this workspace's crates: a probe answers "what does Windows do?",
never "does our code work?". A probe that starts asserting our own behaviour
belongs in the crate that owns that behaviour.

## The earlier probes are migrated, and two of them corrected in the move

<a id="d-migration"></a>

M27.4 moved the nine measurements that existed only in a git-ignored
`.scratch/` directory. They are now `worker_context`, `pool_growth`,
`device_map`, `cancel_io`, and `ioring`, alongside the `error_mode` and
`handle_state` probes that established the scheme.

Two things changed in the move, and both are worth recording because they were
defects in the originals rather than translation choices.

**The device-map control could not have passed.** It compared the logon-session
LUID of the two contexts, reading the *thread* token -- which the
non-impersonating side does not have. So the control reported "these are not
different sessions" no matter what, and a reader checking it would have been
misled into distrusting a correct finding. It now falls back to the process
token, and the two LUIDs differ as they should. A control that cannot succeed is
worse than no control, because it looks like one.

**The `IoRing` registration probe must not use `windows-ioring-sys`.** That
crate refuses a second registration *because of* the assumption being measured,
so probing through its safe API would test the guard rather than the platform --
confirming our own belief by consulting it. The probe therefore calls the Win32
entry points directly. This is the same circularity the contract-integrity rule
names, in the one place where it would have been easiest to miss: the code under
test and the thing asserting it would have been the same claim.

That probe also closes a standing gap. `windows-ioring-sys` recorded its
replace-not-append assumption as explicitly **unverified**; it is now measured,
and it holds.

## "Cannot measure" is a third answer, and is not "no"

<a id="d-cannot-measure"></a>

Several probes need something a host may not have: an `IoRing`, a free drive
letter. Each reports that it could not run rather than returning a negative,
because conflating the two is exactly how a design note ends up citing a
measurement that never happened.

The distinction has teeth in the ignored tier. A test that cannot set up its
fixture returns early; a test whose fixture *is* set up but does not exhibit the
behaviour **fails**. `the_subst_letter_really_was_visible_before_impersonating`
is that check written down: if the drive never resolved in our own session, then
"not found while impersonating" would be true of any letter at all, and the
finding would be vacuous.

## Binaries print magnitudes; tests assert shape

<a id="d-shape-not-magnitude"></a>

The pool-growth migration made the reason concrete. Growth is **not uniform**:
on the measuring host an initial burst of workers arrives in under 500
microseconds, and beyond that the pool adds roughly one thread per 165
milliseconds. A design sizing a stall threshold from the burst would be badly
wrong about the tail.

Neither number belongs in an assertion -- both are host-specific, and a test
pinned to them would fail on the next machine for no useful reason. The *shape*
is what survives a change of host, so the test asserts that there is a burst
followed by a visibly slower regime, and `probe-pool-growth` prints the
magnitudes for a human to read.
