# The permitted kernel response space

What `windows-ioring-sys` will tolerate from the platform, stated as a
specification rather than recorded from a run.

This document is normative. `M26.3`'s resolver generates resolutions **from
this space**, `M26.4`'s properties must hold under every one of them, and the
kernel tests confirm that a real Windows stays **inside** it (`M26.6`). Every
clause carries an ID so those three can cite the clause rather than restate it
-- and [response_space_census.rs](tests/response_space_census.rs) fails when a
clause is claimed by nothing on the side that owes it a check, so the division
of labour is enforced rather than merely described.

## What this is, and what it deliberately is not

**It is not a model of what Windows does.** A model of observed behaviour
freezes one run's testimony, which is the trap
[D-52](DESIGN-NOTES.md#d-52) was opened to escape and the objection that
kept a fake out of this crate for two milestones. A resolver built on a model
asserts something about the kernel and can be wrong about it.

**It is a statement of what this crate will tolerate.** The resolver asserts
nothing about Windows. It picks a point in the space below, and the assertions
are about *us*: does this crate behave correctly under that resolution. There
is no belief here to be wrong about -- only a specification that can be too
narrow, which is a reviewable defect rather than a hidden one.

**It is wider than anything observed, on purpose.** Deriving the space from
observation would close the trap again. Where a clause goes beyond what any
spike has seen, it says so, because the reader's first question about a
permissive clause is whether anyone has watched it happen.

## How to read a clause

Every clause has an ID, a statement, a source, and a provenance tag:

- **Observed** -- a spike or a measurement in this repository saw it happen.
  The citation says which.
- **Documented** -- Microsoft states it on the API's reference page. This is
  the strongest tag, and it outranks the others: a measurement describes one
  run of one build, while a documented return value is what the platform
  commits to. Where a clause carries both, the documentation is the reason and
  the measurement is corroboration.
- **Over-provision** -- wider than anything observed here, allowed
  deliberately. These are the clauses that make the space a specification
  rather than a recording.
- **Decided** -- a constraint this crate chooses to require of the platform.
  Not measured, not derived; a call, and reviewable as one.

A clause tagged **Observed** may still be wider than its observation. Where
that is so it is split, so the measured part and the extrapolated part can be
argued separately.

## Permitted: what a resolver may do

### RS-P-1 -- An operation may complete inside `SubmitIoRing`, or pend

Each operation in a submitted batch resolves independently as either *already
complete when `SubmitIoRing` returns* or *outstanding*.

- **Observed.**
  [write-pending-spike.rs](design-sessions/spikes/write-pending-spike.rs)
  measured both outcomes, and measured the mix varying with handle flags and
  with whether the extent was written beforehand -- see
  [2026-09-24-set-len-vs-zero-fill/](measurements/2026-09-24-set-len-vs-zero-fill/README.md),
  where a buffered handle pended in almost none of 16,000 trials and an
  unbuffered one pended in most runs.
- **Over-provision:** the resolver chooses **per operation**, independently,
  with no rate and no correlation to handle flags. No measurement established
  that operations within one batch resolve independently. The space permits it
  because a consumer that depends on them resolving together is depending on
  something Windows never promised.

### RS-P-2 -- Completion order is unconstrained

Completions may be posted in any order, and that order need bear no relation to
submission order.

- **Observed, in part.**
  [D-47](DESIGN-NOTES.md#d-47) measured operations queued *after* a drained
  flush completing *before* it, at 0.03%-0.8% depending on conditions, with all
  32 overtaking in the worst observed trial.
  [kernel-response-space-probe.rs](design-sessions/kernel-response-space-probe.rs)
  then built a seeded resolver that permutes completion order and ran a
  FIFO-assuming consumer against it: broken under 189 of 200 seeds, first at
  seed 0, where completions arrived as `[3, 1, 4, 2]`. That probe is the
  working demonstration this clause and `M26.3` are both built on; the
  [session](design-sessions/DESIGN-SESSION-2026-09-22-kernel-response-space.md)
  is its write-up.
- **Over-provision:** any permutation, not merely the reorderings observed.
  This is the clause the `D-47` defect class lives in, and the reason it is
  total rather than bounded is that a bound derived from observed rates is a
  recording.

### RS-P-3 -- An operation may fail individually

Any single operation may complete with a failure result while others submitted
beside it succeed.

- **Documented.** `SubmitIoRing`'s Remarks state the mechanism: *"Any errors
  processing a single submission queue entry results in a synchronous
  completion of that entry posted to the completion queue with an error status
  code for that operation."* So a per-entry failure is **not** a submit
  failure; it arrives as an ordinary completion carrying an error. That is the
  contract, and the two observations below are consistent with it rather than
  the basis for it.
- **Observed.** Two independent places in this repository are built around it.
  [`checkpoint.rs`](examples/epoch_log/checkpoint.rs) documents the case
  directly -- a record write failing with `ERROR_DISK_FULL` followed by a flush
  that "completes perfectly happily" -- which is why that control plane checks
  its write's result separately from its flush's. And on the *append* path,
  `M22.2` found an ordering defect in handling exactly this: a failed write's
  token was not claimed, so its arena slot leaked and the failure surfaced
  `SLOTS` appends later with no trace of the cause.
- **Over-provision:** any error code, at any position in the batch, including
  the case where every operation fails. The failure *codes* the platform
  actually returns are not enumerated here and a resolver must not depend on
  the set being small.

### RS-P-4 -- A wait may expire

A submit-and-wait may return `HRESULT_FROM_WIN32(ERROR_TIMEOUT)` having waited
its full timeout, and this is **not** a failure of the ring.

- **Observed.** `M21.6` fixed a defect in this crate that treated an expired
  wait as an operation failure; `ring.rs`'s `IORING_E_WAIT_TIMEOUT` handling is
  the correction, and it maps the code to `Ok(())`. `M26.8` found the same
  defect surviving in [`Batch::submit_and_wait`](src/batch.rs), which that
  sweep had not reached.
- **Documented, and the documentation says more than "not a failure".**
  `SubmitIoRing` gives this code its own return-value row: *"All operations
  were submitted without error and the subsequent wait timed out."* So it
  carries a positive guarantee about the submission half, not merely the
  absence of a failure -- which is why it is classified separately from every
  other error rather than folded in with them.
- **Over-provision:** a wait may expire even when completions are available,
  and may expire on any call including the first.

### RS-P-5 -- A wait may return successfully with nothing poppable

A wait that returns success does not promise that a subsequent pop yields
anything.

- **Observed.** This crate's own `pop_within` documentation states it -- a
  submit-side wait's return "promises nothing about poppability" -- and
  [D-19](DESIGN-NOTES.md#d-19) measured the completion event as **edge**
  triggered on the queue going empty to non-empty, so a signal is not a count
  and the queue can be drained by the time a waiter looks.
- **Over-provision:** this may happen on any wait, any number of times in
  succession. A resolver is not required to make progress on any particular
  call, only to satisfy RS-C-1 eventually.

### RS-P-6 -- A signal may not arrive for a completion posted while the queue was already non-empty

The completion event fires on the empty-to-non-empty edge, so a completion
arriving behind another need not produce its own signal.

- **Observed.** [D-19](DESIGN-NOTES.md#d-19), measured, and
  [D-21](DESIGN-NOTES.md#d-21) is the consequence this crate drew from it --
  auto-reset, exactly one waiter per ring.
- **Over-provision:** none. This clause is the measurement.

### RS-P-7 -- A submit may fail, leaving already-built operations queued for a later submit

`SubmitIoRing` may fail, and when it does every entry it was asked to submit
remains in the submission queue. A later, unrelated submit is what runs them.

- **Documented, which `M26.8` established and `M26.1` had not.** This clause
  was originally written as a *consequence* -- "if a submit fails, entries
  remain queued" -- citing [D-5](DESIGN-NOTES.md#d-5), which establishes the
  no-rewind consequence and nothing about submits failing at all. `M26.3`'s
  resolver read it as a permission to fail submits, and the space had no
  authority for that. It does now: `SubmitIoRing`'s return-value table lists
  *"Any other error value: Failure to process the submission queue in its
  entirety"*, and its Remarks state *"If this function returns an error other
  than IORING_E_WAIT_TIMEOUT, then all entries remain in the submission
  queue."* Both halves of this clause are therefore Microsoft's, not an
  inference from a run.
- **The `IORING_E_WAIT_TIMEOUT` carve-out is part of the clause**, because it
  is the case where the entries did *not* remain queued: that code means every
  operation was submitted and only the wait expired. A consumer that cannot
  tell the two apart cannot know whether its buffers are still owed to the
  kernel, which is the defect `M26.8` fixed in
  [`Batch::submit_and_wait`](src/batch.rs).
- **Over-provision:** the resolver may defer an operation across any number of
  submits, not only across a failed one. It currently declines a submit only
  when something is staged, which is *narrower* than this clause -- nothing
  says a submit carrying no new work cannot fail.

## Constrained: what a resolver may not do

These exist because a resolver free to violate everything makes this crate
defend against a platform that does not exist, and code written against an
impossible kernel is untestable and unreviewable. Each is a call.

### RS-C-1 -- Every submitted operation eventually completes exactly once

No completion is lost, none is duplicated, and every successfully submitted
operation eventually produces exactly one completion.

- **Decided**, not observed. Nothing here has measured the negative, and it
  could not be measured in bounded time.
- **Why:** this crate's accounting is driven by observing a real `IORING_CQE`
  ([D-4](DESIGN-NOTES.md#d-4)) and its [`RingContract`](src/contract.rs) oracle
  states conservation directly. A platform that lost completions would make
  every consumer's outstanding count unbounded and every wait a guess. If this
  is ever observed to fail, the finding is a contract defect in Windows and not
  a gap in this space.

### RS-C-2 -- A completion identifies the operation that produced it

A completion's `UserData` is the value supplied when the operation was built.

- **Decided.** The alternative is that operation identity is unusable, which
  would invalidate [D-4](DESIGN-NOTES.md#d-4)'s whole accounting model and
  `M28`'s pending inventory with it.

### RS-C-3 -- An operation does not complete before it is submitted

- **Decided**, and stated because a resolver that may post a completion for an
  operation still being built would make the `Build*`/`Submit` boundary
  meaningless.

### RS-C-4 -- The drain half of `DRAIN_PRECEDING_OPS` holds

No operation queued **before** a flush carrying
`IOSQE_FLAGS_DRAIN_PRECEDING_OPS` completes after that flush.

- **This is the explicit call `M26.1` demanded, and it is the one place this
  space is narrower than "anything may happen".**
- **Observed, with the strongest evidence in this repository.**
  [D-47](DESIGN-NOTES.md#d-47) measured roughly 4,500 trials in which **not
  once** did an operation queued before a drained flush complete after it --
  the same campaign that falsified the *other* half of `D-24`.
- **Why constrained:** the drain is the documented guarantee this crate's
  durability story rests on ([D-23](DESIGN-NOTES.md#d-23)). A resolver
  permitted to break it would require every consumer to re-verify durability by
  some other means, which is to say it would make the primitive useless. The
  cost of this call is that a Windows which broke the drain would not be caught
  by the resolver at all -- it is caught by the kernel tests instead, which is
  the division of labour they were repointed to in `M26.6`. That is now a
  mechanical arrangement rather than an intention:
  [flush_barrier.rs](tests/flush_barrier.rs) carries a `CONFIRMS: RS-C-4`
  marker and asserts the clause against a real ring on every machine, and
  [response_space_census.rs](tests/response_space_census.rs) fails if that
  marker ever disappears.
- **Note what is *not* constrained:** the hold-back half.
  [D-24](DESIGN-NOTES.md#d-24) claimed the flag holds back what follows and
  [D-47](DESIGN-NOTES.md#d-47) withdrew that claim, so RS-P-2 applies in full
  to operations queued *after* a drained flush. The one-sidedness is the whole
  point of the pair.

## What this space deliberately leaves undecided

Stated so that a later reader can tell an omission from a choice:

- **Rates.** No clause carries a probability. A resolver weights its choices by
  seed, and any weighting is a property of the resolver rather than of this
  space. Recording observed rates here would make the space a recording.
- **Partial transfers.** Whether a completion may report fewer bytes than
  requested is not specified, because nothing in this repository has measured
  it and the over-provision would change what every consumer must handle.
  Decide it before a resolver relies on either answer.
- **Failure code sets.** RS-P-3 permits any code and enumerates none.
- **Timing.** Nothing here constrains how long anything takes. `M25`'s standing
  constraint already forbids this crate from depending on an operation pending,
  and a space that specified durations would invite exactly that.

## Changing this document

A clause moving from **Over-provision** to **Observed** is an improvement and
needs only its citation updated. A clause moving in the other direction, or a
constraint being relaxed, changes what this crate promises to tolerate and
must be recorded as a decision in [DESIGN-NOTES.md](DESIGN-NOTES.md) with the
finding that forced it.
