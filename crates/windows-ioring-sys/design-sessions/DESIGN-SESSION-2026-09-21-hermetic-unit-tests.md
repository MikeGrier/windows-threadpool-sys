# Design session 2026-09-21: hermetic unit tests without losing unit-level coverage

**Status: concluded 2026-09-21.** The engineer's verdict was that the current design is incorrect
and the question is only when to fix it, so the defect and its classification are recorded as
[D-49](../DESIGN-NOTES.md#d-49) and the work is scheduled as `M24` in [CHECKLIST.md](../CHECKLIST.md).
The *remedy* is still open, gated on `M24.1`. What follows is the record as written before that
verdict; the proposal below is the input to `M24.1`, not its answer. This records a question, the
measurements taken to answer it, and a proposal, so that a decision can be made against evidence
rather than against recollection. If the proposal is adopted it becomes a decision in
[DESIGN-NOTES.md](../DESIGN-NOTES.md) and checklist items in [CHECKLIST.md](../CHECKLIST.md), in the
same change.

## The question

Raised by the engineer after the M21 work, in two parts:

> A unit test that is affected by system load is not a unit test. [...] Unit tests should be
> hermetic. The fact that this failure was discovered due to system load is prima facie evidence
> that the tests are not hermetic.

and then, on being told the structural fix would move those tests to `tests/`:

> While we structurally *can* [move] those tests to be integration tests, that means we lose their
> coverage at the unit level which is not something that I want to give up lightly. I would prefer to
> be able to either have the same test code duplicated or be effectively a library and be able to
> apply to the hermetic IoRing as well as the actual one.

## What is already settled, and what was got wrong

**The classification is not in doubt.** The repository's own Quality rule reserves integration tests
for cases that "must cross a real process, filesystem, network, device, **operating-system API**, or
other external boundary". `CreateIoRing` is an operating-system API. Tests that open a real ring are
integration tests, wherever they currently live.

**What M21.6 fixed was not hermeticity.** Removing five wall-clock assertions from four unit tests
made their *outcome* independent of load -- measured, under 2x CPU saturation, at 30x duration
variation with zero outcome variation. It did not make them hermetic: they still open a real kernel
ring. Outcome-stable and hermetic are different properties, and conflating them is what let the
original answer sound complete when it was not.

One correction to the evidence, which cuts in the engineer's favour rather than against: the
load-affected failure actually observed was in `tests/bounded_pop.rs`, which *is* an integration
test and is where it belongs. The unit suite's hermeticity problem was real but separate -- it was
the five clock assertions, which failed nothing and were removed on principle.

## Measurements

Counted by command, not by recollection.

**How much of the unit suite crosses the boundary:**

| Module | Tests | Open a real ring |
|---|---|---|
| `buf`, `capability`, `contract`, `error` | 60 | 0 |
| `batch` | 18 | 13 |
| `ring` | 40 | 37 |
| `event_delivery` | 6 | 6 |
| `token` | 7 | 7 |
| **total** | **131** | **63** |

Four modules are already perfectly hermetic. The non-hermetic mass is concentrated in four others.

**How movable those 63 are, as they stand:**

- **25** use only public API. A pure relocation to `tests/`.
- **38** also reach crate-private items -- `reserve_user_data`, `record_completion`,
  `cancel_reservation`, `Completion::synthetic`, `set_supported_ops_for_test`, `raw_handle`,
  `ring_id`. An integration test cannot see any of those, and widening them would be the wrong
  trade: several exist specifically in order *not* to be public.

**What those 38 are actually testing:** identity minting, outstanding accounting, completion
matching, capability gating. That is bookkeeping, not kernel behaviour. They open a ring only
because the bookkeeping lives as fields on a struct that also owns a handle.

**Whether that bookkeeping separates.** Both questions flagged as unknowns were investigated and
both came back clean:

- `RingId::next()` is a process-global `AtomicU64` and never touches a handle. Identity minting is
  trivially handle-independent.
- `IoRing`'s ten fields split 5/5. Kernel-coupled: `handle`, `completion_event`,
  `registered_buffer_infos`, plus `version` and `supported_ops` which are *negotiated* from the
  kernel and then are pure data (and already have a test seam, `set_supported_ops_for_test`). Pure
  bookkeeping, no handle: `ring_id`, `next_user_data`, `outstanding`, `registered_files`,
  `registered_buffers`.

## The constraint this must not break

[DESIGN-NOTES.md](../DESIGN-NOTES.md) already rejects a mock `IoRing`, in
"Two techniques deliberately rejected", and the argument is a good one:

> Both shipped defects were the kernel behaving differently from this crate's assumptions. A mock
> *encodes* the assumption, so one written before those discoveries would have passed both bugs
> green -- it would not merely have failed to find them, it would have manufactured evidence they
> were absent.

**This pass supplied three more confirmations of exactly that**, all within a few hours:

| What the kernel actually does | What a mock would have been written to do |
|---|---|
| `SubmitIoRing` reports an expired wait as `ERROR_TIMEOUT`, a *failure* HRESULT | return success, because a timeout is not an error |
| `SubmitIoRing` answers `E_INVALIDARG` when asked to wait with nothing pending | time out, or succeed |
| A handle without `FILE_FLAG_OVERLAPPED` completes **inline during submit** | leave the operation pending |

The first is the High-severity defect an independent review found in `M21.2`. A mock would have kept
the suite green while every real timeout returned `Err`.

So any proposal here has to survive that objection rather than ignore it.

## The proposal: one suite, two backends, shared assertions

Write the bookkeeping tests **once**, as generic functions over a small trait, and run them twice:
against a hermetic in-memory implementation and against the real kernel ring.

```text
                    +-- run by src/ unit tests, with FakeRing ..... hermetic
shared suite -------+
   (generic)        +-- run by tests/ integration, with IoRing .... crosses the boundary
```

**Why this is not the rejected mock.** The rejected thing is a mock used *instead of* the kernel,
where nothing ever checks the model against reality. Here the same assertions run against both, so
the fake is continuously differentially tested against the kernel: the moment the model diverges,
one side goes red and names the divergence. That is the same discipline this crate already demands
of its spikes -- "a spike must carry a **control case**, because the first two drain-ordering spikes
could not discriminate and would have returned confidently wrong answers". The kernel run *is* the
control.

The existing decision would therefore need **amending, not overriding**: it rejects a mock as a
substitute, and this is a mock as a co-tested peer. That distinction is the whole proposal, and if it
does not hold up the proposal fails with it.

### The bright line: what the fake is never allowed to answer

The fake models *this crate's bookkeeping*. It never gets a vote on Windows. Concretely, none of
these may be asserted against the fake, because the fake would only be agreeing with whoever wrote
it:

- the completion event being edge-triggered ([D-19](../DESIGN-NOTES.md#d-19));
- one waiter per ring ([D-21](../DESIGN-NOTES.md#d-21));
- flush coverage and drain ordering ([D-23](../DESIGN-NOTES.md#d-23),
  [D-24](../DESIGN-NOTES.md#d-24), [D-47](../DESIGN-NOTES.md#d-47));
- the registration array being read when the operation *runs* ([D-32](../DESIGN-NOTES.md#d-32));
- `ERROR_TIMEOUT` and `E_INVALIDARG` from `SubmitIoRing`;
- inline completion on a synchronous handle.

Those stay kernel-only, forever. They are also precisely the findings this pass produced, which is
the argument for the line being drawn exactly here.

### What the shared suite would cover

Everything whose truth is decided by this crate rather than by Windows:

- identity minting: monotonic, never repeated, exhaustion refused rather than wrapped;
- outstanding accounting: minted, completed, cancelled, saturating rather than underflowing;
- token claim matching on `user_data` *and* `ring_id`, including cross-ring rejection;
- registration index arithmetic (the base index of a second registration);
- capability gating -- a push refused because the op is unsupported (`set_supported_ops_for_test`
  already exists for this and needs no kernel);
- `pop_within`'s deadline arithmetic and its nothing-can-arrive early return, which with a fake ring
  *and* a fake `CompletionWait` becomes fully hermetic;
- `RingContract`'s oracle over observed sequences, which is already hermetic and would simply join
  the suite.

### Mechanics, with costs

Three shapes, cheapest first.

**1. Generic test functions over a narrow trait.** A trait describing only what the bookkeeping
tests need -- mint an identity, observe a completion, read `outstanding`, claim a token. The real
implementation wraps `IoRing`; the fake is a few hundred lines of plain Rust. Test bodies are
`fn identity_is_never_reused<R: RingUnderTest>(ring: &mut R)`.

*Cost:* the suite must be reachable from both `src/` and `tests/`, and `tests/` cannot see
crate-private items. The established answer in this repository is a feature-gated `pub` module --
`windows-file-watcher` already ships `test-util` and `scenario-tool` features for exactly this. So:
a `pub mod conformance` behind a non-default `test-util` feature.

*Consequence to accept:* feature-gated code is invisible to a default `cargo test` and to
`cargo mutants` without `--all-features`, which this repository has already been bitten by (a
`windows-file-watcher` sweep reported 247 survivors of which 147 were in gated modules). CI would
need the suite in its `--all-features` job, and mutation runs would need the flag.

**2. Parameterise `IoRing` over a backend.** `IoRing<B = Win32Backend>` with a private `RingOps`
trait over the ~10 Win32 entry points. The public spelling `IoRing` survives via the default type
parameter.

*Cost:* `Batch<'_>` becomes `Batch<'_, B>`, and every `impl` block gains a parameter. Mechanical but
it touches the whole crate, and it puts a type parameter into a published API for a testing reason.
Higher risk, more coverage: it would let the *submission* paths be exercised hermetically too, not
just the bookkeeping.

**3. Extract the bookkeeping into a handle-free type.** `RingAccounting` holding the five pure
fields, composed by `IoRing`. Most of the 38 internals-touching tests become hermetic *in place*,
with no fake, no feature gate, and no widened visibility.

*Cost:* a real refactor of a shipped crate's internals, though not of its public surface. It is the
smallest conceptual change and the one that most directly matches the observation that those 38
tests were never about the kernel.

These are not exclusive. **3 then 1** is the combination worth considering: extract the accounting so
much of the coverage becomes hermetic without any fake at all, then add the shared suite for what
remains and genuinely benefits from running against both backends.

## Open questions for the engineer

1. **Does the co-tested-peer argument actually survive?** It is the load-bearing claim. If a fake can
   drift in a way the shared assertions do not catch -- because the assertion is about our
   bookkeeping and the drift is in our model of Windows -- then the rejection stands and only option
   3 is safe.
2. **Is a type parameter in the published API acceptable for a testing reason?** That is option 2's
   real cost, and it is a question about the crate's public shape rather than about tests.
3. **Is the feature-gate consequence acceptable?** Gated tests do not run by default, and this
   repository has measured what that does to a mutation sweep.
4. **What is the target?** "`cargo test --lib` is hermetic and means it" is achievable. "Every
   behaviour has a hermetic test" is not, and should not be -- the six items on the bright line above
   must stay kernel-only.

## What was deliberately not queued, and what changed

As first written this recorded no decision and queued no work, on the grounds that the proposal
changes a decision that is currently written down and well argued, and that the evidence for
amending it -- the co-tested-peer distinction -- is an argument rather than a measurement.

The engineer settled the part that did not depend on that argument: **the current structure is
incorrect regardless of which remedy is chosen**, so the defect is now [D-49](../DESIGN-NOTES.md#d-49)
and the work is `M24`. The co-tested-peer question survives untouched as `M24.1`, which is
required to settle it **by demonstration rather than by argument** -- precisely because an argument
is what is in doubt.
