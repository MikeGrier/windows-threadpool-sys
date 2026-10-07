# Completed checklist: durable-ioring

Append-only archive of completed items from [CHECKLIST.md](CHECKLIST.md).

## Moved 2026-10-05 10:44:19 -04:00 -- the crate's name

### <a id="di-13"></a>DI-1.3 -- The name stays `durable-ioring`: the `win-` rule's scope was clarified to direct layers. *(completed 2026-10-05 10:44:19 -04:00)*

The conflict was raised while giving the crate a home: the workspace decision on the `win-` prefix
read as covering every new crate, and predated the engineer's choice of `durable-ioring`. The
engineer clarified that the rule covers crates layering safe abstractions more or less directly over
Windows APIs, and that this crate, a new facility, does not fit it. The authoritative text is the
amended decision in the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix)
and [DI-D-2](DESIGN-NOTES.md#di-d-2); the reasoning is in
[DESIGN-RATIONALE.md](../../DESIGN-RATIONALE.md#why-the-win-prefix-is-scoped-to-direct-layers).

## Moved 2026-10-05 11:42:07 -04:00 -- the ordering of durability points

### <a id="di-11"></a>DI-1.1 -- Points are ordered by the consumer's own tag, sealed as a prefix by `make_durable_through(n)`. *(completed 2026-10-05 11:42:07 -04:00)*

Resolved by adopting [DI-D-9](DESIGN-NOTES.md#di-d-9), which is the authoritative statement. The
question began as "are opaque points ordered?", narrowed to points whose lifetimes overlap, defined
"before" over the instance's own observation order, settled on seal order rather than open or
resolution order, and was then dissolved by the engineer's tags model: because
`make_durable_through(n)` seals a prefix of the consumer's number space, seal order is numeric order
by construction. The full path, including the explicit-handle model it superseded, is in
[DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).
Two parts of the discussion live on elsewhere: the observation-order definition and the corrected
suspect-window bound are in DI-1.4.

## Moved 2026-10-05 18:10:27 -04:00 -- the failure model and its resolution

### <a id="di-12"></a>DI-1.2 -- Durability failures are resolved by the consumer, one failure at a time or several atomically. *(completed 2026-10-05 18:10:27 -04:00)*

The authoritative statements are now [DI-D-12](DESIGN-NOTES.md#di-d-12) (failures and their
resolution) and [DI-D-13](DESIGN-NOTES.md#di-d-13) (the completion queue and its events). The item's
full text at completion is kept below because much of its reasoning -- the ten questions, the
engineer's corrections, and the alternatives each settled against -- is recorded nowhere else.

- [x] **DI-1.2** -- **Can resolving a failure be put in the consumer's control, and how?** The
  engineer's question.

  **Agreed in principle** (the engineer, on the healing trace in the session: "this seems
  reasonable"): yes, with one rule kept here as mechanism -- **a resolution takes effect at a
  durable point sealed after the consumer declares it**. In [DI-D-9](DESIGN-NOTES.md#di-d-9)'s terms:
  a rewrite of a failed epoch's data must carry a higher tag (rule (a) forces it), and the failed
  epoch resolves when an epoch sealed after the declaration is durable -- so whatever was re-issued
  is covered by a later successful flush, and this crate never has to judge whether the repair was
  sufficient.

  **Open questions, each with the assistant's recommended answer; `[x]` marks one the engineer has
  settled:**

  1. [x] **Operations gated on an abandoned epoch** -- **agreed, in the engineer's framing: they
     fail**, because the condition they wait on has been declared permanently false. Each completes
     as a failure in the ordinary way, its buffer coming back with the completion, and the error
     says the operation was *never issued* -- so the consumer knows no bytes were written, unlike an
     I/O failure, which may have partial effects. (The earlier wording, "refused and returned to the
     consumer", was confusing and is retired.) *Context:* this concerns release-after-durable operations dioring is holding
     until an epoch is durable, not the failed epoch's own writes (those were already issued; the
     consumer heals by issuing new writes under a higher tag and declaring the heal, and how those
     relate to the old writes is none of dioring's business). If the epoch heals, its gated
     operations are released when it becomes durable. If it is abandoned, it never will be, so they
     complete with a refusal and their buffers come back. **This also constrains Q5:** a gate must
     wait for *durable*, never merely *resolved*, or abandoning the log epoch would release the data
     write that depended on it.

  2. [x] **How resolutions appear in the completion stream** -- **approved by the engineer**:
     dioring presents a completion queue holding every completion a consumer should observe; the
     four events stand, `Blocked` included, **on the condition that synthesizing it is cheap**, so an
     uninterested consumer can simply ignore it; and `Durable`'s ordering promise is the right one.
     No filtering is needed, since the only synthesized event beyond the four outcomes arises only in
     durability-failure cases -- to be revisited if a noisy class of synthesized entries ever
     appears. Detail as proposed: meaning *dioring's own* completion
     queue, the one the consumer pops, not the kernel ring's (which only dioring touches, per
     [DI-D-10](DESIGN-NOTES.md#di-d-10)). It carries the consumer's operation completions plus
     durability events dioring synthesizes, which correspond to no single kernel completion; and
     it can promise that `Durable { through: n }` is delivered after the completions of every write
     tagged at or below n. **`Blocked`, explained on the engineer's question:** emitted when a
     `make_durable_through(n)` has done all its physical work -- its flushes succeeded -- but `n`
     cannot be reported durable because an unresolved failure sits at or below it (the prefix
     property). It names that failure, and is not terminal: `Durable { through: n }` follows once
     the failure is resolved. Its purpose is that every `make_durable_through` gets a prompt answer
     -- `Durable`, `Failed`, or `Blocked` -- rather than a silent stall; it adds no information the
     earlier `Failed` event did not, only the link between this request and that failure, so it
     could be dropped if a minimal event set is preferred. The event shapes `Failed`, `Blocked`,
     `Durable` and `Abandoned` are named here and given types in DI-2.1.

  3. [x] **A failure the consumer never resolves** (approved by the engineer, both points) -- the watermark stays stalled and the inventory
     stays held; no timeout is invented here
     ([D-67](../windows-ioring-sys/DESIGN-NOTES.md#d-67)), and the contract says the bound is the
     consumer's.

  4. [x] **Importing an external failure** -- **kept in v1 by the engineer, on one condition**:
     the state graph stays reducible. An import must be the same "failure observed" transition a
     real flush failure takes, with the cause as data, adding no state and no edge of its own --
     the engineer's concern was a new edge multiplying the state space. Carried into DI-3+.2 as an
     implementation constraint. Original question: ("treat everything pushed before now as suspect"), so a
     multi-writer consumer can propagate a failure another writer sharing the flush regime
     observed -- in v1. *Explained on the engineer's request:* it creates a `DurabilityFailure`
     with cause "imported", whose suspect set is every write pushed before the import that no
     successful covering flush yet covers -- the same rule as any failure, since the consumer learned
     of the outside failure after it happened. It is then resolved like any other failure. Two
     limits: it is an assertion dioring cannot check (wrong imports cost only a needless rewrite),
     and **it cannot retract a `Durable` already reported** -- an outside failure the consumer
     learns of late may already have falsified one, and finality plus "truthful relative to what the
     instance was told" means dioring does not revisit it. An import is only as good as how promptly
     the consumer learns. *In the engineer's two-way framing:* the import answers "writes not yet made
     durable may already have been lost", and never "writes reported durable were not" -- the
     consumer supplies only the *fact* of an outside failure, and dioring supplies *which* of its
     writes that puts at risk. **The engineer's concern: testability.** It may be hard to test and
     may keep poking holes in the ordering logic by generating faults in whatever order is claimed,
     so it could be deferred past v1. The assistant's answer: (i) an import cannot claim an order --
     it can only say "now", in the instance's own observation order, which is exactly where a real
     flush failure is observed (at a drain, interleaved arbitrarily with the consumer's calls); so it
     adds no ordering that real failures do not already produce; (ii) the fault-injecting
     implementation (DI-3+.3) needs a "fail now" operation anyway, and dioring's own failure tests
     will be built on it, so the import is that operation made public and is tested by the same
     tests; (iii) whatever is decided, the core must treat "a failure is observed" as one
     transition that can occur at any drain point, because real failures require it -- deferring
     the public call costs little *only* if that holds. Not decided; the engineer's call between
     including it in v1 and deferring the public call while keeping the core shaped for it. The
     system-event watcher that would feed it is separate and is not proposed for v1.

  5. [x] **What the watermark means once an epoch is abandoned** -- **settled by the engineer**:
     once the consumer has closed out the faulted epoch, it is business as usual, so the watermark
     moves past it. Consequences recorded: (a) `durable_through(m)` means "every epoch at or below m is
     durable or was abandoned", which the contract must say in those words; (b) a request left
     `Blocked` behind the failure is answered with `Durable` as soon as the abandonment lands; (c) per
     Q1, operations gated on the abandoned epoch still fail -- gates wait for *durable*, never for
     *resolved*. *Terminology:* failures are **resolved** (healed or abandoned); "seal" means
     only `make_durable_through`'s action. Original wording: -- it may pass the abandoned epoch, so
     `durable_through(m)` means "every epoch at or below m is durable *or was abandoned by the
     consumer*", with the `Abandoned` event as the record. Changes the headline guarantee.

  6. [x] **The unit of resolution** -- **settled by the engineer: the failure.** v1 resolves whole
     failures (`heal(failure)` / `abandon(failure)`); the prefix form below remains a recorded,
     non-foreclosed extension, and per-I/O resolution belongs to the retention layer (DI-5+.1).
     Original question: the failure (one event, its suspect set spanning several tags,
     resolved once), not individual epochs. **The engineer's question: "why not both?"** The
     assistant's analysis, not decided:
     - *What per-epoch resolution buys:* the fate of gated operations per epoch. Under failure-only
       resolution, a consumer that heals epoch 41 but must abandon 42 has to abandon the whole
       failure, so operations gated on the healed 41 fail too and must be reissued against the
       healing epoch. A precise record of which epochs were healed is the other gain.
     - *What it costs:* state per (failure, epoch) pair instead of per failure -- the multiplicative
       growth the engineer warned against under Q4 -- and ambiguity once Q7 lets two separate failures
       share an epoch: resolving "41" then needs to say for which failure.
     - *Neither is wrong analytically*, so OPTION INTEGRITY does not settle it; the trade is state
       space and test burden against a precision whose main effect has a workaround.
     - *Assistant's lean:* failure-level in v1, with per-epoch resolution addable later as an extra
       call -- provided the core keeps a failure's epochs as data it can refine, not a single
       opaque blob.
     - **Refined by the engineer's follow-up** ("if I heal 42, I believe I have implicitly healed
       41; if I abandon 42, implicitly abandoned 41"). A resolution's *effect* is downstream: it
       moves the watermark, decides the fate of gated operations, and leaves a record. The claim
       holds if resolution is **defined over a prefix**, like `make_durable_through`: resolving
       *through* n covers every still-unresolved epoch of the failure at or below n. Physically,
       healing 42 does not rewrite 41's data, so `heal ... through 42` is the consumer's assertion
       for the whole prefix -- the same kind of assertion every heal already is. Consequences:
       **any per-epoch mixture is expressible** as a sequence of prefix calls in increasing order
       (heal through 41, then abandon through 42), while the state stays **one number per failure**
       ("resolved through k") -- reducible, as Q4 required. **Scope it to one failure**, not to every
       failure at or below n: a cross-failure prefix could resolve a failure that arrived after the
       consumer's rewrite and that it has not yet seen. Proposed shape: resolve(failure, through n,
       heal or abandon). Not decided.
     - **The engineer's second follow-up:** per-buffer resolution matters in the retention layer,
       which can reissue what it holds, but at this layer it is unclear what dioring would *do*
       with knowledge of which I/Os were healed and which abandoned. The assistant's answer: at
       this layer, nothing per I/O. dioring uses the heal-or-abandon distinction for exactly three
       things -- when the watermark moves (a heal waits for its healing seal, an abandon is
       immediate), the fate of gated operations, and the contract's "durable or abandoned"
       meaning -- and only the gated operations ever differ below the failure level, with the
       reissue workaround available. **Revised recommendation:** resolve whole failures in v1
       (`heal(failure)` / `abandon(failure)`); the prefix form above stays a recorded,
       non-foreclosed extension; per-I/O resolution belongs to the retention layer (DI-5+.1).

  7. [x] **A second failure before the first is resolved** -- **settled by the engineer:** the
     failures stay separate, **but one call may resolve several at once** -- "I don't see why you
     can't supply resolutions to the same API call at the same time". The assistant's notes on the
     batch, proposed rather than decided: it changes no state model (each failure is still resolved
     individually, and mixing heal and abandon in one batch is fine); it should apply **atomically**,
     so all of its resolutions take effect at one point in the instance's observation order -- an
     epoch caught in two failures then never passes through a half-resolved state that would emit a
     transient `Blocked`; it is validated whole before anything changes, as
     [D-28](../windows-ioring-sys/DESIGN-NOTES.md#d-28) treats legality checks, so an unknown or
     foreign handle refuses the entire call; and because it names failures explicitly, it cannot
     resolve one the consumer has not seen. A single resolution is a batch of one. (Whether naming an
     already-resolved failure is refused or reports its state was left to DI-2.1; Q9's move-only token
     has since dissolved it, since a token cannot be presented twice.) Original wording: the failures stay
     separate, each resolved
     on its own, and the watermark passes only when every failure at or below it is resolved.
     **The engineer's question: are failures 1:1 with epochs, or 1:n?** The assistant's answer:
     *many-to-many*, inherently.
     - **One failure, many epochs:** a failure's suspect set is defined by push order, not by tag,
       so it holds writes of the sealed epoch whose flush failed, of sealed epochs still pending,
       and of epochs still open.
     - **One epoch, many failures:** an open epoch can have writes before failure F1 (in F1's set)
       and further writes before a later F2 (in F2's).
     - **Forcing 1:1 is the per-epoch granularity of Q6 again** -- splitting one physical failure
       into per-epoch pieces -- so it is not free.
     - **Consequence for an epoch in several failures:** it is past the watermark only when every
       failure containing it is resolved, and it counts as durable only if every one was healed; if
       any abandoned it, the epoch is abandoned, and its gated operations fail (Q1).
     - **Edge case:** a failure whose suspect set is empty (an import when everything was already
       covered) puts nothing at risk; whether it is created at all, or reported and resolved at
       once, is a detail for DI-2.1.

  8. [x] **What counts as a failure object** -- **settled by the engineer:** durability failures
     only, meaning a failed flush or an imported external failure (Q4). The suggested mitigation
     below is **withdrawn by the assistant as redundant**: Q2's ordering promise already delivers
     every write completion tagged at or below n, failures included, before `Durable { through: n }`,
     so a consumer that wants the count can take it from the stream. Original text -- only *durability*
     failures: a failed flush, or an imported external failure. A failed or short write is an
     ordinary operation error on its own completion, and "durable through n" covers the writes tagged
     at or below n *that completed successfully, up to the byte count each completion reported*; the
     contract must say so in those words. Suggested mitigation for naive consumers, not agreed: the
     `Durable` event carries a count of failed or short writes in its range.

  9. [x] **How a failure is named** -- **revised by the engineer: the handle that resolves a
     failure is linear** (later settled as **affine, with drop meaning `close()`** -- DI-2.7 point 7)
     -- dispensed once (with the `Failed` event), movable, and consumed by the
     resolution, so a failure cannot be resolved twice and the DI-2.1 question of naming an
     already-resolved failure disappears at compile time. The assistant's notes: (a) Rust's types are
     *affine*, not linear -- a value can be dropped unused -- so what dropping an unresolved token
     does is a real decision, queued as DI-2.7 point 7; (b) identity and capability split into two
     types: a `Copy` failure *identity* for `Blocked` events and for Q10's queries, and the move-only
     *token* that alone can resolve, so enumeration never mints a second token; (c) a batch that is
     refused (Q7) must hand its tokens back in the error, since it took them by value. Superseded
     proposal -- a unique, opaque, `Copy` handle that owns nothing and is never
     reused within an instance, the same shape as `windows-ioring-sys`' `OperationId`
     ([D-71](../windows-ioring-sys/DESIGN-NOTES.md#d-71)); `heal` and `abandon` take it, and another
     instance's handle is refused.

  10. [x] **Whether failures are visible after the event** -- **settled by the engineer:** there is
      an inventory of unresolved failures; there is **no** memory of a failure after it is resolved
      (the "a handle still answers resolved, and how" half is dropped). The assistant's notes:
      (a) no history is needed, because identities are never reused -- an identity once seen and
      no longer in the inventory unambiguously means resolved, and the `Durable` or `Abandoned`
      event was the record; (b) **the engineer's corollary:** move-only tokens notwithstanding, a token
      can be *closed* without resolving its failure, which stays in the inventory -- see DI-2.7
      point 7 for what that does to dropping a token. Original proposal -- unresolved failures are enumerable, and
      each is queryable for its state and its suspect inventory, which is frozen when the failure is
      observed. Once resolved, a handle still answers "resolved, and how", but the inventory is
      released, with a bounded history as in [D-72](../windows-ioring-sys/DESIGN-NOTES.md#d-72).

## Moved 2026-10-05 18:12:43 -04:00 -- DI-M1 complete: the contract

### <a id="di-14"></a>DI-1.4 -- The contract is written, prose first, in CONTRACT.md. *(completed 2026-10-05 18:12:43 -04:00)*

The authoritative text is [CONTRACT.md](CONTRACT.md). The item as it stood at completion is kept
below: its ten points are the checklist the contract was written against, each now a section or a
numbered statement there.

- [x] **DI-1.4** -- **Write the contract, prose first.** As the sample's
  [contract.rs](../windows-ioring-sys/examples/epoch_log/contract.rs) was: guarantees,
  non-guarantees, requirements of the handle and of the consumer, and assumptions. Written in terms
  of durability, with D-47 as an implementation floor ([DI-D-3](DESIGN-NOTES.md#di-d-3)). Depends
  on DI-1.2.

  **It must state, at least:**

  1. **What "durable" means for an epoch** -- including that it covers only writes that completed
     successfully, up to their reported byte counts.

  2. **The prefix property and `make_durable_through`'s rules** ([DI-D-9](DESIGN-NOTES.md#di-d-9)).

  3. **The suspect window's bounds** -- every write *pushed before the failure was observed* that no
     earlier successful covering flush covers. (Proposed; an earlier "completed before the failed
     flush completed" bound was found unsound, since completion order does not bound what a failure
     touched.)

  4. **Release-after-durable.**

  5. **The failure states and their resolution** (DI-1.2).

  6. **That "durable" is truthful relative to what the instance was told** -- every covering flush
     it issued succeeded and no failure in its flush regime was reported to it, by the ring or
     through the import -- where the regime is at least the device and includes other processes'
     flushes and the system's own write-back, none of which the instance can observe.

  7. **That nothing is claimed across a process lifetime** except that what was reported durable
     was durable.

  8. **That ordering overlapping writes is the consumer's** ([DI-D-6](DESIGN-NOTES.md#di-d-6)).

  9. **That the device honouring the flush is assumed, not checked.**

  10. **A worked example of failures and epochs being many-to-many** (the engineer's request, from
      DI-1.2 Q7): one failure spanning a sealed epoch, a pending one and an open one, and one open
      epoch caught in two failures, showing when it passes the watermark and how one abandonment
      makes it abandoned. Once there is code, the example becomes a compiled doctest driven by the
      fault-injecting implementation (DI-3+.3), so a contract change that invalidates it breaks the
      build rather than leaving it to rot.

This completes **DI-M1**. Its other items were archived as they completed: [DI-1.1](#di-11),
[DI-1.2](#di-12) and [DI-1.3](#di-13).

## Moved 2026-10-05 18:36:17 -04:00 -- the scope of an epoch number

### <a id="di-28"></a>DI-2.8 -- An epoch number's scope is one instance, with one author per epoch space. *(completed 2026-10-05 18:36:17 -04:00)*

Approved by the engineer, with the instruction that the scope of uniqueness be well documented. The
authoritative statement is [DI-D-14](DESIGN-NOTES.md#di-d-14), and the scope is stated in
[CONTRACT.md](CONTRACT.md) under Terms and under what the contract requires of the consumer. The item
as approved:

- [x] **DI-2.8** -- **Independent clients sharing one instance.** Raised by the engineer's question
  whether independent clients may have to coordinate epoch numbering. The assistant's analysis, not
  decided:

  1. [x] **Separate instances need no coordination.** Each instance's epoch space is its own; a
     number means something on two rings only if one client chooses that (the LSN example in
     [API.md](API.md)). The same holds for the engineer's layered stack: each layer has its own epoch
     space, and a layer maps its epochs onto the one below rather than sharing them.

  2. [x] **Sharing one instance forces coordination, and badly.** `make_durable_through(n)` seals a
     prefix of *everyone's* numbers, so client A sealing 100 seals B's 99, and B's next write tagged
     99 is refused. Partitioning the space (A even, B odd) does not help, because a prefix seal
     cuts across any partition. The only working arrangement is one shared allocator -- in effect a
     single author of the epoch space.

  3. [x] **Sharing couples more than numbering.** The commit barrier is instance-wide, so one
     client's commit waits on the other's operations; and a failure's suspect set is instance-wide,
     so one client's flush failure puts the other's writes at risk. Separate instances avoid all
     three.

  4. [x] **Recommendation:** state in the contract that an instance has one epoch space with one
     author, and that independent clients use separate instances or share one allocator. Several
     independent sequences per instance ("lanes") would be a feature of its own, not proposed and
     not foreclosed.

## Moved 2026-10-05 19:16:00 -04:00 -- the API shape for tags and the completion queue

### <a id="di-21"></a>DI-2.1 -- The tag and completion-queue types are API.md, generic over the epoch type. *(completed 2026-10-05 19:16:00 -04:00)*

Approved by the engineer ("api.md looks good"). The authoritative text is [API.md](API.md); the
decision is [DI-D-15](DESIGN-NOTES.md#di-d-15). The item as it stood at approval:

- [x] **DI-2.1** -- **The tag type and the completion stream's type.** The tag's concrete type;
  operation completions interleaved with durability entries (`Durable { through }`, failed, and the
  no-op's report of an already-sealed epoch's state); and how a write's push carries its tag.
  **Drafted in [API.md](API.md), proposed; this item closes when the engineer approves it.** At the
  engineer's direction the epoch type is generic -- `DurableRing<B, E = u64>`, `E: Copy + Ord +
  Debug` -- so comparability across instances is the consumer's choice through types, which answers
  [DI-D-9](DESIGN-NOTES.md#di-d-9)'s first cost; the per-instance guard stays on dioring-minted
  identities. The draft also proposes `OpId` ordered by push order
  ([DI-D-6](DESIGN-NOTES.md#di-d-6)'s open order question).

## Moved 2026-10-05 20:36:01 -04:00 -- composition with windows-ioring-sys

### <a id="di-22"></a>DI-2.2 -- dioring's composition with `windows-ioring-sys` is API.md sections 7-10. *(completed 2026-10-05 20:36:01 -04:00)*

The decision is [DI-D-17](DESIGN-NOTES.md#di-d-17), and the authoritative text is
[API.md](API.md) sections 7-10. Points recorded only here:

- **The item was blocked on the ring crate, and the block was cleared there rather than worked
  around.** Drafting against the real `windows-ioring-sys` surface found two gaps: file
  registration had no safe form, and a refused push dropped its buffer. A wrapper crate,
  `win-ioring`, was sketched and dropped in favour of fixing the layer: `M30.1` and `M30.2` there
  ([D-80](../windows-ioring-sys/DESIGN-NOTES.md#d-80),
  [D-81](../windows-ioring-sys/DESIGN-NOTES.md#d-81)). dioring then consumes the prerelease ring
  crate by path ([DI-D-16](DESIGN-NOTES.md#di-d-16)).
- **The engineer's five answers** were: both owned buffers and registered spans; the consumer's
  sidecar as a generic parameter; files left to the assistant; buffer registration mediated by
  dioring, with placement the topology planner's; and removal once nothing is in flight. On the
  draft, the engineer accepted dioring holding the registered buffers, and accepted the stricter
  removal rule on the condition that the refusal be descriptive. `FileBusy` is that refusal.
- **Carried forward, open:** when the kernel's registration completion itself fails during
  construction, the ring crate drops the buffers rather than returning them. That is the question
  recorded in its [M30.1 archive](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m301).

## Moved 2026-10-05 21:29:18 -04:00 -- completion routing under Model A and Model B

### <a id="di-23"></a>DI-2.3 -- Completion routing: an I/O-free core with a front end per model, delivering one entry at a time in order. *(completed 2026-10-05 21:29:18 -04:00)*

The decision is [DI-D-18](DESIGN-NOTES.md#di-d-18). The item as it stood at completion, with each point's outcome, follows verbatim. Point 11's prerequisite moved to `DI-3+.2` on the engineer's agreement: the design binds to the contract `M31.1` states, and the first code that relies on it is `DI-3+.2`.

- [ ] **DI-2.3** -- **Completion routing under Model A and Model B.** The two delivery
  architectures `windows-ioring-sys` supports ([D-3](../windows-ioring-sys/DESIGN-NOTES.md#d-3)):
  **Model A, threadless dispatch**, is the thread pool's -- the ring's completion event is wired to
  a pool wait (`EventDelivery`), and a pool thread runs a callback when completions arrive, so no
  thread of the consumer's blocks; **Model B, dedicated domain thread**, is a pinned thread per
  domain that owns its ring and drains it itself, parked in `submit_and_wait` or in a multi-object wait on the ring's completion event.
  This crate must observe every completion to advance its high-water mark: natural on a Model B
  drain; under Model A it must sit inside `EventDelivery`'s callback. Both models are first-class,
  so both are designed in.

  Accepted by the engineer, 2026-10-05 ("this all is reasonable"), to be recorded as a decision when
  DI-2.3 closes:

  1. [x] An I/O-free core (epochs, failures, held operations, the entry queue) with two front ends:
     `DurableRing` for Model B (`&mut self`, the consumer pops), and a Model A type that owns the
     delivery, with `&self` methods usable from any thread.
  2. [x] Model A delivers through a consumer callback receiving `Entry<B, E, C>` on a pool thread.
     dioring translates ring completions inside its own delivery callback first.
  3. [x] Under Model A, the consumer's callback runs with no lock held, so it may call back into
     dioring, and locks are taken in a fixed order.
  4. [x] Verify that `EventDelivery` serializes its callbacks. DI-D-13's ordering (`Durable` after
     the completions it covers) depends on it. **It does not.** Its callback drains, re-arms the
     wait, then drains again, so a completion arriving after the re-arm can start a second callback
     on another pool thread while the first is still draining. Each completion is delivered once,
     but two threads may hand completions over in either order. dioring must order delivery itself:
     see point 8.
  8. [x] **Ordering under concurrent delivery** (from point 4; accepted by the engineer, who
     confirmed "serialized" means strictly one entry at a time, in queue order). dioring records each
     completion and appends its entries to its own queue under its lock, so queue order is state
     order. `Durable { through: n }` is appended only once every write tagged n or lower has been
     *recorded* complete, not merely popped. A single deliverer drains that queue: whichever
     callback finds no one delivering takes the role and hands entries over until the queue is
     empty, while the others only append and leave. Consumer callbacks are then serialized without
     a lock held across them.
  9. [x] **Withdrawn: a per-request event.** The engineer suggested it to break up a weaker
     ordering than point 8 delivers. With entries handed over strictly one at a time, it is not
     needed, and he asked that it be set aside.
  5. [x] An entry the kernel never announced -- produced by `abandon`, or a refused gate --
     signals the ring's own completion event. That wakes a Model B waiter and fires the Model A
     pool wait, so every entry arrives the same way.
  6. [x] Model B waits as the ring crate does: non-blocking `pop`, bounded `pop_within`, and
     `completion_event` for multi-object waits.
  7. [x] Model A teardown is DI-2.7 point 5; DI-2.3 leaves it room.

  10. [x] **dioring keeps its own lock; locks are never shared across layers.** The engineer:
      "do not try to share locks. bad coupling." A single lock would be better in itself, but
      sharing the ring's would exploit an implementation artifact. Supporting it properly would
      mean making the ring crate a synchronization provider, which is outside its scope. Under
      Model A, dioring's state has its own lock, separate from the ring lock inside
      `EventDelivery`, and the order is always dioring's lock first, then the ring's. Both paths
      that need both locks follow it: a consumer push (check the seal, mint the `OpId`, then push),
      and a gate release inside delivery (record, then push).
  11. [ ] **Bind only to `EventDelivery`'s specified contract.** Point 10's order is safe only if
      `EventDelivery` calls dioring with the ring lock released, and point 8 assumes callbacks may
      overlap. Both hold today as implementation only. They are not stated on the public API, so
      relying on them now would be the same coupling point 10 refuses. They are specified in the
      ring crate first, then cited here.

      > **CROSS-COMPONENT PREREQUISITE:** `crates/windows-ioring-sys` -> `M31` -> `M31.1` (state and pin `on_completion`'s re-entrancy and concurrency on the public surface). See [CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md).

## Moved 2026-10-05 22:54:02 -04:00 -- durability lineages

### <a id="di-29"></a>DI-2.9 -- Durability lineages: an epoch is a lineage plus an epoch id, and lineages are minted, enumerable and retirable. *(completed 2026-10-05 22:54:02 -04:00)*

The decision is [DI-D-19](DESIGN-NOTES.md#di-d-19), whose closing summary states the settled specifics, and the authoritative text is [API.md](API.md) section 12. Point 8's outcome is [DI-D-21](DESIGN-NOTES.md#di-d-21), whose specifics continue as `DI-2.10`. The item as it stood at completion follows verbatim, including point 1's superseded answer and point 5's correction.

- [ ] **DI-2.9** -- **Durability lineages: the specifics** ([DI-D-19](DESIGN-NOTES.md#di-d-19)).
  Lineages are adopted as the normal way to get independent durability within one instance; this
  item settles how they work. Working position from the session (not decided):

  1. [x] **A file belongs to exactly one lineage**, fixed when the file is given to dioring. A
     flush acts on a whole file, so a file shared by two lineages would let one lineage's flush
     failure put the other's writes at risk. A write then takes its lineage from its file, so the
     write call is unchanged. One lineage may span several files and devices.
     **Alternative raised by the engineer:** the file's lineage is a *default*, and a write may name
     another. Nothing becomes incorrect; the cost is that a file shared by lineages shares its
     failure scope. A failed flush on it puts at risk the uncovered writes of every lineage that
     wrote there, so that failure belongs to each of those lineages. That extends DI-D-12's
     many-to-many between failures and epochs to lineages. A flush made for one lineage also makes
     another's completed writes durable on disk, but does not credit them: each lineage advances
     only by its own seal. The assistant's lean is a default with an explicit override, keeping
     the coupling visible as the consumer's choice. If adopted, the override is a `WriteOptions`
     setter ([DI-D-20](DESIGN-NOTES.md#di-d-20)), so the plain `write` is unchanged.
     **Settled by the engineer, 2026-10-05: the default.** A file's lineage is its default; a
     write may name another through `WriteOptions`, and a file written by several lineages
     shares its failure scope among them. **Superseded the same evening, choice (a):** every
     epoch now names its lineage (`Epoch<E> { lineage, id }`), so a file has no lineage at all
     and there is no override. A file written by several lineages still shares its failure scope
     among them ([API.md](API.md) section 12).
  2. [x] **Lineage identity.** Settled by the engineer, 2026-10-05: **a handle dioring mints**,
     not a consumer-chosen key. dioring has APIs to **enumerate** its lineages. When minting, the
     consumer **may supply a UTF-8 description**; it is for people reading reports and
     enumerations, and is not part of the identity. Working position for the details: the handle is
     `Copy` and carries its instance, as `OpId` and `FailureId` do, so another instance's handle
     is refused; the description is optional at minting and returned by the enumeration, which
     returns owned values, as `failures()` does. **Lifecycle, also settled by the engineer:**
     lineages can be minted **at any time**, and an **empty lineage can be retired**. Working
     position for "empty": it has nothing in flight, nothing held
     for a gate, no write that no successful flush has covered, and no unresolved failure. A
     refused retirement is descriptive, like `FileBusy`, and reports each hold and what clears it.
     A retired handle is never reused, as a `FailureId` is not, and is refused from then on. Whether
     the default lineage (point 3) can be retired belongs to point 3.
  3. [x] **The default lineage:** an instance with one implicit lineage keeps today's API as the
     simple case, so the lineage parameter appears only where it is needed. **Settled by the
     engineer, 2026-10-05:** the default lineage has a presence -- it is a real lineage, with a
     handle, and is found through the enumeration like any other. Working position for the
     details: the enumeration marks which lineage is the default, and the default cannot be
     retired, because it is the lineage a consumer that never mints one uses.
  4. [x] **API amendments.** The engineer, 2026-10-05: everywhere API.md takes an epoch, it takes
     a struct holding both the lineage and the epoch. That struct is the **`Epoch`**, and the
     former epoch value is renamed the **epoch id**, "better in cases where it is not, logically,
     a number". Applied in [API.md](API.md) section 12 and the sketch, compile-checked against the
     ring crate: `Epoch<E> { lineage, id }`, only `PartialOrd`; the `EpochId` trait, formerly
     `EpochTag`; `durable_through(lineage)` and `sealed_through(lineage)`; `mint_lineage`,
     `retire_lineage`, `default_lineage` and `lineages`; and gates that may cross lineages.
  5. [x] **Failures and imports per lineage:** a failed flush is a failure in every lineage with
     an uncovered write to that file; an imported failure names its lineage, or all of them.
     Approved by the engineer, 2026-10-05 ("point 5: ok!"). In [API.md](API.md):
     `import_failure(ImportScope)`, where the scope is one lineage or all; a failure's lineages
     are read from its suspect set, whose writes carry their `Epoch`, rather than stored beside it;
     and one resolution applies to the failure in every lineage it belongs to. **Correction found
     under point 7:** "every lineage with an uncovered write *to that file*" was narrower than the
     contract. DI-D-12(b) makes the suspect set every uncovered write in the instance, so as
     specified a failed flush reaches every lineage with an uncovered write anywhere. Point 8.
  6. [x] **Ordering:** `Durable` follows the completions of its own lineage's writes at or below
     n, and one deliverer still serves every lineage ([DI-D-18](DESIGN-NOTES.md#di-d-18)).
     Approved by the engineer, 2026-10-05 ("point 6: ok!"). Nothing is promised about order
     between lineages beyond the single queue's order.
  7. [x] **Re-read the approved decisions** that say "instance" for what is now a lineage's --
     [DI-D-9](DESIGN-NOTES.md#di-d-9), [DI-D-12](DESIGN-NOTES.md#di-d-12),
     [DI-D-14](DESIGN-NOTES.md#di-d-14) -- and [CONTRACT.md](CONTRACT.md), and amend each. The
     same pass sweeps the old terms in those live texts: "epoch number" and "epoch type" become
     "epoch id" and "epoch-id type". The archive and the session record keep their wording.
     **The engineer's caution, 2026-10-05:** "when a client wants no interaction, another instance
     is a safe bet." So the rewrite does not replace "separate instances" with "separate lineages"
     wholesale. Where a text speaks of independent *durability* (epoch scope, seal, failure set),
     it becomes per lineage. Where it speaks of independence in general -- DI-D-14's "keep commit
     cost and failure scope independent", CONTRACT.md's "independent clients use separate
     instances" -- it keeps instances as the answer for no interaction, and adds lineages for
     independent counters on a shared ring ([DI-D-19](DESIGN-NOTES.md#di-d-19) lists what lineages
     still share).
     **Done 2026-10-05.** [CONTRACT.md](CONTRACT.md): the guarantee sentence, Terms (lineage;
     epoch and epoch id; the scope of an epoch id; seal; high-water mark), guarantees 2, 5, 6 and
     9, the epoch-id type and one-author requirements, and the examples' default lineage. DI-D-9
     and DI-D-12 carry amendment markers; DI-D-14's title uses "epoch id". The suspect-set scope
     was left unchanged and flagged, because it needs a decision: point 8.
  8. [x] **The default suspect-set scope across lineages.** DI-D-12(b) and CONTRACT.md make a
     failure's suspect set every uncovered write in the *instance*, because dioring cannot see
     which writes share a device's volatile cache. With lineages, that means one failed flush puts
     every lineage with any uncovered write at risk, so lineages on separate devices share
     failures. Options: keep the instance-wide default (safe; independent failure scope then needs
     a separate instance); narrow the default to the failed flush's file (unsafe where files share
     a device cache, which dioring cannot see); or keep it instance-wide and let a consumer declare
     which files share a flush regime, so a failure reaches only its regime -- the parked
     [DI-6+.2](CHECKLIST.md), caller-declared flush regimes. **Decided by the engineer,
     2026-10-05: "caller declared flush regimes"** ([DI-D-21](DESIGN-NOTES.md#di-d-21)). The
     instance-wide scope stays the default; a consumer narrows it by declaring flush regimes.
     Specifics in DI-2.10.

## Moved 2026-10-06 07:00:00 -04:00 -- caller-declared flush domains

### <a id="di-210"></a>DI-2.10 -- Caller-declared flush domains: a set of opaque byte keys per file, fixed at registration, with failures reaching files whose sets intersect. *(completed 2026-10-06 07:00:00 -04:00)*

The decision is [DI-D-21](DESIGN-NOTES.md#di-d-21), whose closing summary states the settled specifics, and the authoritative text is [API.md](API.md) section 13 and [CONTRACT.md](CONTRACT.md)'s failures section and requirements. The item as it stood at completion follows verbatim.

- [ ] **DI-2.10** -- **Caller-declared flush domains: the specifics**
  ([DI-D-21](DESIGN-NOTES.md#di-d-21)). Graduated from the parked `DI-6+.2`, whose concept and
  handover are recorded at `M33+.5` in [CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md).
  Reworked 2026-10-06 from "one declared regime per file" to **a set of flush domains per file**.
  A **flush domain** is the unit whose volatile cache a flush commits, and whose failure everything
  behind it shares: a disk, a controller cache, a virtual disk, or a remote share. It is not always
  a storage device, which is why dioring names the abstraction rather than the hardware. Working
  position (not decided):

  1. [x] **The default:** a file declared with no domains is *unknown* and shares fate with every
     file. So the naive use -- nothing declared -- is logically one flush domain for the whole
     instance, today's contract, and a partly declared instance stays safe for its undeclared
     files. Settled by the engineer, 2026-10-06 ("the naive use is just logically a single
     instance-wide flush domain"). Over-declaring is safe in the same way: extra domains only widen
     who shares a failure.
  2. [x] **Identity:** a consumer-supplied opaque key (like `FileKey`), not a dioring-minted
     handle, because it names something physical that two instances or processes must be able to
     name alike. dioring only compares keys. The Windows-derived keys come from the locality
     helper (root `M39`). **Settled by the engineer, 2026-10-06:** the key is the identifier's
     **exact bytes**, and dioring **interns** each distinct key into a small pointer-sized value
     for its own use. Equality is then exact -- no hash, so no collisions -- and every comparison
     after declaration is a word compare. Working position for the details: the consumer supplies
     the bytes when declaring a file's domains; reports that name a domain return its bytes, so
     they stay meaningful outside the instance; the interned value is internal; and an interned
     key lives for the instance's life. The number of distinct domains is the number of devices
     and shares the consumer touches, so the table stays small without reference counting.
  3. [x] **Declaration:** a file is given its domain set when it is given to dioring
     (`Setup::files`, `add_file`). Whether the set can change later, and when. **Settled by the
     engineer, 2026-10-06: "specified at registration only"** -- read as the moment the file is
     given to dioring, by either path, not only the kernel registration made at construction. The
     set is fixed for the file's life in the instance; changing it means `remove_file` and adding
     the file again, and `remove_file` already refuses while the file has uncovered writes, so a
     set can never be narrowed under writes it still covers.
  4. [x] **The suspect set** (approved by the engineer, 2026-10-06: "correct"): for a failed flush on file F, the uncovered writes to every file
     whose domain set intersects F's -- unknown files included -- in observation order, as
     [DI-D-12](DESIGN-NOTES.md#di-d-12)(b) defines. Intersection, not partition: a file on a
     volume striped over disks A and B shares fate with a file on B alone, and a partition could
     express that only by merging them.
  5. [x] **Imports:** an outside failure is physical, so its natural scope is a set of flush
     domains. Reconcile with DI-2.9's `ImportScope` (a lineage, or all). **Approved by the
     engineer, 2026-10-06 ("looks right"):** `ImportScope` is `All`, `Lineage(lineage)` or
     `Domains(set)`. A domain-scoped import puts at risk the uncovered writes to files whose
     domains intersect the set, unknown files included. No combinations; a consumer needing a
     lineage and domains together imports twice.
  6. [x] **The consumer's warranty** (approved by the engineer, 2026-10-06: "true, needs clear
     documentation"; written into [CONTRACT.md](CONTRACT.md)'s requirements on the consumer, with
     the cost of a missing domain, what is always safe, and a striped-volume example) that each file's declared set contains every domain its
     writes depend on, written into [CONTRACT.md](CONTRACT.md)'s requirements, with what a wrong
     declaration costs: a failure that reaches an undeclared dependency goes unreported.
  7. [x] **API and contract amendments** (approved by the engineer, 2026-10-06: "good"; done --
     [API.md](API.md) section 13 and the sketch, compile-checked against the ring crate:
     `FlushDomain`, `FileOptions`, `FileSetup`, `add_file_with`, `ImportScope::Domains`,
     `Cause::Imported { scope }`, and the internal interning table; CONTRACT.md's failures
     section states the intersection rule): [API.md](API.md) (domain keys on file setup, imports)
     and CONTRACT.md's failures section, where the physical "flush regime" becomes derived: the
     writes to every file sharing a domain with the failed one.

## Moved 2026-10-06 14:12:33 -04:00 -- multi-file seals

### <a id="di-24"></a>DI-2.4 -- Multi-file seals: completion-gated flushes only, gate cycles detected, and the seal's file set fixed when it is requested. *(completed 2026-10-06 14:12:33 -04:00)*

The decisions are [DI-D-22](DESIGN-NOTES.md#di-d-22) (completion-gated flushes only) and [DI-D-23](DESIGN-NOTES.md#di-d-23) (gate cycles detected); the contract change is [CONTRACT.md](CONTRACT.md)'s non-guarantee `Nothing about writes outside the seal`. Point 2 moved to DI-2.12, point 3's optimization is DI-6+.7, and the redefinition of `covered` that point 5 surfaced is DI-2.12 point 9. The item as it stood at completion follows verbatim.

- [ ] **DI-2.4** -- **Multi-file epochs.** The set of files each epoch wrote, and how
  `make_durable_through(n)` over epochs that touched N files is done: a chain of N covering flushes
  (serialised device flushes, and a barrier that also waits on writes of still-open epochs above
  `n`) or host sequencing (observe only the writes tagged at or below `n`, then N unordered flushes
  that may proceed together -- the tag lets it ignore later work). Offering both was the
  assistant's working position, never decided (corrected 2026-10-06). Renamed at the engineer's
  prompting, since the old names were uninformative: the chain is **ring-barrier flushes**, host
  sequencing is **completion-gated flushes**. Points, each with the assistant's first view:

  1. [x] **The strategy.** **Decided by the engineer, 2026-10-06: completion-gated flushes "only"**
     ([DI-D-22](DESIGN-NOTES.md#di-d-22)). Ring-barrier flushes are not offered, so the question
     of where a strategy is chosen and the chain's shape both fall away.
  2. [x] **Moved to DI-2.12, 2026-10-06** (the engineer: "this is a design point on flush domains
     now right?"). dioring's core issues no flushes of its own: the provider chooses per-file or
     per-domain under its own warranty, the default provider syncs each file, and per-domain
     syncing for it is DI-6+.6. dioring never coalesces on declared domains by itself, so
     over-declaring stays safe. As first written: **One flush per file, never one per flush domain.** A file flush also commits that
     file's metadata, and declared domains scope failures only; if they also elided flushes,
     over-declaring would stop being safe. State it explicitly. **Reopened by the engineer,
     2026-10-06:** "why not one per domain? i was thinking that perhaps the flush domains should
     come with functors for them to do their flushing, but now its feeling like we should have
     flush domain registration apis." Reshaped into the durability provider; carried by
     **DI-2.12**, which this point now waits on.
  3. [x] **Settled 2026-10-06 by separating contract from optimization.** The engineer:
     "contractually, what is guaranteed is that the series of writes in the lineage. If additional
     writes are written, that's not incorrect, that's a performance issue." So a provider call must
     make durable the sealing lineage's named writes, and a success warrants those; making more
     durable is permitted and never reported early -- CONTRACT.md's non-guarantee "Nothing about
     writes outside the seal" (widened from open epochs to other lineages). Widening a call to
     other completed writes on the same files, and crediting them to later or concurrent seals, is
     an optimization, parked as **DI-6+.7**. As first written: **Credit coverage across seals and
     lineages.** A successful flush on a file covers every
     write to it whose completion was observed before the flush was issued, whatever its lineage or
     epoch. A later seal, or one arriving while a flush on that file is in flight, uses that
     coverage rather than flushing again. Fewer flushes issued; the tracking already exists.
  4. [x] **Decided by the engineer, 2026-10-06: gate cycles are detected.** "I think we have to
     detect it. The alternative leads to a lot of extra work around enumeration, modification and
     cancellation of lineages that I don't really want to have to do." The assistant's proposed
     mechanism, not yet approved: a gated write tagged `L:a` with gate `M:b` adds the dependency
     "L reaching a needs M reaching b"; because durability is a prefix, a cycle exists when M
     reaching b already needs, transitively, L reaching a or beyond. The push that would close a
     cycle is refused before anything is reserved, as every legality check is
     ([D-28](../windows-ioring-sys/DESIGN-NOTES.md#d-28)), with an error naming the cycle; the
     same-lineage case (`L:a` gated on `L:b`, `b >= a`) is its one-edge form. Edges leave the graph
     when a gate releases or its epoch is abandoned. Only gates dioring sees are covered: a consumer
     that waits in its own code for one lineage before writing another can still deadlock itself.
     **Caveat for the implementation (found 2026-10-06):** a heal takes effect only when the first
     seal made after it completes, so M reaching a failed epoch `b` can depend on M reaching an
     epoch *above* `b`; the search must not assume reaching `b` depends only on writes at or below
     `b`.
     As first written: **Seals that wait on gated writes, and gate cycles.** A write at or below `n` may be held
     by a gate, so the seal waits until it is issued and covered. Within one lineage a write gated
     on its own epoch or later can never be released, and can be refused at push. Across lineages a
     cycle (L:5 gated on M:3, M:3 gated on L:5) cannot be seen from one write: detect it, or
     document that it never resolves.
  5. [x] **Approved by the engineer, 2026-10-06.** The seal's file set is fixed when the seal is
     requested (rule (a) refuses later writes at or below `n`) and includes in-flight and gated
     writes; the call is made once all have completed. What "covered" means under the provider is
     DI-2.12 point 9. As first written: **The set of files each epoch wrote.** Falls out of the
     uncovered-write tracking that the suspect set, `Durable`'s ordering and `remove_file` already
     need; close with no separate structure specified.

  **The engineer's expectation, 2026-10-06** (stated as a characteristic, because the strategy
  names are uninformative): "if i make a durability epoch which is separable from another
  durability chain's flush domains, they will not overlap on make durable events." Clarified: not
  a guarantee -- "they may overlap. i was stating what would happen for a careful consumer who kept
  everything very orderly." So the bar is that dioring's own flush mechanism adds no coupling of its
  own between lineages a consumer keeps separable; overlap from shared resources remains, and is
  made visible by DI-2.11. The chain's ring-wide barrier adds such coupling; host sequencing does
  not.

  **Consequence recognised by the engineer, 2026-10-05:** an instance has one high-water mark, and
  a seal covers every file at or below n, so a consumer that wants **independent durability per
  device** must use one instance -- and so one ring -- per device. That follows from the prefix seal
  ([DI-D-9](DESIGN-NOTES.md#di-d-9)) under either strategy, not from the chain. **Answered by
  lineages ([DI-D-19](DESIGN-NOTES.md#di-d-19), specifics in DI-2.9, which this item now follows):**
  independent durability is a lineage per device on one ring. DI-2.4 therefore settles how one
  lineage spanning several files is sealed. The chain's barrier covers the whole ring, so a
  lineage's covering flush would wait for every other lineage's earlier writes; that is why point 1
  chose completion-gated flushes.

## Moved 2026-10-06 15:14:11 -04:00 -- the contract as a trait

### <a id="di-25"></a>DI-2.5 -- The contract is a trait: identity types associated, generic only, a registered-buffer extension trait, a readiness signal, and a conformance oracle. *(completed 2026-10-06 15:14:11 -04:00)*

The decision is [DI-D-24](DESIGN-NOTES.md#di-d-24). The readiness signal's form and wake-up rules continue as DI-2.14, restating [API.md](API.md) as the trait is DI-2.15, and the conformance oracle is DI-3+.7. The item as it stood at completion follows verbatim.

- [ ] **DI-2.5** -- **A trait or a concrete type first.** The contract is something layers above may
  implement as well as consume; decide whether it is a trait from the start, or a concrete type
  until the fault-injecting implementation (DI-3+.3) gives a second implementation to extract one
  from.

  **The engineer, 2026-10-06: "I would prefer a trait."** Asked what challenges that poses; the
  assistant listed seven, and the engineer answered:

  1. [x] **Identity types are associated types.** `Lineage`, `OpId`, `FailureId` and
     `FailureToken` embed a private instance identity or a cell into their instance, so a second
     implementation could not mint them. The engineer: "yes associated types". Plain values --
     `Epoch`, `FileKey`, `FlushDomain`, the options builders -- stay shared.
  2. [x] **Generic only, not object-safe.** The engineer: "yes generic only. We will make things
     free functions when needed." Construction stays on the concrete types.
  3. [x] **A core trait plus an extension trait for registered buffers**, since a fault injector
     or a non-ring layer may have none. The engineer: "agreed, I assume you can do this."
  4. [x] **Front ends over the trait.** Decided 2026-10-06 -- the engineer: "why wouldn't it have
     such a signal?"; the case against was only a test double maintaining a signal nobody waits
     on. Its form and wake-up rules are **DI-2.14**. Model A and Model B need `pop` and a way to
     know there is something to pop: front ends are written once, generic over the trait, which
     carries a readiness signal set whenever an entry becomes available -- from a ring completion
     or a provider's out-of-band completion alike, which also answers DI-2.12 point 4's waking
     question. Rejected: a `pop`-only trait with front ends rebuilt per implementation.
  5. [x] **A conformance oracle in dioring.** A trait makes CONTRACT.md bind other implementations,
     so dioring ships a reusable checker over the event stream that every implementation's tests
     run, after `windows-file-watcher`'s `ContractChecker`. The engineer: "yes".
  6. [x] **Evolution** (a consequence of choosing a trait, not a separate decision). A public
     trait others implement cannot gain a required method without a major release, and sealing it
     would defeat the purpose; additions come as default methods or extension traits. Noted, not
     discussed.
  7. **Unaffected:** buffers move by value and come back on refusal; nothing is `async`; the
     durability provider (DI-2.12) is a separate trait at a different layer.

## Moved 2026-10-06 16:29:12 -04:00 -- the operation set

### <a id="di-26"></a>DI-2.6 -- The operation set: no consumer flush, per-write write-through passed through, no per-operation cancel, and a completion fence parked. *(completed 2026-10-06 16:29:12 -04:00)*

The authoritative text is [API.md](API.md) section 11 (`WriteOptions::caching`, no consumer flush). The completion fence is parked as DI-6+.8, and per-operation cancel stays parked as DI-6+.4. The item as it stood at completion follows verbatim.

- [ ] **DI-2.6** -- **The operation set.** Reads go through the ring and are not epoch members;
  tagged writes; `make_durable_through`. No consumer operation may interfere with dioring's control
  of the ring ([DI-D-10](DESIGN-NOTES.md#di-d-10)), so what remains open is which *mediated*
  operations dioring offers. **Cancel: the engineer leans towards not offering it in v1**, because
  cancel guarantees no response time -- it is a request, and the operation may still complete
  normally, later -- so it does not deliver what shutdown or a deadline wants from it; parked as
  DI-6+.4. **Registration** moved to DI-2.2, to be worked through when the API shape needs it.

  Reviewed 2026-10-06 against the wioring's operations (`Nop`, `Read`, `Write`, `Flush`,
  `RegisterFiles`, `RegisterBuffers`, `Cancel`):

  1. [x] **No consumer flush.** Durability comes only from `make_durable_through`, through the
     provider; a consumer's flush would bypass coverage accounting. The engineer: "agreed".
     Stated in [API.md](API.md) section 11.
  2. [x] **Per-write write-through.** `WriteOptions` carried only a gate, hiding the flag the
     wioring exposes (its D-25: hiding it "narrows the platform"). Added as
     `WriteOptions::caching(WriteCaching)`, dioring's own type so other implementations need
     nothing from the ring crate; latency only, never durability. The engineer: "agreed".
  3. [x] **A marker entry: parked as DI-6+.8.** The engineer: "I assume the use of it would be the
     next layer up. I don't know I don't see the use but if it can be a sequencing point that the
     consumer can depend on that only sequences i/o submissions (or completions?) rather than
     durability maybe it has utility?" Submissions are already ordered by `OpId`; completions are
     where a fence would mean something.
  4. [x] **No per-operation cancel.** The engineer: "I still don't think we want to get in the
     business of cancelling individual I/Os. I believe that entire larger structures may be
     abandoned but not surgical I/O cancellation. I may be proven wrong in the long run but right
     now I just can't see it." DI-6+.4 stays parked; abandoning larger structures is DI-2.13.

## Moved 2026-10-06 16:48:35 -04:00 -- ending an instance

### <a id="di-27"></a>DI-2.7 -- Ending an instance seals nothing, waits for kernel operations and provider calls, and close() hands back what is left. *(completed 2026-10-06 16:48:35 -04:00)*

The decision is [DI-D-25](DESIGN-NOTES.md#di-d-25); the authoritative text is [CONTRACT.md](CONTRACT.md)'s `Ending an instance` and [API.md](API.md) section 14. Ending a lineage, which need not wait, continues under DI-2.13. The item as it stood at completion follows verbatim.

- [ ] **DI-2.7** -- **Review what dropping a dioring instance means.** Raised by the engineer while
  approving DI-1.2 Q3 ("I suspect none but something that should be reviewed"). Points to settle,
  each with the assistant's first view:

  1. [x] **No implicit durability on drop.** (Approved by the engineer, 2026-10-06: "yes".) Drop
     must not issue a final flush: that would be policy, it would block, and a failure would have
     nobody to report to. Dropping without `make_durable_through` promises nothing, consistent
     with the contract's non-guarantees.

  2. [x] **In-flight kernel operations.** (Approved, 2026-10-06: "yes".) dioring's ring performs
     the rundown `windows-ioring-sys` already specifies
     ([D-4](../windows-ioring-sys/DESIGN-NOTES.md#d-4): buffers are forgotten, not freed, if
     rundown cannot prove quiescence). dioring adds no blocking of its own, and states that drop may
     block for as long as that rundown does.

  3. [x] **Operations held for release-after-durable.** (Approved, 2026-10-06: "yes". Under
     lineages the final high-water mark is one per lineage. `close()` is an API addition: API.md
     gains it on the concrete type, and DI-2.15 carries it into the trait as a by-value method.)
     The kernel never saw them, so their buffers can simply be dropped -- but the consumer loses
     the fact that they never ran. An explicit `close()` that returns the leftovers (held
     operations, unresolved failures, the final high-water mark) before the instance is gone would
     preserve it, with `Drop` as the fallback.

  4. [x] **Unresolved failures and unpopped events.** (Approved, 2026-10-06: "yes".) Discarded
     with the instance. Nothing true becomes false: a durable fact stays durable whether or not its
     event was popped, and the consumer already received each `Failed` event or can enumerate
     failures before dropping.

  5. [x] **Model A teardown order.** (Approved, 2026-10-06: "yes".) Under `EventDelivery`,
     delivery must be quiesced before any state its callback reaches is released -- the order
     [D-13](../windows-ioring-sys/DESIGN-NOTES.md#d-13) already established for the ring crate.

  6. [x] **Drop during unwinding.** (Approved, 2026-10-06: "yes".) Follow the ring crate's rule
     ([D-79](../windows-ioring-sys/DESIGN-NOTES.md#d-79)): trace rather than panic while
     unwinding.

  7. [x] **Dropping an unresolved failure token** (from DI-1.2 Q9). **Settled by the engineer:
     the token is affine, and dropping it is `close()`** -- the failure returns, unresolved, to the
     inventory; `close()` stays as the explicit spelling; `#[must_use]` warns about an unconsumed
     token. The history of how this was reached follows. Rust cannot force the token to
     be used. Options: leave the failure unresolved, which stalls the high-water mark permanently with no
     way left to resolve it; abandon it implicitly and emit `Abandoned` with the drop as its cause,
     which never over-reports, since "durable through" already means "durable or abandoned"; or
     treat the drop as a defect and panic, as [D-79](../windows-ioring-sys/DESIGN-NOTES.md#d-79)
     does for defects, tracing instead while unwinding. `#[must_use]` on the token in every case.
     The assistant leans towards implicit abandonment, being the only option that neither bricks
     the instance nor crashes the process. **Superseded by the engineer's corollary in DI-1.2 Q10**
     (a token can be closed without resolving): dropping a token *closes* it; the failure stays
     unresolved and in the inventory, and the inventory can dispense its token again. Linearity
     then means *at most one live token per failure*, not "one token ever". This neither bricks the
     instance, nor crashes, nor invents an abandonment the consumer never asked for. Proposed by the
     assistant as the reading of the corollary; awaiting the engineer's confirmation.
     **Corrected by the engineer:** closing is not dropping. The token is linear, so it has an
     explicit **`close()`** -- the third way to consume it, beside `heal` and `abandon` -- which
     returns the failure, unresolved, to the inventory (from which its token can be dispensed again).
     What remains open here is only what an implicit *drop* does, since Rust cannot forbid it at
     compile time. The assistant's proposal: it is a violation of the token's linearity, so a
     defect, handled as the ring crate handles defects
     ([D-79](../windows-ioring-sys/DESIGN-NOTES.md#d-79)) -- panic, but only trace while unwinding --
     with the failure returned to the inventory either way, so even a defect never bricks the
     instance; plus `#[must_use]` so the compiler warns first.
     **The engineer then asked whether the token should be affine rather than linear.** The
     assistant's answer, recommending affine:
     - *Rust cannot give linearity at compile time.* The choice is only between an affine token
       whose drop is benign and an affine token whose drop panics at run time.
     - *A panicking drop turns ordinary error handling into crashes:* any `?` between receiving a
       token and resolving it, or a discarded error from a refused batch that carried tokens back,
       would panic.
     - *What linearity was protecting against is already loud without it.* A forgotten failure stays
       in the inventory, the high-water mark stops at it, and requests behind it get `Blocked`; nothing
       is silently lost and nothing is falsely reported durable.
     - *The workspace's precedent is affine with a defined drop:* `windows-ioring-sys` forgets rather
       than panics in the analogous cases ([D-4](../windows-ioring-sys/DESIGN-NOTES.md#d-4),
       [D-18](../windows-ioring-sys/DESIGN-NOTES.md#d-18)).
     - *So:* drop is `close()`, `close()` remains as the explicit spelling of it, and `#[must_use]`
       makes the compiler warn about an unconsumed token. The intent behind the linearity -- every
       failure is decided explicitly -- is kept, and the stalled high-water mark enforces it better than
       a panic would. Not decided; the engineer's call.

  8. [x] **Provider calls in flight at drop.** A gap the durability provider (DI-2.12) opened
     after points 1-6 were written. The assistant proposed not waiting; **corrected by the
     engineer, 2026-10-06:** "the responsibilities w.r.t. outstanding calls to durability providers
     are similar to the responsibilities to around outstanding ring operations. so in the end we
     have to await them. perhaps there should be a cancellation notification but I doubt it's
     worthwhile." Ending waits for them as it does for kernel operations; no cancellation
     notification is scheduled ([DI-D-25](DESIGN-NOTES.md#di-d-25)). Point 3's `close()` was added
     to API.md at the engineer's request ("add close here also").

## Moved 2026-10-06 17:29:06 -04:00 -- ETW events for delays

### <a id="di-211"></a>DI-2.11 -- Delay events: one manifest ETW provider owned by dioring, for locks and flushes, costing nothing when nobody listens. *(completed 2026-10-06 17:29:06 -04:00)*

The decision is [DI-D-26](DESIGN-NOTES.md#di-d-26). What each event names, the time base, the provider's name and GUID, and installing the manifest continue as DI-3+.6. The ring crate's `M31+.1` was queued and withdrawn during this item. The item as it stood at completion follows verbatim.

- [ ] **DI-2.11** -- **ETW events for delays.** The engineer, 2026-10-06: "delays are inevitable
  with shared resources. we should plan for etw logging for delays." Lineages and instances share
  submission-queue capacity, the deliverer, dioring's lock and (under Model B) a thread, so a seal
  can be delayed by work it has no durability relation to; the remedy is to make every such delay
  observable, not to promise it away. dioring would be the workspace's first ETW *provider* (the
  existing ETW work only consumes kernel traces). Working position, deliberately coarse:

  1. [x] **Which delays.** **The engineer, 2026-10-06:** "I have two major concerns for delays:
     flushes and locks. Locks are in our control and flushes are up to the flush domains control."
     So the primary events are lock waits (the pattern below) and flushes, measured at the
     provider boundary -- call issued, and each domain's outcome, so an event names the slow
     domain; what happens inside a provider is its own to report, and the default provider, being
     dioring's, can also time its per-file ring flushes. A seal's other phases -- waiting for
     covered writes to complete, and for gated writes to be released, neither a lock nor a flush --
     get a secondary event; the engineer: "I think it's very unlikely to warrant one but I'm ok
     with it." The rest of the first list becomes secondary candidates. The first list: a seal's
     phases (requested, last covered write completed, flushes issued, flushes completed, `Durable`
     delivered); an operation held because the submission queue was full; delivery delay from
     completion popped to callback; lock wait; a
     gated write's hold and the epoch it waited on; and, where a delay was caused by another
     lineage's work, which lineage.
     **The engineer's pattern for locks, 2026-10-06** (detail of events deferred): "instead of
     simply taking the blocking lock we would establish a pattern where we would always do the
     "try-lock" ... and then if that were to fail, capture a timestamp (interrupt time for
     efficiency), take a blocking lock, and after acquiring the lock, log the etw event with the
     delta in timestamp. So the fast path stays fast, the contended path logs an event." The
     assistant's notes, **agreed by the engineer 2026-10-06 ("all agreed, we can consider the time
     base later")**, the time base excepted (point 6): the timestamp is taken only on a path that
     is about to block, so a precise source (`QueryInterruptTimePrecise` or the performance
     counter) costs little there, while plain interrupt time advances only per clock tick and
     would read most microsecond waits as zero; emit the event after the guard is released, so writing it does
     not lengthen the hold and the next waiter's wait; put the pattern in one wrapper type with a
     name per lock rather than at each lock site; a failed try-lock can also mean poisoned, which
     is its own case; and the same shape -- try, timestamp, block, report the delta -- fits the
     other waits on the list. The `trace_record!` facility in `windows-threadpool-sys` is a
     feature-gated in-memory debug trace for races, a different purpose, and stays separate.
  2. [x] **Identity in events. Moved to DI-3+.6.** The engineer, 2026-10-06: "I think it's going to
     depend on the event. We don't have to design this now." As first written: instance, lineage
     (and its description), and the epoch id -- which is the consumer's generic `E`, so how it is
     rendered into an event field is open.
  3. [x] **Cost when nobody is listening: nothing.** The engineer, 2026-10-06: "nothing". With no
     session listening, no timestamp is taken and no event is built; the contended path checks
     whether the provider is enabled before it reads the clock.
  4. [x] **Provider mechanics: a manifest.** The engineer, 2026-10-06: "manifest". The provider's
     name and GUID, and whether it is dioring's alone or part of a workspace scheme, are DI-3+.6's.
     Consequence carried: a manifest-based provider's events decode by name only where its manifest
     is installed (`wevtutil im`, an administrative step), which an application shipping dioring
     must do or document.
  5. [x] **The layer that owns each delay: dioring.** The engineer, 2026-10-06: "we are generating
     events about durability delays, so we own it", and "the etw events we publish give a surface
     for diagnosis of issues; clients don't want to search around across providers to try to
     correlate issues." dioring publishes the one surface, measuring its own locks directly and
     whatever it calls -- the wioring, a provider -- at the boundary, so the wioring's lock shows
     as a slow call into the ring. The ring crate's `M31+.1` was withdrawn. Earlier text:
     dioring instruments its own lock and the provider boundary; the wioring's own lock
     (`EventDelivery`'s mutex, never shared per [DI-D-18](DESIGN-NOTES.md#di-d-18)) is the ring
     crate's to instrument with the same pattern; a provider instruments its own internals. As
     first written: Delays that originate in the ring itself (a full submission or completion
     queue) may belong to `windows-ioring-sys`'s own events rather than dioring's, per the
     mono-repo bug policy; decide which, and queue the ring crate's share there.
  6. [x] **The time base. Moved to DI-3+.6.** The engineer, 2026-10-06: "let's see what the
     timelines look like before we decide." Interrupt time, `QueryInterruptTimePrecise`, or the
     performance counter, chosen from observed timelines.

## Moved 2026-10-06 20:03:54 -04:00 -- the durability provider

### <a id="di-212"></a>DI-2.12 -- The durability provider: a built-in default plus one consumer provider, per-domain completion handles, no timeout, and per-domain failure scope. *(completed 2026-10-06 20:03:54 -04:00)*

The decision is [DI-D-27](DESIGN-NOTES.md#di-d-27). Points 2-9 were the assistant's stances, approved together by the engineer ("seems right, let's move on"). Writing them into [CONTRACT.md](CONTRACT.md) and [API.md](API.md) is DI-2.16; the router is DI-3+.8; the default provider's optimizations are DI-6+.6. The item as it stood at completion follows verbatim.

- [ ] **DI-2.12** -- **The durability provider.** From DI-2.4 point 2. The engineer first proposed
  registered domains and a three-phase flush session, so that coupled domains with different names
  could minimise the physical flush count. Reshaped 2026-10-06 after the assistant's feedback that
  per-domain participants and a session protocol add back the coupling they were meant to remove
  (the session record has both); the engineer: "I generally like this, it's what I expect dioring
  to be on top of the wioring". Working position, not decided:

  1. [x] **A built-in default plus at most one consumer provider** (as first written: "One
     provider per instance"). Consumer-supplied, behind a trait. dioring's own provider
     -- each file syncs itself through dioring's wioring -- is the default, so a naive consumer
     never meets the interface. **The engineer, 2026-10-06:** "I hope we can do a little better
     than per-file sync." dioring holds a file's `FileKey` and its handle (`SharedFile`), no name;
     a handle identifies the volume more reliably than a name would (hard links, renames, mount
     points, junctions, redirectors). The optimizations to strongly consider are parked as
     **DI-6+.6**; coverage credit across seals is DI-2.4 point 3 and applies to every provider.
     **The engineer, 2026-10-06: "one provider per instance seems too restrictive."** Several
     providers -- a local set, an exotic device, a remote share -- are natural. Where the routing
     lives is open: (a) in dioring's core, each domain registered against a provider and one call
     per provider per seal; or (b) the assistant's preference, a routing provider dioring ships,
     implementing the provider trait over providers registered with it by domain, so the core keeps
     one provider and routing is one replaceable component. Either way: unregistered domains and
     files declared with none go to the default provider; a file whose domains span providers
     appears in each provider's call and is durable when all succeed (point 5); and coupled domains
     dedupe only within one provider, so splitting them across providers is correct but costs
     physical flushes -- documented, not enforced. **Wrinkle in (b), found while giving the
     engineer an example (2026-10-06):** the default provider alone may use dioring's wioring
     (DI-D-10), so a router that falls back to it would need ring access passed in, leaking the
     core's privilege. The clean form of (b) keeps the default built into the core: the core sends
     unregistered domains' files to its built-in default itself, and only the rest to the one
     consumer-supplied provider, which may be a router -- the core routing exactly one way.
     **Decided by the engineer, 2026-10-06: "do your clean form of b".** The core holds the
     built-in default and at most one consumer-supplied provider, which states at construction the
     flush domains it serves; a file's work for those domains goes to it, everything else --
     other domains, and files declared with none -- to the built-in default. dioring ships a
     router, itself a provider, that dispatches by domain to providers registered with it when it
     is built and refuses a domain routed twice there; it is **DI-3+.8**.
  2. [x] **One asynchronous call per seal** to the consumer's provider, and the built-in default's
     own work, for whichever of them a seal's files need (point 1). It carries, per flush domain,
     the files and the `OpId`s of the writes to make durable -- identities, not extents (the
     engineer: "drop the extents. The consumer may track them themselves via the OpId.").
     Precondition: dioring has observed every named write complete. The provider reports one
     outcome per domain. *Stance, approved 2026-10-06 ("seems right"):* one request per seal with
     an entry per domain -- its files (`FileKey` and a `SharedFile` clone) and each file's named
     `OpId`s -- and **one completion handle per domain**, consumed once with success or an error,
     so outcomes arrive independently and a router can hand each sub-provider exactly its handles.
  3. [x] **The provider flushes by any means.** The engineer: some domains flush "entirely out of
     band, as for some exotic hardware", and "The "filesystem interaction" could be entirely a
     fiction that ended up being manifested as a series of udp interactions". It never issues or
     carries writes, and never pushes to dioring's wioring ([DI-D-10](DESIGN-NOTES.md#di-d-10));
     it may use a wioring of its own -- IoRing carries flushes (`IORING_OP_FLUSH`), though not
     range flushes, device commands or network operations. Deduplicating across coupled domains,
     or across concurrent calls, is internal to the provider. **Boundary:** the provider makes
     ring-written data durable; if the writes stop being ring I/O, that is not dioring.
     *Stance:* as written; the provider may hold its `SharedFile` clones while it works, which is
     why ending an instance waits for it ([DI-D-25](DESIGN-NOTES.md#di-d-25)).
  4. [x] **Completion and waking.** The provider completes asynchronously, and the outcome enters
     dioring's core as a flush outcome scoped by DI-D-21, never through the ring's completion
     queue. A completion wakes the front end through the trait's readiness signal like any other
     entry (DI-2.5 point 4; its form is DI-2.14). Open: what an abandoned or never-completed call
     means. *Stance:* a handle dropped without completing is a failure of that domain with its own
     cause, through the ordinary failure path; a call that never completes leaves the seal pending,
     visible through DI-D-26's provider-boundary event, and ending the instance waits for it. No
     timeout in dioring: it would be policy, and a late success after a timed-out failure would
     contradict what was reported. Completing a handle records the outcome and sets the readiness
     signal, never running delivery on the provider's thread.
  5. [x] **When a write is durable.** When every domain in its file's set has reported success
     for a call that covered it. A failure from any of them makes it suspect, with the scope
     DI-D-21 defines. *Stance:* as written, refined -- since outcomes are per domain, a failure of
     domain D reaches the files whose sets contain D, plus unknown files, as
     `ImportScope::Domains([D])` does; narrower than DI-D-21's file-intersection rule, and sound
     because the provider warrants per domain. A per-file sync failure in the default provider
     keeps DI-D-21's rule.
  6. [x] **The consumer's warranty.** A domain's success means its writes are durable however the
     provider got there. It joins DI-D-21's completeness warranty in CONTRACT.md's requirements.
     *Stance:* as written; a domain's success covers exactly the `OpId`s named for that domain,
     and a false success is the consumer's defect, like an incomplete declaration.
  7. [x] **Call rules.** Non-blocking, and whether the provider may call back into dioring, in
     line with [DI-D-18](DESIGN-NOTES.md#di-d-18). *Stance:* the call returns promptly; handles
     may be completed from any thread, at any time, including inside the call; the provider gets no
     reference to the instance, so it cannot re-enter, and the handles are its only channel. A
     provider panic propagates as a defect, and handles dropped by the unwind count as abandoned.
  8. [x] **Separable lineages.** dioring makes one call per seal and merges nothing; a provider
     that couples separable lineages does so by its own choice. The default provider does not
     ([DI-D-22](DESIGN-NOTES.md#di-d-22)). *Stance:* as written; the router (DI-3+.8) merges nothing
     across calls either.
  9. [x] **Redefine "covered".** CONTRACT.md defines it physically -- dioring issued a flush that
     "cannot complete before the write has" -- which the provider no longer allows dioring to know.
     Proposed: a write is covered when named in a provider call dioring made after observing it
     complete, and covered successfully once every domain of its file reports success. One
     definition then drives the seal's file set, suspect sets (an unnamed write stays at risk until
     named), `remove_file`'s `FileBusy`, and DI-6+.7's crediting. Consequence to document on
     `remove_file`: a file stays busy while any lineage has unnamed writes to it. *Stance:* adopt
     it, as one change touching CONTRACT.md's Terms, DI-D-9(c)'s "a commit's flush happens to
     cover", the `FileBusy` documentation and DI-6+.7.

## Moved 2026-10-06 20:31:42 -04:00 -- the readiness signal

### <a id="di-214"></a>DI-2.14 -- The readiness signal: dioring's own auto-reset event, relayed from EventDelivery and set by provider completions, with the wioring's wake-up rules. *(completed 2026-10-06 20:31:42 -04:00)*

The decision is [DI-D-28](DESIGN-NOTES.md#di-d-28). Stating the wake-up rules in [CONTRACT.md](CONTRACT.md) and the signal in [API.md](API.md) is DI-2.16; the harness check is DI-3+.7. Point 5 went against the assistant's stance, and point 4's first stance is superseded in place. The item as it stood at completion follows verbatim.

- [ ] **DI-2.14** -- **The readiness signal: form and wake-up rules.** From DI-2.5 point 4: the trait
  carries a signal set whenever an entry becomes available, from any source, so Model A and Model
  B are written once over the trait. To settle, and to state in CONTRACT.md rather than leave to
  each implementation:

  1. [x] **Form.** An event handle suits this Windows-only crate and Model A's thread-pool wait;
     whether anything else is needed. *Stance, approved 2026-10-06 ("yes"):* an auto-reset event the
     implementation owns, the consumer receiving a duplicate -- the shape of the wioring's
     `IoRing::completion_event` ([D-20](../windows-ioring-sys/DESIGN-NOTES.md#d-20)).
  2. [x] **Wake-up rules.** When an implementation must set the signal relative to `pop`, and what a
     front end must do before waiting again -- the drain-to-empty-then-rearm discipline the
     wioring learned in its M4.2 -- so that no entry is left unseen. *Stance:* the wioring's own
     contract rules ([D-19](../windows-ioring-sys/DESIGN-NOTES.md#d-19),
     [D-21](../windows-ioring-sys/DESIGN-NOTES.md#d-21)), restated for the trait: an implementation
     sets the signal only after an entry is poppable, and at least on every empty-to-non-empty
     transition; a front end pops to `None` after every wake before waiting again; a wake with
     nothing to pop is normal; one waiter per instance, matching the single deliverer.
  3. [x] **Conformance.** Whether DI-2.5 point 5's oracle can check any of it, or what can only be
     stated. *Stance:* the oracle sees entries, not wakes, so it cannot; a separate harness check
     can -- push into an empty queue and assert the signal is set, drain, and assert the next push
     sets it again -- run by every implementation's tests beside the oracle.
  4. [x] **dioring's signal is its own event** (superseded stance: "the wioring's completion
     event", set by hand on provider completions). Follows from point 5: an auto-reset event dioring
     owns, set by its `EventDelivery` callback after recording each ring completion into dioring's
     core, and by each provider completion. The trait's signal keeps semantics dioring defines,
     independent of the ring's event.
  5. [x] **dioring keeps `EventDelivery` and relays into a second event.** Decided by the engineer,
     2026-10-06: "relay to a second event" -- over the assistant's stance that dioring run its own
     thread-pool wait on the ring's event. The ring's arm-and-drain discipline stays in the ring
     crate where it is already solved, [DI-D-18](DESIGN-NOTES.md#di-d-18)'s binding to
     `EventDelivery`'s callback contract stands, and ring `M31.1` stays a prerequisite of DI-3+.2.
     Implementation constraint: callbacks may overlap with no order between them (M31.1), so
     dioring's callback records under dioring's own lock and assumes no order among ring
     completions. Consequence: in both delivery models the ring is drained by `EventDelivery`'s
     pool callbacks; Model A and Model B differ only in who delivers.

## Moved 2026-10-06 20:52:36 -04:00 -- the provider and the signal written into the contract and API

### <a id="di-216"></a>DI-2.16 -- The durability provider and the readiness signal are written into CONTRACT.md and API.md, compile-checked. *(completed 2026-10-06 20:52:36 -04:00)*

The authoritative text is [CONTRACT.md](CONTRACT.md) (Terms `Durability provider` and `Covered`; guarantee 4; the non-guarantees on writes outside the seal, on what a commit waits for, and on timeouts; the provider warranty; `How a seal is made durable`; per-domain failure reach; `The readiness signal`) and [API.md](API.md) sections 15 and 16 with the sketch. Two restatements outside the item's list were corrected in the same sweep: guarantee 4's `never a silent stall` (with an amendment marker on [DI-D-13](DESIGN-NOTES.md#di-d-13)), and `No bound on what a commit waits for`, which still described the ring barrier DI-D-22 had removed. The item as it stood at completion follows verbatim.

- [ ] **DI-2.16** -- **Write the durability provider into [CONTRACT.md](CONTRACT.md) and
  [API.md](API.md)** ([DI-D-27](DESIGN-NOTES.md#di-d-27)). CONTRACT.md: redefine "Covered" in
  Terms; restate DI-D-9 rule (c)'s incidental coverage; add the per-domain failure scope beside
  DI-D-21's rule; add the provider's success warranty to the consumer's requirements; state that a
  seal with a never-completing provider call stays pending; and settle "How a seal is made durable"
  in "Not yet specified". API.md: the provider trait, the per-seal request, the per-domain
  completion handle, the abandoned-handle failure cause, how an instance is given its provider, and
  the `FileBusy` note that a file stays busy while any lineage has unnamed writes to it.
  Compile-checked like the rest of the sketch. Before or with DI-2.15, which restates the result as
  the trait. Also the readiness signal ([DI-D-28](DESIGN-NOTES.md#di-d-28)): its wake-up rules in
  CONTRACT.md, and in API.md the method that hands the consumer a duplicate of the event.

## Moved 2026-10-06 21:00:49 -04:00 -- the API restated as the trait

### <a id="di-215"></a>DI-2.15 -- API.md is restated as the DurableRing trait, a registered-buffer extension and an Identities bundle, with Dioring as dioring's implementation, compile-checked. *(completed 2026-10-06 21:00:49 -04:00)*

The authoritative text is [API.md](API.md) section 17 and its sketch. One consequence was forced rather than chosen and is recorded as a refinement on [DI-D-24](DESIGN-NOTES.md#di-d-24): `Epoch` and every shared type that carries an identity is generic over one `Identities` bundle, because an epoch holds a lineage. The readiness signal is `readiness()` rather than a placeholder, DI-2.14 having settled its form. Renaming the concrete type to `Dioring` and the signal's move to dioring's own event left markers on DI-D-15, DI-D-17 and DI-D-18. One open question surfaced and is DI-2.17. The item as it stood at completion follows verbatim.

- [ ] **DI-2.15** -- **Restate [API.md](API.md) as the trait** ([DI-D-24](DESIGN-NOTES.md#di-d-24)).
  The sketch becomes a core trait with associated identity types and a registered-buffer extension
  trait, `DurableRing` becomes dioring's implementation of both, and the prose sections that name
  concrete types are updated to match. The readiness signal is a placeholder until DI-2.14 settles
  its form. Compile-checked against the ring crate like the current sketch.

## Moved 2026-10-06 21:12:39 -04:00 -- the contract's own file type

### <a id="di-217"></a>DI-2.17 -- The contract owns its file type, DurableFile: owning, cheaply shared, lending its handle, defined in this crate. *(completed 2026-10-06 21:12:39 -04:00)*

The decision, with the list of features such a type has, is [DI-D-29](DESIGN-NOTES.md#di-d-29); the type is in [API.md](API.md)'s sketch and section 17. The engineer asked whether a canonical type existed and, assuming not, for one of the crate's own: none fits -- `std::fs::File` is not cheap to share, the ring's `SharedFile` exposes no handle (so a provider could not flush it out of band), and the namespace crate's `CapturedHandle` owns a duplicate. Handing the ring the same handle needs `windows-ioring-sys` `M31.3`, added to DI-3+.2's prerequisite. The item as it stood at completion follows verbatim.

- [ ] **DI-2.17** -- **Does the contract own its file type?** Found while restating the API as the
  trait (DI-2.15): the core trait names `windows-ioring-sys`' `SharedFile` in `add_file`,
  `remove_file` and `FileWrites`, so every implementation depends on the ring crate for that one
  type -- unlike `WriteCaching`, which the contract owns for exactly that reason (DI-2.6). Either
  accept the dependency, or give the contract its own owning file type that dioring converts at
  its boundary. Recorded in [API.md](API.md) section 17 and "Left to other items".

## Moved 2026-10-06 21:34:21 -04:00 -- ending a lineage

### <a id="di-213"></a>DI-2.13 -- A lineage its holder will not finish is ended: its unfinished epochs abandoned, its handle refused, retired once drained, nothing waited for or cancelled. *(completed 2026-10-06 21:34:21 -04:00)*

The decision is [DI-D-30](DESIGN-NOTES.md#di-d-30); the authoritative text is [CONTRACT.md](CONTRACT.md) `Ending a lineage` and [API.md](API.md) section 18 (`end_lineage`, `EndLineageError`, `Entry::LineageEnded`), compile-checked. The engineer approved the assistant's six stances together ("close it"). The item as it stood at completion follows verbatim.

- [ ] **DI-2.13** -- **A lineage its holder will not finish.** Raised with DI-2.4 point 4; the
  engineer, 2026-10-06: "We may need to implementation cancellation just in general but only because
  somewhere along the line the holder of a lineage may decide not to finish it for whatever reason."
  Working position, hedged as the engineer hedged it -- whether v1 needs it is itself open:

  1. [x] **What "not finishing" means.** Today a lineage retires only when empty, and an epoch is
     abandoned only after a failure. A holder walking away from open epochs has no operation.
  2. [x] **Its own writes.** Issued writes are on the device with no promise, which the contract
     already allows; gated writes not yet issued end as `NeverIssued { abandoned }`, which the API
     already has.
  3. [x] **Other lineages' gates on it.** The hard part: a gate in another lineage naming an epoch
     that will now never be durable can never release. Those writes must end -- refused, or reported
     never issued -- rather than wait forever, and this is the case cycle detection cannot see.
  4. [x] **Relation to cancel.** Not operation cancel, which [DI-6+.4](CHECKLIST.md) parks; this is
     cancelling a lineage's future. Whether the two share a mechanism is open.
  5. [x] **Ending a lineage does not wait.** The engineer, 2026-10-06: shutting down a lineage "does
     not have to wait because there is not a lifetime issue and there is not a dependency around
     durability" -- unlike ending an instance ([DI-D-25](DESIGN-NOTES.md#di-d-25)). Its dependants
     end rather than wait, as point 3 says.

  The engineer, 2026-10-06: "almost all of this is straightforward. What do you need answered?"
  Points 3 and 4 fall out of existing rules once ending abandons the lineage's epochs: gated writes
  on them end as `NeverIssued` and new gates on them are refused (`GateAbandoned`), per contract
  guarantee 9 and [DI-D-23](DESIGN-NOTES.md#di-d-23); and ending cancels no I/O -- operations in
  flight complete normally. Questions put to the engineer, each with the assistant's stance:

  6. [x] **In v1?** *Stance:* yes; it is small and built from existing operations.
  7. [x] **What ending abandons.** *Stance:* every epoch not yet durable, sealed-but-pending ones
     included; a flush already in flight may finish but reports nothing.
  8. [x] **Ending retires.** *Stance:* the handle is refused at once and the lineage retired once
     its in-flight operations drain, so the consumer need not retry `retire_lineage`.
  9. [x] **Failures shared with other lineages.** *Stance:* ending L resolves no failure that also
     blocks another lineage; it stays unresolved for them, and L's writes stop counting.
  10. [x] **Reporting.** *Stance:* a new queue entry, `LineageEnded { lineage, abandoned_through }`,
      delivered in order; `Abandoned { failure, .. }` only for a failure that belonged to L alone.
  11. [x] **The default lineage.** *Stance:* cannot be ended, as it cannot be retired; a consumer
      walking away from it ends the instance.

## Moved 2026-10-06 21:34:21 -04:00 -- DI-M2, the API shape over `windows-ioring-sys`, complete

Every item below was already archived above under its own anchor; these are the milestone's stubs as they stood at completion.

- [x] **DI-2.1** -- The tag and completion-queue types are API.md, generic over the epoch type. -> [completed 2026-10-05](COMPLETED-CHECKLIST.md#di-21)

- [x] **DI-2.2** -- dioring's composition with `windows-ioring-sys` is API.md sections 7-10. -> [completed 2026-10-05](COMPLETED-CHECKLIST.md#di-22)

- [x] **DI-2.3** -- Completion routing: an I/O-free core with a front end per model, delivering one entry at a time in order. -> [completed 2026-10-05](COMPLETED-CHECKLIST.md#di-23)

- [x] **DI-2.9** -- Durability lineages: an epoch is a lineage plus an epoch id, and lineages are minted, enumerable and retirable. -> [completed 2026-10-05](COMPLETED-CHECKLIST.md#di-29)

- [x] **DI-2.10** -- Caller-declared flush domains: a set of opaque byte keys per file, fixed at registration, with failures reaching files whose sets intersect. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-210)

- [x] **DI-2.4** -- Multi-file seals: completion-gated flushes only, gate cycles detected, and the seal's file set fixed when it is requested. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-24)

- [x] **DI-2.5** -- The contract is a trait: identity types associated, generic only, a registered-buffer extension trait, a readiness signal, and a conformance oracle. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-25)

- [x] **DI-2.6** -- The operation set: no consumer flush, per-write write-through passed through, no per-operation cancel, and a completion fence parked. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-26)

- [x] **DI-2.7** -- Ending an instance seals nothing, waits for kernel operations and provider calls, and close() hands back what is left. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-27)

- [x] **DI-2.11** -- Delay events: one manifest ETW provider owned by dioring, for locks and flushes, costing nothing when nobody listens. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-211)

- [x] **DI-2.12** -- The durability provider: a built-in default plus one consumer provider, per-domain completion handles, no timeout, and per-domain failure scope. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-212)

- [x] **DI-2.14** -- The readiness signal: dioring's own auto-reset event, relayed from EventDelivery and set by provider completions, with the wioring's wake-up rules. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-214)

- [x] **DI-2.16** -- The durability provider and the readiness signal are written into CONTRACT.md and API.md, compile-checked. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-216)

- [x] **DI-2.15** -- API.md is restated as the DurableRing trait, a registered-buffer extension and an Identities bundle, with Dioring as dioring's implementation, compile-checked. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-215)

- [x] **DI-2.17** -- The contract owns its file type, DurableFile: owning, cheaply shared, lending its handle, defined in this crate. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-217)

- [x] **DI-2.13** -- A lineage its holder will not finish is ended: its unfinished epochs abandoned, its handle refused, retired once drained, nothing waited for or cancelled. -> [completed 2026-10-06](COMPLETED-CHECKLIST.md#di-213)

- [x] **DI-2.8** -- An epoch number's scope is one instance, with one author per epoch space. -> [completed 2026-10-05](COMPLETED-CHECKLIST.md#di-28)
