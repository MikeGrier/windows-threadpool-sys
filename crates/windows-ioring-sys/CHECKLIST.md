# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
Everything through M18 is archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md): M1-M6
[here](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation),
M7 [here](COMPLETED-CHECKLIST.md#moved-2026-08-23----m7-ring-copy-a-topology-aligned-sample), M11-M14 in their
own dated groups, M8-M10
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m8-through-m10-handle-lifetime-cross-ring-identity-and-the-contract-audit),
and M15-M18
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m15-through-m18-the-testing-strategy-response-to-eight-defects).

M19 is archived [here](COMPLETED-CHECKLIST.md#m19). `M20` through `M24`, and the `M21+`, `M22+` and
`M28+` buckets, are archived
[here](COMPLETED-CHECKLIST.md#moved-2026-10-01-m20-through-m24).

**An `M{n}+` heading is parked rather than pending** -- see the `M{n}+` convention: it is gated work
with no current obligation, not an unfinished milestone. Which milestones are open is what the
headings below say; this preamble deliberately does not restate it, because a second copy is one
nobody updates.

## M26+ -- The wakeup window review opened

- [x] **M26.12** -- Not a race: the setup signal was owed only to the call that attached the event, so a caller that attached earlier got no wakeup at all. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2612)

- [x] **M26.13** -- Transferred to `windows-threadpool-sys`: the fault is pool-wide, not this
  crate's. The question, the timeline and the 33 measurement captures now live there as `M-T7.1`.
  -> [windows-threadpool-sys/CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md)

- [x] **M26.14** -- A self-removing `TempPath` guard, shared by 14 test files: an all-passing suite
  run went from 25 leaked files to 0, and a test panicking with its handle open now leaves none.
  The item's "none clean up" was wrong; 12 of 15 did, just not on the panicking path. ->
  [completed 2026-10-01](COMPLETED-CHECKLIST.md#m2614)

## M27 -- What this crate owes the topology planner

**Re-planned 2026-09-23, the same day it was written.** M27 was originally "Adaptivity: the benefit
without the architectural commitment", and asked whether *this crate* should derive a partition for a
consumer who expresses no preference. That was the wrong owner, and the checklist rules require
saying so rather than quietly rewriting it. The adaptivity the
[adoption thesis](../../DESIGN-NOTES.md#the-adoption-thesis) asks for is delivered by
[topology-planner](../topology-planner/COMPONENT.md), which takes a dataflow description of the
application and returns one or more suggested realizations
([EP-D-6](../topology-planner/DESIGN-NOTES.md#ep-d-6)). Had the original M27.1 been answered here it
would have grown a second, weaker policy surface beside the one that component exists to provide --
the `outermost_partitioning_cache` defect again, where a policy answer lands in a crate whose job is
something else.

> **-> CROSS-COMPONENT PREREQUISITE:** `M27.1` and `M27.2` are gated on component
> `crates/topology-planner` -> `M1+` -> `EP-1+.5` and `EP-1+.6`, which decide the plan vocabulary
> this crate would be realized from. See [CHECKLIST.md](../topology-planner/CHECKLIST.md).

**What survives here is the realization end, not the policy end.** The planner emits a plan; the
outward adapter realizes it as buffers, rings and threads
([EP-D-5](../topology-planner/DESIGN-NOTES.md#ep-d-5)). That adapter is a separate crate, but it can
only build what this crate exposes, and nothing has ever checked that what it exposes is sufficient.
[D-8](DESIGN-NOTES.md#d-8) is untouched by all of this: policy stays out of this crate, and being
*constructible from* a policy decision made elsewhere is the opposite of taking one.

- [ ] **M27.1** -- **Census what a realizer would need from this crate, against the plan vocabulary,
  and name what is missing.** A plan states which processor a domain pins to, which memory node its
  pool allocates from, how many queues of which types, and where each channel's buffer lives. Walk
  each of those to the public API that would realize it and record the gaps. `NumaBuffer`
  ([D-51](DESIGN-NOTES.md#d-51)) is one half of the pool answer and arrived this month; the ring's
  own construction takes no placement input at all. **The output is a gap list, not an API** --
  proposing surface before the plan vocabulary is settled would be binding to a draft.

- [ ] **M27.2** -- **Gated on `M27.1` and on the planner's `EP-1+.6`.** Close the gaps the census
  names, as ordinary capability on this crate with no policy attached. Each gap is an input a caller
  supplies, never a choice this crate makes. Verify the way the thesis demands rather than the
  convenient way: construct from a plan built against a *synthetic* machine, since the planner is
  mockable by construction and this crate should be realizable without the hardware the plan
  describes.

- [ ] **M27.3** -- Give a consumer the means to answer placement questions on their own hardware.
  **Not gated on the planner** -- it is the client-side half of the thesis, and it is what lets a
  developer disagree with any plan they are handed. `cache_domains.rs` now prints every cache level
  beside the heuristic's pick; the equivalent for placement is a sample that reports what a chosen
  arrangement costs and what the alternatives would have cost, on the machine in hand.
  [ring_copy](examples/ring_copy) is the natural host, being already policy-selectable. **Do not ship
  a verdict** -- report the observation and let the consumer conclude, per OPTION INTEGRITY.

## M28 -- The ring owns the pending inventory

Queued by [D-55](DESIGN-NOTES.md#d-55), taken 2026-09-23 after the `M23.3` exploration. The break
is accepted deliberately: `IoRing` becomes generic so the inventory cannot drift from the ring,
because a consumer never holds a token to lose. The exploration and everything it falsified is in
[DESIGN-SESSION-2026-09-23-pending-inventory.md](design-sessions/DESIGN-SESSION-2026-09-23-pending-inventory.md);
`src/pending.rs` is the working spike and is the shape the internal map starts from.

**Sequenced so each step compiles.** The published crate is at 0.3.1, so this is a major bump and
every consumer names the type -- which means the migration order matters more than usual.

- [x] **M28.1** -- Decided: a push returns a `Copy` identity that owns nothing, and the ring returns the payload at its own pop -- `Token` is split, not moved. Recorded as [D-71](DESIGN-NOTES.md#d-71). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m281)

- [x] **M28.2** -- `RingContract` is bounded by operations in flight: terminal entries are retired, and a capped history keeps a duplicate distinguishable from an unrecognised completion. Recorded as [D-72](DESIGN-NOTES.md#d-72). -> [completed 2026-09-25](COMPLETED-CHECKLIST.md#m282)

- [ ] **M28.3+M28.4** -- **Make `IoRing` generic, move the inventory inside, and migrate every
  consumer, as one commit.** Gated on [D-71](DESIGN-NOTES.md#d-71), which settled what a caller
  receives.

  **Merged deliberately, and the coupling is acknowledged rather than disguised.** The milestone
  header requires each step to compile, `M28.4` requires converting all consumers or none, and
  `M28.3` changes `IoRing`'s shape -- so the three cannot all hold with the items separate. The
  alternative considered and rejected was landing `M28.3` additively, with an `OperationId` path
  beside the existing `Token` one: it compiles at every step, but it leaves two token models live
  at once and the old one still lets a consumer lose a token, which is the defect `D-55` exists to
  remove. Never having both is worth one large commit.

  **Carry the sidecar.** The census found two thirds of consumers keep per-operation data beside
  the token, so an inventory holding only tokens serves a minority. Mixed-shape consumers use a
  closed `enum`; [generated_sequences.rs](tests/generated_sequences.rs) is the worked example.

  **Soundness already settled:** [`IoBuf`](src/buf.rs) is an unsafe trait whose contract requires
  the address to survive a move, so the ring may hold buffers in a map.

  Sequenced so the work is resumable, since it does not compile in the middle:

  - [x] **M28.3.1** -- `OperationId`: `Copy`, no `Drop`, a name and not a capability (`D-71`).
  - [x] **M28.3.2** -- `IoRing<T = ()>` carrying `inventory: HashMap<usize, T>`, and
        `Batch<'ring, T>` with it. A default keeps a payload-free consumer from naming `()`.
  - [x] **M28.3.3** -- Push stores the payload and returns an `OperationId`; pop returns the
        payload with the completion. Shape settled by [D-73](DESIGN-NOTES.md#d-73):
        `IoRing<T, X = ()>`, with the file guard held in a concrete internal `Held` rather than
        made generic, which is sound because `FileTarget` is sealed.

        **Do the `Drop` half in this step, not later.** Moving buffers into the ring removes the
        protection `Token`'s leak-on-unclaimed-drop provides on the path where `run_down` fails --
        which `Drop for IoRing` takes best-effort, closing anyway. The inventory must be
        *forgotten* rather than dropped there, or the close frees memory the kernel may still be
        writing into. Landing the inventory without this is a use-after-free, so it is one step.

        The tokenless shape is `M28.5`'s and is only accommodated here, not answered.
  - [x] **M28.3.4** -- Carry the parameter through `EventDelivery`, `RingScope` and the contract
        wiring.

        **The tree stops compiling here, as this plan said it would.** The delivery callback is
        now `Fn(Completion, Option<(T, X)>)`, so ten call sites across
        [event_delivery.rs](tests/event_delivery.rs), [handover.rs](tests/handover.rs),
        [checkpoint.rs](examples/epoch_log/checkpoint.rs) and
        [model_a_delivery.rs](examples/model_a_delivery.rs) take a one-argument closure and no
        longer build. The library and its own unit tests are green; the integration and example
        targets are `M28.4.1`'s to migrate. Contract wiring is untouched deliberately -- what
        becomes of the oracle's leak rules is a decision `M28.4.1` carries, per
        [D-73](DESIGN-NOTES.md#d-73).
  - [x] **M28.4.1a** -- Restore the tree: every delivery callback takes the payload, so the
        build is green again on every feature set.

        **Two call sites were invisible to an ordinary sweep**, and both are worth remembering
        rather than rediscovering. One lives in a **doctest**, found only because this repository
        compiles its prose. The other is in [failure_paths.rs](tests/failure_paths.rs), which
        compiles only under `--all-features`, so a default-feature check could not see it. A
        migration sweep here has to run `--all-features` **and** `--doc` before it means anything.

  - [x] **M28.4.1b** -- All ten push shapes have an inventory form: `read_raw_owned`,
        `write_raw_owned`, `read_owned`, `write_owned`, `flush_owned`, `cancel_owned`, and the
        four registered variants. `Held` is populated by the guarded pushes and
        `Held.registration` by the registered ones, so both halves are exercised and neither
        needs a dead-code marker any longer. `read_owned` is the only
        entry point that stows, so "migrate the consumers" has no destination for the other ten
        shapes yet -- writes, flushes, cancels, and the registered variants. Give each an
        inventory form first, populating `Held` for the guarded ones, which is what retires the
        `#[expect(dead_code)]` on `Held` and `FileGuard`.

  - [x] **M28.4.1c** -- Decided in [D-74](DESIGN-NOTES.md#d-74): the leak rules **retire** rather
        than narrow, because a push that hands back only an `OperationId` leaves nothing to drop
        unclaimed. `State::Pushed` and `State::PushedTokenless` collapse with them. The
        conservation they approximated becomes `held() == outstanding()`, which the ring can check
        about itself. `RingContract` keeps the four claims that are about the kernel rather than
        about a caller's bookkeeping.

  - [ ] **M28.4.1d** -- Migrate every consumer onto the inventory, then retire the token API.

        **Measured before planning, and it is larger than "36 files" suggested**: 172 push call
        sites and **116 `claim_if` sites** across 35 files. `claim_if` is not a substitution --
        it is how each test *drives* its ring, so converting restructures control flow rather
        than replacing a call.

        **What a conversion actually does, which is why it is worth it.** The caller's
        `HashMap<usize, (sidecar, Token<..>)>` *disappears* at each site: the push carries the
        sidecar as `X`, and the pop returns `(payload, sidecar)` together. That is `D-55` paying
        off rather than a cost being paid.

        **Batched, and the reason that is legitimate.** Both APIs coexist today, so a
        partly-converted tree still compiles and every batch is a green commit. `M28.4.2`'s
        "convert all of them or none" governs the **shipped** state -- never two token models in
        a release -- not the path to it. The final batch is what makes that true, and nothing is
        released in between.

  - [x] **M28.4.1d.1** -- [bounded_pop.rs](tests/bounded_pop.rs) converted as the worked
        pattern. The `Token<Vec<u8>>` threaded through `push_pending_read`, `settle` and six call
        sites is gone; `PipeRing = IoRing<Vec<u8>>` holds the buffer instead. The conversion
        found a real defect in rundown, which is recorded as `M28.4.1d.1b`.

  - [x] **M28.4.1d.1b** -- Decided: there should be one pop, not two. **Decide what `try_pop`
        and `pop_within` mean on a ring that holds payloads.** Found by converting the first file: `drain_for_rundown` popped with `try_pop`
        and never reclaimed, so rundown stranded every entry it reaped. Fixed there -- rundown is
        teardown, so dropping is right, and the completion is the proof that makes freeing safe.

        **The same gap is open on the public paths, and the answer is that the split should not
        exist.** An earlier note here offered three ways to manage a permanent distinction
        between `try_pop` and `try_pop_held`. That was wrong, and the symmetry is the argument:
        `try_pop_held` sits beside `try_pop` for exactly the reason `read_raw_owned` sits beside
        `read_raw` -- the inventory was added *beside* the token API rather than replacing it.
        Both are the same transitional duplication, and the push side is already settled as
        ending with one family.

        **Nothing needs a non-reclaiming pop.** Measured: the only internal callers are
        `drain_for_rundown` (now reclaims), `pop_within_with`, and one other -- and once the
        token pushes are gone, *every* operation has an inventory entry, including the tokenless
        ones, which stow `payload: None`. There is no operation a pop could legitimately find
        nothing for.

        So `try_pop` becomes the reclaiming pop, `try_pop_held` disappears as a name, and
        `pop_within` returns the same shape. Preserving the distinction would be a rule against
        an impossible act, which [D-74](DESIGN-NOTES.md#d-74) already names as worse than no rule
        at all.

        **This does not gate the conversion.** Consumers migrate onto the reclaiming pop either
        way; the rename lands in `M28.4.1d.3` beside the push retirement.

  - [x] **M28.4.1d.2** -- Every consumer that can be converted before the token API is retired now is; three plus `append.rs` are blocked on `M28.4.1d.3`. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2)

  - [x] **M28.4.1d.2b** -- Documented the two ways round a mixed payload on `IoRing::with_inventory` and in `D-73`; no `IoBuf` enum helper built. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d2b)

  - [x] **M28.4.1d.3** -- Token API retired, `D-74` applied, one reclaiming pop per shape; the break is closed. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2841d3)

  - [x] **M28.4.2** -- Five inventory sabotages added (push, both pops, both appender halves); the sweep also found `d.3` had broken the manifest. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m2842)

- [x] **M28.5** -- `observe_tokenless_push` retired; the outer `None` has two causes the caller distinguishes, recorded as `D-75` and asserted both ways. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m285)

- [x] **M28.6** -- Swept what the break made false: `D-4` and `D-55` amended, six live example claims corrected, and three defects found in `d.3`'s own prose sweep. -> [completed 2026-09-26](COMPLETED-CHECKLIST.md#m286)
