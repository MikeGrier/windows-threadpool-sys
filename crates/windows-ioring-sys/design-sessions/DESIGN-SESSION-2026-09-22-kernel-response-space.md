# Design session 2026-09-22: the kernel response space

**Decisions resulting from this session:** D-52 (the resolver technique and what it is for), and an
amendment to "Two techniques deliberately rejected" in [DESIGN-NOTES.md](../DESIGN-NOTES.md). The work
it queues is `M26` in [CHECKLIST.md](../CHECKLIST.md); `M24.1` is answered and `M24.4` is withdrawn.

The apparatus built during the session is kept as
[kernel-response-space-probe.rs](kernel-response-space-probe.rs). It is a **demonstration**, not a
test: drop it into `tests/` and run with `--nocapture` to reproduce every figure below.

## What the session set out to do

`M24.1` asked one question: does a fake whose assertions are **shared** with the kernel escape the
objection that led this crate to reject a mock? That objection is specific rather than generic --

> Both shipped defects were the kernel behaving differently from this crate's assumptions. A mock
> *encodes* the assumption, so one written before those discoveries would have passed both bugs green
> -- it would not merely have failed to find them, it would have manufactured evidence they were
> absent.

The item insisted the question be settled by demonstration, "because the argument is exactly what is
in doubt", and predicted two outcomes: a wrong **accounting** model would be caught by a shared suite,
and a wrong **Windows belief** would not.

Both predictions held. Then two further cases changed the answer.

## Case 1 and 2: the predictions, confirmed

A five-method slice (`push_one`, `submit`, `try_pop`, `outstanding`, `pop_within`), one generic
assertion suite, two implementations -- a real `IoRing` over a temp file, and an in-memory fake.

| fake's flaw | shared suite |
|---|---|
| none | GREEN, and the kernel agrees |
| `outstanding` decremented at push instead of at pop | **RED** -- caught |
| pops without submitting | GREEN -- slipped |
| pre-`M21.6`: an expired wait is an error | GREEN -- slipped |

The third flaw is the one worth dwelling on, because it is not invented: it is the belief this crate
actually held until `M21.6`. `SubmitIoRing` reports an expired wait as `ERROR_TIMEOUT`, a *failure*
HRESULT, so `pop_within` returned `Err` on every ordinary timeout. A fake written before that
discovery would have encoded exactly this.

Add a timeout assertion and the fake is caught instantly. But **that assertion could only be written
after the kernel had already revealed the answer.** The fake could never have produced it.

## Case 3: the argument *for* co-testing, which the item did not predict

The first pass missed the case that decides it. What happens when the shared suite itself encodes the
wrong belief, and is run against both?

| | timeout assertion written from the pre-`M21.6` belief |
|---|---|
| kernel | **RED** -- "an expired wait returned `Ok(None)`, not the error we expected" |
| fake built from the same belief | GREEN |

The kernel **refutes** us. The fake **confirms** us. That is the manufactured-evidence mechanism made
visible -- and it is also the escape, because running a shared assertion against the kernel is how a
wrong belief gets contradicted. A mock-only world never performs that experiment.

So after three cases the conclusion was: the rejection stands for mocks-as-substitutes, a co-tested
peer is admissible for a narrow category, and the bright line is "accounting versus Windows
behaviour".

**That line was wrong**, and the next case is why.

## Case 4: an assertion that looks like a contract and is a frozen observation

Raised in review: *the kernel's behaviour is not necessarily reproducible run to run. When we observe
it, that is an observation at a point in time, not a record of objective truth. We must not
over-index on a record of how it runs as being "right".*

Take the assertion "after submitting, the completion is already queued". It reads like a contract.
Run it against two handles of the same API:

| | "completion is already queued after submit" |
|---|---|
| kernel, buffered handle | GREEN |
| kernel, `NO_BUFFERING` + `OVERLAPPED`, pre-written extent | **RED** -- "nothing was queued when submit returned" |
| fake | GREEN -- it encoded whichever one its author saw |

Opposite answers on the same API. So the assertion was never about the ring; it was about a handle, a
filesystem and a moment. And the run-to-run half was measured the same day:
[write-pending-spike.rs](spikes/write-pending-spike.rs)'s `NO_BUFFERING`-extending condition reported
**5/500 in one run and 271/500 minutes later**, same binary, same machine.

Restate the same question as *our* contract -- "the completion arrives within a bound we specify" --
and all three go green. That form is robust because it is a statement about what this crate promises
rather than about when the kernel happens to finish.

### The ratchet

The danger is worse than one bad test, and it compounds:

1. Observe the kernel once.
2. Freeze the observation into a conformance assertion.
3. Build the fake to satisfy that assertion.
4. Three artifacts now agree -- and the agreement reads as corroboration when it is **one observation
   restated three times**.

That is CONTRACT INTEGRITY rule 1 ("a hand-written second copy of a contract rule is not a check of
the contract, it is a check of the copy") applied to platform behaviour rather than to our own. This
crate has already paid for it once: [D-47](../DESIGN-NOTES.md#d-47) is exactly this failure, where a
handful of runs showed a barrier holding, that was written down as a guarantee, and the real violation
rate was nearer one in a thousand.

### The corrected line

Not "accounting versus Windows behaviour". The axis is:

**our specified contract, versus the platform's incidental behaviour.**

- `pop_within` returns `Ok(None)` on an expired wait -- **ours**. The kernel says `ERROR_TIMEOUT`; the
  wrapper translates. Legitimate to assert, and legitimate for a fake to encode.
- "`SubmitIoRing` reports `ERROR_TIMEOUT`" -- **an observation**. Dated, machine-specific, possibly not
  reproducible. It belongs in a spike with a rate and provenance, never as a pass/fail assertion.
- "the completion is queued when submit returns" -- **an observation wearing a contract's clothes**,
  which case 4 demonstrates.

This is why Design Autonomy is a repository rule: we define our behaviour and choose dependencies that
satisfy it. The wrapper is the layer that absorbs kernel variation, and the conformance suite asserts
what the wrapper promises -- which is precisely what a fake can faithfully implement.

## Case 5: the reframing, and the actual answer

Also raised in review, and it is a different technique rather than a refinement:

> Our code needs to work in light of all the possible ways that the platform may respond to the rings
> we submit. Some seed-derivable selection of a set of resolutions to what a given epoch's IoRing
> would turn into in terms of synchronously versus asynchronously completed items. Input state plus a
> seed gives a set of kernel responses, and we verify that `windows-ioring-sys` responds correctly to
> the kernel stimuli.

The fake stops modelling **what Windows does** and starts modelling **what Windows is permitted to
do**. A seed picks one resolution out of that space: which operations finish inside `SubmitIoRing`
and which pend, in what order completions are posted, which fail.

**This dissolves the mock objection rather than working around it**, because there is no belief to be
wrong about. The resolver asserts nothing about the kernel. The assertions are about *us*: does this
crate behave correctly under this resolution.

It also inverts the problem case 4 raised. Non-reproducibility stops being a threat and becomes the
expected case -- Windows exercising a different point in a space the tests already sweep. A run-to-run
change like 5/500 to 271/500 is two samples from a space covered by construction.

A minimal resolver in the probe -- SplitMix64 over one seed, permuting completion order -- was run
against a consumer that assumes completions arrive in submission order:

```
a consumer assuming FIFO completion order:
  broke under 189 of 200 seeds
  first at seed 0: completions arrived as [3, 1, 4, 2], not [1, 2, 3, 4]
```

Some seeds pass and some fail, which is the point. A fixed fake reports whichever single answer it
encoded.

## What this does and does not add to the toolkit

[DESIGN-NOTES.md](../DESIGN-NOTES.md) already draws the boundary this sits on:

> All five techniques check this crate's code against **this crate's stated contract**. None of them
> can tell you the stated contract is wrong.

The resolver is a sixth technique, and it does something none of the five do: it checks the code
against a **space** of platform behaviours rather than against one. It still cannot tell you the
stated contract is wrong -- only a spike does that. What it can tell you is that the code is brittle
to variation *inside* the space, which nothing in the toolkit currently detects.

It composes with what exists rather than replacing it.
[generated_sequences.rs](../tests/generated_sequences.rs) (M17.3, `D-41`) already generates the
**input** space and runs it against the real kernel; the resolver generates the **response** space.
Both seeded, both replayable from one number, and together a two-dimensional exploration.

## The hard part, which is the whole design

**Where the permitted space comes from.** Derive it from observation and the trap closes again. It has
to be a deliberate specification -- "we will tolerate these behaviours" -- written wider than anything
observed, on purpose. That makes this crate's model of Windows an explicit, reviewable, versioned
artifact instead of an accident of whichever machine ran the tests last.

Three consequences that should be decided rather than defaulted:

1. **Constraints must be modelled too, or the tests demand over-defensive code.** `D-23`'s
   covering-flush guarantee held with zero failures in roughly 4,500 trials. If the resolver may
   violate it, we would write code defending against a kernel that breaks a documented guarantee.
   Making that an explicit call is the improvement; it is still a call.
2. **The seam is invasive.** A resolver has to sit under the `windows-sys` calls -- `SubmitIoRing`,
   `PopIoRingCompletion`, the `Build*` family -- which is substantially more than `M24.2`'s field
   split, on a published crate.
3. **Kernel tests do not go away; their job changes and improves.** They stop being "run everything
   against Windows" and become "confirm reality stays *inside* the declared space". Better defined,
   and if Windows ever moves outside it, that test is what reports something real.

## What it would have caught, stated as reasoning rather than measurement

The FIFO consumer in case 5 is structurally the same defect as `D-47` -- a consumer assuming an
ordering the platform does not guarantee -- so that class is covered. `M21.6`'s `ERROR_TIMEOUT` defect
is covered provided the space includes "a wait may expire and report it as a failure HRESULT".

**Both are analogies from the demonstration, not separate measurements.** `M26` carries a calibration
item to re-inject both defects against a real resolver, because `D-41`'s corollary is the most
transferable rule this crate has produced: *a green result from an instrument nobody has shown can go
red is not evidence.* This session produced two apparatus failures of exactly that kind -- a first
draft that ran each spike condition once, and a case-4 harness whose bare flush completed inline on
both handles and so could not discriminate until it was rebuilt around aligned writes.

## Consequences for the plan

- `M24.1` is **answered**: the fake was the wrong instrument. Recorded, and the decision amended.
- `M24.4` is **withdrawn**. A shared conformance suite over a hand-written fake is superseded by the
  resolver, and its stated purpose -- hermetic bookkeeping tests -- is served better by one.
- `M24` **no longer depends on either**. Its goal is a hermetic lib suite, and relocation plus the
  accounting extraction achieve that on their own. It is now unconditional.
- `M26` is new, and is justified by **what it catches** rather than by hermeticity. That is a real
  distinction: hermeticity is achievable without it, so the resolver has to earn its place on the
  defect class it detects.
