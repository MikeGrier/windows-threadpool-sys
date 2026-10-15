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

## Moved 2026-10-07 16:24:03 -04:00 -- DI-3.1: the crate is scaffolded

### <a id="di-31"></a>DI-3.1 -- The crate is scaffolded: a workspace member with `#![forbid(unsafe_code)]`, registered for release and publication. *(completed 2026-10-07 16:24:03 -04:00)*

[Cargo.toml](Cargo.toml) inherits the workspace's edition and MSRV, depends on
`windows-ioring-sys` by path only ([DI-D-16](DESIGN-NOTES.md#di-d-16)) and on
`win-shared-os-owned-handle` by version and path ([DI-D-29](DESIGN-NOTES.md#di-d-29)), and uses
[COMPONENT.md](COMPONENT.md) as its readme. [src/lib.rs](src/lib.rs) forbids unsafe code
([DI-D-4](DESIGN-NOTES.md#di-d-4)) and holds nothing else yet. Registered in the workspace
members, `release-please-config.json`, the manifest at `0.0.0`, and the publish workflow.

DI-D-16's claim that a path-only dependency stops a publish was checked, not assumed:
`cargo publish --dry-run` refuses with "all dependencies must have a version requirement
specified when publishing", naming `windows-ioring-sys`. So dioring's first release cannot
publish until `DI-3.5` pins that version.

## Moved 2026-10-07 17:05:12 -04:00 -- DI-3.2.1: types, trait and construction

### <a id="di-321"></a>DI-3.2.1 -- Types, trait and construction: the shared types, identities and trait in code, and `Dioring::new` with its refusals. *(completed 2026-10-07 17:05:12 -04:00)*

The shared types, `DioringIds` and its identities, and `DurableRing` are code in
[src/](src/lib.rs). The trait carries what this step implements -- its associated types,
`default_lineage` and `lineages` -- and grows with each later step. `DomainCompletion` exists so the
provider trait can be named, and gains its answers in `DI-3.2.6`. `Dioring::new` refuses a duplicate
key and a provider domain named twice before creating anything, and a ring that cannot be created
or cannot register; every refusal hands back what it was given. The crate's documentation is
[CONTRACT.md](CONTRACT.md).

**API.md's sketch** is marked superseded by the code for what the code carries, and kept frozen as
the design of what is not yet built; the code wins where they differ.

Two findings. The kernel accepts queue sizes of zero, so ring creation is refused here with
`u32::MAX` instead. And no input tried makes a registration fail at completion -- a thread handle
and a zero-length buffer both registered -- so those error paths are unreached by tests;
[dioring.rs](src/dioring.rs) says what would reach them, and [sabotage.json](sabotage.json) records
it. Six sabotages, each caught, and a control that survives.
## Moved 2026-10-07 19:18:33 -04:00 -- DI-3.2.2.1: plain I/O and the shared delivery

### <a id="di-3221"></a>DI-3.2.2.1 -- Plain I/O and the delivery every front end shares: pushes, `pop`, the readiness `Event`, and the conformance oracle with its readiness check. *(completed 2026-10-07 19:18:33 -04:00)*

**What landed.** The trait grew by `readiness`, `add_file`/`add_file_with`, `write`/`write_with`,
`read`/`read_with` and `pop`, and the extension trait `RegisteredBufferRing` by the registered-span
forms and `registered_buffer`/`registered_buffer_mut`. In `Dioring`, the ring goes to
`EventDelivery` after construction's registrations, and its callback records each completion into
`Relay` -- dioring's queue under dioring's own lock -- setting the readiness event on every
empty-to-non-empty transition. `pop` serves that queue. Pushes run through one path (`push.rs`):
the file, then the epoch's lineage, then the registration a span operation needs are checked before
anything is reserved, and every refusal, the ring's included, hands back the buffer and the
context. Only an accepted push spends an `OpId`. A submission that fails after a push was accepted
leaves the entry queued, so the push stands and the next push or pop submits again, `pop`
reporting the error only when it has no entry to return.

**The conformance oracle** is `durable_ioring::oracle`: `ConformanceOracle` checks one completion
per operation, with the kind and context it was pushed with, and lists what it deliberately does not
check; `check_readiness` is DI-D-28's harness check, waiting on the signal through a thread-pool
wait. Each rule is tested in both directions, and the readiness check is shown to fail against a fake
that never sets the signal and one that sets it once.

**Decided while implementing, each recorded where it binds:**

- `readiness()` returns a `win-sync-sys` `Event`, not an `OwnedHandle`: the readiness check cannot
  wait on a bare handle without `unsafe`, and neither could a Model A front end. DI-D-28 carries the
  amendment, and CONTRACT.md's readiness section names the type.
- The gate setters on `WriteOptions` and `ReadOptions` are crate-private until gates are honoured,
  so a gated operation cannot be issued ungated; `DI-3.2.5` makes them public again.
- `B`, `C` and the epoch-id type must be `Send`, and the epoch-id type `Sync` too, because they cross
  to pool threads and a failure's suspect set is shared.
- `IoBuf`, `IoBufMut`, `RegisteredSpan`, `SharedHandle` and `Event` are re-exported, so a consumer
  calls the contract with no direct dependency on the crates it is built on.
- `win-sync-sys` and `windows-threadpool-sys` are path-only dependencies; `DI-3.5` pins them.
- "Writes are tagged and recorded": the epoch is checked against the instance's lineages and travels
  in the operation's sidecar, so its completion carries it. Coverage state is `DI-3.2.3`'s.

**Found:** `IoRing`'s write-through flag fails with Win32 error 509 on a handle opened for cached
I/O, including one opened with `FILE_FLAG_WRITE_THROUGH`, and succeeds only on an unbuffered handle.
The ring crate's documentation does not say so; that is queued there. dioring's `WriteCaching`
rustdoc records the measurement, and `the_caching_choice_reaches_the_kernel` uses it to show the
flag is passed through.

**Verification.** The crate's tests pass; a harness run recorded all 26 entries of
[sabotage.json](sabotage.json) as declared, the two readiness sabotages caught by the readiness
check alone.

The item as it stood at completion:

- [x] **DI-3.2.2.1** -- **Plain I/O, and the delivery every front end shares.** Re-planned
  2026-10-07 from `DI-3.2.2`, which also held the Model A front end (now `DI-3.2.2.2`). `write`,
  `read` and their `_with` and
  registered-span forms, `add_file` and `add_file_with`, with the consumer's context passed through
  to each `OpCompletion` and every refusal handing back what it took (`UnknownFile`,
  `NoRegisteredBuffers`, `Ring`). The delivery: `EventDelivery`'s callback moves ring completions
  into dioring's queue, `pop` serves it, and the readiness signal is set as
  [DI-D-28](DESIGN-NOTES.md#di-d-28) states. Writes are tagged and recorded, but nothing is sealed
  yet. **The conformance oracle starts here**, with this step's rules -- one completion per
  operation, its context handed back -- and beside it the readiness signal's harness check, which
  the oracle cannot make because it sees entries, not wakes: push into an empty queue and assert
  the signal is set; drain; assert the next push sets it again. **Constraint from [DI-D-18](DESIGN-NOTES.md#di-d-18):** the front end binds only to
  `EventDelivery`'s specified callback contract -- called with the ring lock released, and possibly
  concurrently -- never to how its delivery loop happens to work. **What it relies on from the
  ring's completions, already specified:** `try_pop` hands an operation's payload back by that call
  and no other, and every operation completes exactly once with the identity it was built with, a
  completion for anything not in flight being a defect that panics
  ([D-79](../windows-ioring-sys/DESIGN-NOTES.md#d-79)).

## Moved 2026-10-07 20:14:18 -04:00 -- DI-3.2.2.2: the Model A front end

### <a id="di-3222"></a>DI-3.2.2.2 -- The Model A front end: `EntryDelivery` and its `DeliveryHandle`, delivering entries one at a time to a handler on pool threads. *(completed 2026-10-07 20:14:18 -04:00)*

The decisions are [DI-D-31](DESIGN-NOTES.md#di-d-31) (the engineer's: name and constructor) and
[DI-D-32](DESIGN-NOTES.md#di-d-32) (the assistant's, on analytic grounds, for the engineer to
confirm: mirrored `&self` methods rather than a closure over the instance, because the ring crate's
D-43 measured a closure over `&mut` silently stopping delivery; and ending by quiescing first, with
`into_inner` handing the instance back). `EntryOf<D>: Send`, proposed in DI-D-31, proved
unnecessary and is noted there.

**Tests** (`delivery/tests.rs`): the handover backlog delivered in queue order after its wake was
consumed; a failed setup handing back the instance and the handler; every push through the owner
delivered once, under the oracle; the handler pushing through its handle; four threads pushing
through `&self` while no two handler calls overlap; `into_inner` losing nothing and leaving a working
instance; drop with operations in flight returning; refusals through the handle; registered bytes
and spans through the handle; and the lineages it reports. The oracle tests' fake moved to
`src/fake.rs` so both suites use it, gaining a failing `readiness` and contexts that number its
entries.

**Verification.** Three sabotages, each caught by the test its `why` names: no signal at the
handover, a wake that stops after one entry, and the handler run with the instance's lock held --
the push-from-the-handler test leaks the front end on failure, so that one fails at its bound rather
than hanging. The test binary ran 30 times without a failure.

The item as it stood at completion:

- [x] **DI-3.2.2.2** -- **The Model A front end.** Split from `DI-3.2.2` on 2026-10-07: DI-2.3 decided
  its behaviour, but [API.md](API.md) never sketched its API, so it needs design before code. What
  is decided ([DI-D-18](DESIGN-NOTES.md#di-d-18), and DI-2.3's points 1-3 and 8 in
  [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#di-23)): a type written once over the trait
  that owns the delivery, with `&self` methods usable from any thread; it hands entries strictly
  one at a time, in queue order, to a consumer callback on a pool thread, with no lock held, so the
  callback may call back in; dioring's lock is taken before the ring's. It waits on `readiness()`
  through a thread-pool wait. **Name and constructor decided** ([DI-D-31](DESIGN-NOTES.md#di-d-31),
  2026-10-07): `EntryDelivery<D>`, built by `EntryDelivery::new(ring, on_entry, env)`, the handler
  `FnMut(EntryOf<D>, &DeliveryHandle<D>)`, a failure handing back the ring and the handler as
  `DeliverySetupError`; entries queued before the handover are delivered, which `new` guarantees by
  arming and then setting the readiness event, tested from the start. **Open, for the engineer:**
  which of the trait's `&mut self` operations the owner and `DeliveryHandle` offer as `&self`, and
  how (mirrored methods, or a closure over the instance under its lock); and its teardown, which
  DI-2.7 point 5 orders -- delivery quiesced before the state it reaches is released.

## Moved 2026-10-07 20:38:53 -04:00 -- DI-3.2.3: seals and durability in one lineage

### <a id="di-323"></a>DI-3.2.3 -- Seals and durability in one lineage, through the built-in default provider: `make_durable_through`, `durable_through`, `sealed_through` and `epoch_state`, with `Durable` reported as a prefix after the completions it covers. *(completed 2026-10-07 20:38:53 -04:00)*

How it is built is [DI-D-33](DESIGN-NOTES.md#di-d-33) (the assistant's, implementing DI-D-9 and
DI-D-22, for the engineer to confirm): one lineage's durability is an I/O-free state machine,
`dioring/durability.rs`, held under dioring's lock with the queue; the delivery callback records
each completion there and pushes the flushes it makes due while still holding it. Two cases are
left pending rather than approximated, each already the work of a later step: a flush that fails
(`DI-3.2.4`) and a write whose file a consumer provider serves (`DI-3.2.6`). What `epoch_state`
answers for another instance's lineage is left open for the engineer and queued under `DI-3.2.5`.

**Tests.** The durability core's rules without I/O (`dioring/durability/tests.rs`). Against real
files (`dioring/tests/seals.rs`): Durable after completed writes, with the four queries and
`lineages()` agreeing; a seal with nothing written; a seal made while sixteen writes are in flight,
reported after all of them; two seals reported in order; late writes refused as `Sealed` with buffer
and context returned, owned and registered, while later epochs and reads are accepted;
`AlreadySealed` answering `Durable`, and `Pending` for a seal a provider-routed write holds -- which
also holds back a later seal with nothing in it; a failed write neither covered nor holding its seal
(guarantee 1); and another instance's lineage refused. Through Model A: a seal made from inside the
handler, its `Durable` delivered to that handler. **The oracle gained this step's rules**: no write
accepted at or below its lineage's seal, `Durable` only for what was sealed, and only after the
completion of every write it covers -- each tested in both directions.

**Verification.** Thirteen new sabotages, each caught; the prefix-order and provider-hold ones by
the I/O test as well as the unit tests. One was first caught only by a crash: a write accepted past
the seal tripped the core's debug assertion after the ring held the push, and the unwind through an
unsubmitted batch aborted the process. The same check now also runs in `issue` before anything
reaches the ring, so it fails as an ordinary assertion in the test named for it. The relay's six
existing sabotages were re-anchored after its rewrite. The test binary ran 30 times without a
failure.

The item as it stood at completion:

- [x] **DI-3.2.3** -- **Seals and durability in one lineage, through the built-in default
  provider.** `make_durable_through`, `durable_through`, `sealed_through` and `epoch_state`, in the
  default lineage. Coverage as CONTRACT.md defines it: a write is named to the default provider
  only after it is observed complete, and the default flushes each of its files through the
  instance's ring. Guarantees 1, 2, 3, 5, 6 and 7: `Durable` as a prefix, after the completions it
  covers, never retracted; a late write refused as `Sealed`; asking again answered by
  `AlreadySealed` with the epoch's state.

## Moved 2026-10-07 21:17:47 -04:00 -- DI-3.2.4: failures and their resolution

### <a id="di-324"></a>DI-3.2.4 -- Failures and their resolution: `Failed`, heal, abandon and close, `resolve`, `Blocked`, `import_failure`, the inventory, and a fault seam that drives CONTRACT.md's worked examples as doctests. *(completed 2026-10-07 21:17:47 -04:00)*

How it is built is [DI-D-34](DESIGN-NOTES.md#di-d-34) (the assistant's, for the engineer to
confirm). The trait gained `resolve`, `import_failure`, `failures` and `take_token`, mirrored on
`DeliveryHandle` ([DI-D-32](DESIGN-NOTES.md#di-d-32)); `FailureToken` gained its liveness flag,
`close`, and a drop that closes. Flush-domain reach for flush failures and imports was built here
rather than in `DI-3.2.5`, because the suspect set is defined here; `DI-3.2.5` and `DI-3.2.6` were
re-worded to say what remains for each.

**The fault seam.** `windows-ioring-sys`' `fault-injection` feature can fail a flush:
`Completion::with_injected_failure` transforms a real completion, and dioring's callback receives
exactly that `Completion`. So dioring's seam builds on it -- `Dioring::fail_next_flush(file, code)`
behind a `fault-injection` feature of its own -- and `DI-3.3` can build on that in turn. A self
dev-dependency enables it for every test build, which is what lets the doctests use it.

**Tests.** The durability core, without I/O (`dioring/durability/tests/failures.rs`): what a failed
flush suspects, in push order and including writes in flight and above the seal; freezing; finality;
reach by domain, for flushes and every import scope; a failure holding only its own epochs;
`Blocked`, including for a seal finished before the failure arrived; both of CONTRACT.md's traces;
a heal made after a seal not taking effect at it, and one waiting past a seal that failed; abandon
at once, including an open epoch; a refused resolution; and the token's liveness. Against real files
through the seam (`dioring/tests/failures.rs`): the cause and the error passed through, healing,
`Blocked` then abandonment, domain reach, every import scope, a foreign token refused, a closed
failure's token taken again, and an armed failure used only by its own file. Through Model A: a
handler abandoning a failure through its handle. **The oracle gained this step's rules**: `Durable`
never passes a failure neither healed nor abandoned; a failure's identity is new and its suspects
are accepted writes with their epochs, in push order, none already durable; and `Abandoned` and
`Blocked` name a live failure -- each tested in both directions, with `ConformanceOracle::healed` to
report a heal. **CONTRACT.md's worked examples** are now `rust` blocks, compiled and run as
doctests; the many-to-many example's second ending was corrected to match the contract's own rule.

**Verification.** Sixteen new sabotages, each caught, and the DI-3.2.3 entries the rewrite moved
re-anchored. Two existing sabotages turned out to be caught only by a process abort -- a debug
assertion firing in the delivery callback -- so the core now records an inconsistency and `pop`
asserts on it, and both fail as ordinary tests. The manifest now runs `--no-fail-fast`, so the
doctests run under every sabotage: three sabotages fail them, which is the evidence they are live.
The test binary ran 30 times without a failure.

The item as it stood at completion:

- [x] **DI-3.2.4** -- **Failures and their resolution.** A failed flush recorded as a failure whose
  suspect set is frozen at observation, by push order ([DI-D-12](DESIGN-NOTES.md#di-d-12));
  `Failed` with its identity and affine token; `take_token`, and a dropped token as `close`;
  heal (effective at the first seal after it), abandon (`Abandoned`) and close; `resolve`, validated
  whole and applied atomically, handing its tokens back when refused; `Blocked`; `import_failure`;
  and the inventory, `failures()`. **Constraint from DI-1.2 Q4:** "a failure is observed" is one
  transition, reachable at any drain point, with its cause (flush, import) as a field -- no
  import-specific state or edge. Prefer enforcing it in the types (the cause is data on one variant,
  not a variant of its own) over a test that could be skipped. **A fault seam lands here**, because
  this step's tests need flush failures on demand and so do CONTRACT.md's worked examples: the
  healing trace and the many-to-many failures become compiled doctests driven by it, so a
  behaviour change that invalidates them breaks the build. The seam must therefore be reachable
  from doctests, not only from `cfg(test)`. Check first whether `windows-ioring-sys`'
  `fault-injection` feature can fail a flush; `DI-3.3` may build on the seam rather than beside it.

## Moved 2026-10-07 21:24:28 -04:00 -- DI-3.2.4.1: writes into abandoned epochs refused

### <a id="di-3241"></a>DI-3.2.4.1 -- A write tagged with an abandoned epoch is refused as `EpochAbandoned`, open or sealed, per the engineer's decision DI-D-35. *(completed 2026-10-07 21:24:28 -04:00)*

The decision is [DI-D-35](DESIGN-NOTES.md#di-d-35), the engineer's answer to the question
`DI-3.2.4` left open under `DI-3.2.5`. Added on 2026-10-07 as a follow-on to `DI-3.2.4` and done in
the same session, because the question arose there and the change is the push path's alone.
`PushRefusal::EpochAbandoned { epoch }` is checked after `Sealed`, so a sealed abandoned epoch is
still refused as `Sealed`; CONTRACT.md's guarantee 6 and its resolution section, and API.md's
refusal list and sketch, say so. The oracle gained the rule, applied once the `Abandoned` entry has
been observed.

**Tests:** the core reporting which epochs are abandoned; the oracle in both directions, including a
write into the epoch before its `Abandoned` entry is seen and another lineage's same-numbered epoch;
and, through the ring, owned and registered writes into an abandoned open epoch refused with their
buffer and context, a write into the sealed abandoned epoch still refused as `Sealed`, and a later
epoch accepted. **Verification:** two sabotages -- the refusal removed, and the oracle's rule
removed -- each caught by the test named for it.

## Moved 2026-10-08 10:41:11 -04:00 -- DI-3.2.4.2: markings and a failure's final record

### <a id="di-3242"></a>DI-3.2.4.2 -- Markings: a failure records what happens to its suspect writes afterwards -- nullified, short, or covered -- reports each as `Marked`, and ends with a final record, `Abandoned` or the new `Healed`; every failure carries its error code, read with `ErrorCode::of`. *(completed 2026-10-08 10:41:11 -04:00)*

The shapes are [DI-D-39](DESIGN-NOTES.md#di-d-39), decided while implementing and awaiting the
engineer's confirmation; the direction is [DI-D-36](DESIGN-NOTES.md#di-d-36).

- **Types.** `Marking { write, observed, kind }`; `MarkingKind`, non-exhaustive: `Nullified { code }`,
  `Short { transferred }`, `Covered`. `ErrorCode`, an `HRESULT` newtype in its own module, with
  `of(&io::Error)`, `from_win32`, `from_hresult`, `hresult` and `win32`. `Entry::Marked`, and
  `Entry::Healed { failure, suspect, markings }`; `Entry::Abandoned` and `FailureInfo` gain
  `markings`. All re-exported, with `TimeBase`, which WT-2.2.2 had left unexported although it
  bounds `Dioring::with_clock`.
- **Core.** The core records each write's requested length at push, through a new `Accepted`
  value. A suspect write completing failed or short marks every unresolved failure suspecting it,
  and one whose own file's flush succeeds is marked covered, all at one clock reading. A heal taking
  effect emits `Healed` before any `Durable` it lets through. The relay maps a completion's outcome
  to how the write ended (`WriteEnd::of`).
- **Oracle.** Each marking is checked against its write's completion; one stamp order covers
  `Failed` and `Marked`; a final record must carry exactly the markings reported; `Healed` is
  accepted once and only for a failure the consumer healed. With `Healed` in the stream, a
  `Durable` passing a failure must now follow that failure's `Healed` or `Abandoned` entry, which
  removed "when a heal takes effect" from what the oracle leaves unchecked.
- **Contract.** A suspect set "never gains a write" rather than "never grows"; a new section on
  markings and error codes; the queue table gains `Marked` and `Healed`; the inventory reports
  markings; a heal is reported by `Healed`. DI-D-13 is marked amended for the two entries.

**One correction to the item as written.** Its oracle rule said a covered marking names a write "in a
seal that later succeeded". Coverage is per file: a seal that fails on file A has still committed
file B, whose writes are on the device. So a write is covered when its own file's flush succeeds,
and `a_flush_failure_on_one_file_leaves_the_writes_another_file_flushed_covered` pins that down.

**Tests.** Core: a nullifier with its code; a write failed before observation neither suspected nor
marked; a full completion unmarked; short, including zero bytes, staying held; covered, and still
held; covered per file; one write marking two failures at one reading; abandoning carrying the
record and ending it; a heal's `Healed` record before its `Durable`s, with markings gained while
healing; the inventory as markings accrue; and `WriteEnd::of`. `ErrorCode`'s arithmetic and reading.
Oracle: markings agreeing with completions accepted, contradicting ones and early ones refused; a
marking for a write not suspected, or a failure not live, refused; one stamp order; final records
exact; `Healed` once and only when healed. Through the ring: an import's suspect write covered and
abandoned with its marking; a heal's record carrying a covered marking; a flush failure's code read
with `ErrorCode::of`. Three existing heal tests changed to expect `Healed`, and the covered
markings a partly failed seal now produces. The test binary ran 30 times in sequence without a
failure.

**Not reachable through the ring:** a nullifier or short marking needs the suspect write in flight
when the failure is observed, which no test can hold it to; the core's tests cover both, and the
manifest's `notCoveredHere` names the requested-length wiring this leaves unchecked.

**Verification.** Thirteen sabotages, each caught: no nullifier; a full completion marked short; no
short marking; coverage per seal rather than per file; only the first failure marked; only the first
suspect found; no `Healed`; an inventory without markings; a lost code; the oracle ignoring codes;
the oracle letting `Durable` pass a healed failure before `Healed`; the oracle ignoring a final
record's markings; a Win32 code unmasked. That last one first survived, because the test's extra
bit fell inside the facility field; the test now uses a bit outside it. A control, reordering the
covered test's conditions, survives.

The item as it stood at completion:

- [x] **DI-3.2.4.2** -- **Nullifiers** ([DI-D-36](DESIGN-NOTES.md#di-d-36), the engineer's direction):
  a suspect write that completes as failed after its failure was observed gains an append-only
  marking in that failure's record, carrying the completion's error and when it was observed, so
  the history can be reconciled after the fact. Reporting only: the mark, what holds it, and what
  resolution needs are unchanged. Amends CONTRACT.md's "frozen ... and never grows" to "never gains
  a write", and DI-D-12 (b). Everything DI-D-36 allows for is built here: markings in the inventory
  and the `Abandoned` entry, and as queue entries of their own; a marking for a short write, with its
  transferred count; a marking type open to further kinds, starting with a suspect write later
  covered by a successful flush; and a failure's final record surviving its resolution, including a
  heal, which needs an entry of its own. A `Failed` entry's own observation time lands first, with
  the dependency, under win-time-sys' `WT-2.2.2`; every
  failure, synchronous or asynchronous, carries its error code -- a failed completion's and a failed
  flush's alike -- in a representation chosen here, which also settles how a consumer reads the code
  without `windows-ioring-sys`' `IoRingErrorExt` (DI-D-34's question). Timestamps are interrupt time
  ([DI-D-37](DESIGN-NOTES.md#di-d-37)). The oracle gains the rules: a nullifier names a write in the
  set whose completion reported `Failed`, a short-write marking one whose completion was short, and
  a covered marking one in a seal that later succeeded.

## Moved 2026-10-08 13:28:56 -04:00 -- DI-3.2.5.1: lineage handles and failures across lineages

### <a id="di-3251"></a>DI-3.2.5.1 -- Lineage handles, minting, ending and retiring, and failures across lineages, with a heal taking effect in each lineage separately. *(completed 2026-10-08 13:28:56 -04:00)*

How it was built is [DI-D-42](DESIGN-NOTES.md#di-d-42). The item as written:

**Lineage handles, minting, ending, and failures across lineages.**
  `mint_lineage`, `lineages()` and `default_lineage()`, with every rule of the steps above holding
  per lineage ([DI-D-19](DESIGN-NOTES.md#di-d-19)); ending and retiring
  ([DI-D-30](DESIGN-NOTES.md#di-d-30)). **Lineage handles** ([DI-D-40](DESIGN-NOTES.md#di-d-40)):
  a `Clone` handle over an `Arc`, `#[must_use]`, an associated type of the contract beside
  `FailureToken`, reaching dioring through a `Weak`. `Dioring::new` and `mint_lineage` return one;
  the instance keeps a reference to the default's and hands out clones. Releasing the last handle
  ends the lineage; `end` and `retire` take the handle by value and succeed only when it is the
  last. Pushing and sealing take the handle, with another instance's refused. Naming or asking
  takes the plain value and answers "not this instance's" explicitly: `epoch_state` stops
  panicking, `durable_through` and `sealed_through` tell "nothing yet" from "not this
  instance's", and `import_failure` refuses a lineage the instance does not have. **Failures
  become the instance's** rather than the default lineage's: a suspect set spans every lineage's
  held writes, a failure belongs to each lineage it suspects a write of, and `ImportScope::Lineage`
  narrows to that lineage's writes. These land together with minting: once a second lineage exists,
  a failed flush must suspect every lineage's writes to that file, or it answers wrongly in between.
  (Flush-domain reach for flush failures and imports landed in `DI-3.2.4`, where the suspect set is
  defined -- [DI-D-34](DESIGN-NOTES.md#di-d-34); containment for a provider's domain is
  `DI-3.2.6`'s.) The fake implementation and the oracle follow the trait, and CONTRACT.md states
  the handle, its release, and the rule. **A heal takes effect per lineage**
  ([DI-D-41](DESIGN-NOTES.md#di-d-41)): a healed failure stops holding lineage L once L's first
  seal made after the heal succeeds, and is resolved as `Healed` only when that has happened in
  every lineage it belongs to; CONTRACT.md's "Resolving a failure" says so.

## Moved 2026-10-08 14:46:18 -04:00 -- DI-3.2.5.2: gates and gate cycles

### <a id="di-3252"></a>DI-3.2.5.2 -- Gates: operations held until an epoch is durable, ended as never issued when an epoch they need is abandoned, and a push or a seal that would close a gate cycle refused. *(completed 2026-10-08 14:46:18 -04:00)*

The engineer's decision to refuse a seal that would close a cycle, and how the search was built, are [DI-D-43](DESIGN-NOTES.md#di-d-43). The item as written:

**Gates.** Gated operations held until their epoch is durable, ending as
  `NeverIssued` when it is abandoned; a gate on an abandoned epoch refused as `GateAbandoned`; and a
  gate cycle refused as `GateCycle`, before anything is reserved, with an error naming the cycle
  ([DI-D-23](DESIGN-NOTES.md#di-d-23)). A gate takes the plain `Epoch`, which may name another
  lineage, and a gate on a lineage the instance does not have is refused
  ([DI-D-40](DESIGN-NOTES.md#di-d-40)). The cycle search must follow every way reaching an epoch
  can depend on a later one: DI-D-23 names a pending heal, and once heals take effect per lineage
  (`DI-3.2.5.1`), reaching L:a can depend on L's next seal and on any gated write in it. Make
  `WriteOptions::gate` and `ReadOptions::gate` public again: `DI-3.2.2.1` made them crate-private,
  because until gates are honoured a public setter would let a gated operation be issued ungated.
  CONTRACT.md states the gate rules, `GateCycle` included, which it does not yet mention. Gates on
  the writer's own lineage need nothing from `DI-3.2.5.1`; gates across lineages do, so this follows
  it. From `DI-3.2.5.1` ([DI-D-42](DESIGN-NOTES.md#di-d-42)): a gated operation on an epoch of a
  lineage that ends, in any lineage, ends as `NeverIssued` ([DI-D-30](DESIGN-NOTES.md#di-d-30)),
  and `LineageBusy::held_for_gate`, zero until now, counts the operations held for a gate in the
  lineage or gated on one of its epochs.

## Moved 2026-10-08 16:04:04 -04:00 -- DI-3.2.6: the consumer's durability provider

### <a id="di-326"></a>DI-3.2.6 -- The consumer's durability provider: one request per seal, called from the consumer's next call with the lock released, answered once per domain from any thread, its failures and its silence reported with their own causes. *(completed 2026-10-08 16:04:04 -04:00)*

When and how the provider is called, and the shapes chosen while implementing it, are [DI-D-44](DESIGN-NOTES.md#di-d-44). The item as written:

**The consumer's durability provider.** `Setup::provider` and the domains it
  serves; one `FlushRequest` per seal, naming files and `OpId`s per domain; `DomainCompletion`
  answered once from any thread, with a failure as `Cause::Provider` and a completion dropped
  unanswered as `Cause::ProviderAbandoned`; the readiness signal set by provider answers
  ([DI-D-27](DESIGN-NOTES.md#di-d-27)). A write is durable when every domain of its file has
  succeeded. A provider's failure of domain D reaches the held writes of every file whose declared
  domains contain D, and every file declared with none ([DI-D-21](DESIGN-NOTES.md#di-d-21)); it
  is observed through the same transition as a failed flush, with its own `Reach`.

## Moved 2026-10-08 17:25:40 -04:00 -- DI-3.2.7: ending

### <a id="di-327"></a>DI-3.2.7 -- Ending: `remove_file` refused with every hold reported at once, and `close()` and drop waiting for the kernel and the provider, `close()` handing back what is left. *(completed 2026-10-08 17:25:40 -04:00)*

How the refusal and the ending path are built is [DI-D-45](DESIGN-NOTES.md#di-d-45); its review is `DI-3.2.8`. `retire_lineage`, `LineageBusy`, `end_lineage` and `LineageEnded` had already landed under `DI-3.2.5.1`. The item as written:

**Ending.** `remove_file` refused while the file has anything in flight, held
  or uncovered, reported whole in `FileBusy`; `retire_lineage` and `LineageBusy`; `end_lineage` and
  `LineageEnded` ([DI-D-30](DESIGN-NOTES.md#di-d-30)); and `close` and drop, waiting for every
  kernel operation and provider call in flight and returning `Leftovers`
  ([DI-D-25](DESIGN-NOTES.md#di-d-25)), with no callback after the end and tracing rather than
  panicking while unwinding.

## Moved 2026-10-08 18:15:04 -04:00 -- DI-3.2.8 and DI-3.2: dioring over windows-ioring-sys

### <a id="di-328"></a>DI-3.2.8 -- The engineer confirmed [DI-D-45](DESIGN-NOTES.md#di-d-45) as built. *(completed 2026-10-08 18:15:04 -04:00)*

"di-3.2.8: confirmed!" -- no point was amended. The item as written:

**The engineer confirms or amends
  [DI-D-45](DESIGN-NOTES.md#di-d-45)**, the shapes the assistant chose while implementing
  `DI-3.2.7`: `FileBusy.in_flight` clearing at the kernel's completion; a suspect write, covered
  or not, holding its file through its failure until the failure is resolved; the provider dropped
  before ending waits; completions recorded after the end discarded unreported; and the `trace`
  feature. A rejected point is reworked here, with its decision row amended.

### <a id="di-32"></a>DI-3.2 -- dioring over `windows-ioring-sys`, implementing the contract in steps `DI-3.2.1` to `DI-3.2.8`, from construction through ending. *(completed 2026-10-08 18:15:04 -04:00)*

Each step is archived above under its own anchor. The item as written:

**dioring over `windows-ioring-sys`**, implementing the contract in
  [CONTRACT.md](CONTRACT.md), in the steps `DI-3.2.1` to `DI-3.2.8` below; checked when the last of
  them is. Re-planned 2026-10-07: as one item it covered the whole contract, and two of its
  requirements sat on items numbered after it. Each step lands what it implements, with its tests,
  and adds to the trait only the methods it implements -- the crate is unreleased, so the trait
  grows rather than shipping `todo!()` bodies. Two dependencies on later items were settled by the
  engineer on 2026-10-07, each by bringing the work forward:
  - **The worked-example doctests** land in `DI-3.2.4`, on a fault seam built there, rather than
    waiting for the fault-injecting implementation, `DI-3.3`.
  - **The conformance oracle** ([DI-D-24](DESIGN-NOTES.md#di-d-24)), formerly `DI-3.7`, is folded
    into these steps: `DI-3.2.2.1` builds it, and each later step adds the rules it implements, so
    dioring's own tests bind to it from the start. It is a reusable checker over the event stream,
    owned by dioring, that every implementation of the trait runs in its tests -- dioring's own,
    the fault-injecting one (`DI-3.3`), and layers above. Modelled on `windows-file-watcher`'s
    `ContractChecker`: it accepts the legal-but-surprising sequences as carefully as it rejects the
    illegal ones, and says which rules the stream cannot show.

  **Every later step that adds a trait operation a consumer calls on a live instance also adds its
  `&self` form to `EntryDelivery`'s `DeliveryHandle`** ([DI-D-32](DESIGN-NOTES.md#di-d-32)): the
  handle mirrors those operations rather than lending the instance, so nothing adds them for it.
  `pop` and `readiness` stay off it.

## Moved 2026-10-08 20:42:29 -04:00 -- DI-3.9.1: the review of DI-D-34

### <a id="di-391"></a>DI-3.9.1 -- The engineer confirmed DI-D-34 with three amendments: unmatched import domains reported, a heal released only by a seal reaching the re-issue, and inconsistencies panicked on where found. *(completed 2026-10-08 20:42:29 -04:00)*

The review, point by point, and the amendments it made are [DI-D-46](DESIGN-NOTES.md#di-d-46). The item as written:

**[DI-D-34](DESIGN-NOTES.md#di-d-34)**, from `DI-3.2.4`: a failed flush and
  an import as one transition; reach by declared flush domains, a domain no file declared reaching
  only the undeclared files; what the core records of each write, and when an epoch passes the
  mark; what abandon, heal and `resolve` do and answer; an internal inconsistency recorded on a
  pool thread and asserted by the consumer's next `pop`; and the fault seam,
  `fail_next_flush`, behind the `fault-injection` feature that every test build enables.

## Moved 2026-10-08 21:57:56 -04:00 -- DI-3.9.2: the review of DI-D-39

### <a id="di-392"></a>DI-3.9.2 -- The engineer confirmed DI-D-39 with amendments: every report cites its provenance, a covered marking means covered successfully and cites who answered, and a nullifier's code is a plain `ErrorCode`. *(completed 2026-10-08 21:57:56 -04:00)*

The review and the amendments it made are [DI-D-47](DESIGN-NOTES.md#di-d-47). The item as written:

**[DI-D-39](DESIGN-NOTES.md#di-d-39)**, from `DI-3.2.4.2`, less what
  [DI-D-42](DESIGN-NOTES.md#di-d-42) amended: `Marking { write, observed, kind }` with a
  non-exhaustive `MarkingKind`; when each marking is made, a covered one by the write's own file's
  flush rather than by its seal; where markings are read; error codes, read with `ErrorCode::of`;
  and the oracle's rules for markings.

## Moved 2026-10-09 17:36:42 -04:00 -- DI-3.10.1: the target design

### <a id="di-3101"></a>DI-3.10.1 -- The target design: `IoTarget` and its obligations, `DurableIoCore` and its backend, `FailureCause` and `IoFault`, and two crates (DI-D-50). *(completed 2026-10-09 17:36:42 -04:00)*

The decision is [DI-D-50](DESIGN-NOTES.md#di-d-50); the reasoning behind the obligations and short writes is in [the design session](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md#di-3101-proposals-names-and-the-obligations-2026-10-09). The item as it stood at completion:

**Design: the target and its obligations, and relayed causes.** Settle the
  obligations DI-D-48 drafts and the target's name; how a target is given to and taken from an
  instance, replacing `add_file`, `remove_file`, `FileBusy` and `FileOptions`; failure sharing
  between targets in place of per-file flush domains (DI-D-21); what a provider request names in
  place of a `SharedHandle` (DI-D-29); the built-in default provider as a target's own commit
  (DI-D-27); and causes that each name their source -- the layer below, the consumer, or the
  provider -- restating DI-D-47's cause shapes. Also: the backend's ring shape (commit scope, how
  completions are learned of, buffer ownership), a target that breaks its obligations, the core's
  public API, the crates and their names, and `DI-3.3`'s shape. Designed by the assistant as
  concrete proposals the engineer approves or rejects (DI-D-49).

## Moved 2026-10-09 18:00:15 -04:00 -- DI-3.10.2: the contract restated over targets

### <a id="di-3102"></a>DI-3.10.2 -- CONTRACT.md restated over targets: the obligations, warranty classes by backing, and markers on the decision rows it changes. *(completed 2026-10-09 18:00:15 -04:00)*

[CONTRACT.md](CONTRACT.md) now states the guarantee as holding "as far as the targets it was written to honour their obligations", defines target, backend, commit, deciding commit and domain, replaces "What this contract requires of the handle" with "What this contract requires of a target" -- the obligations of [DI-D-50](DESIGN-NOTES.md#di-d-50) and a table of warranty classes by backing -- and restates seals, failure reach, markings, provenance, the readiness signal, ending and gates over targets and the backend. Overlapping writes in flight now leave the overlapping bytes unspecified rather than landing "in an unspecified order". The worked examples are unchanged, because they compile against the current API; a "Vocabulary ahead of the code" note says so, and `DI-3.10.3` removes it. Amendment markers were added to DI-D-4, 10, 12, 22, 26, 28 and 37; DI-D-11's citation changed vocabulary only. The item as written:

**Design: CONTRACT.md restated over target obligations**, including how far
  each kind of backing can warrant the reporting obligation -- a raw volume held exclusively, an
  unbuffered file, a cached file on a best-effort basis, a layered target such as a VHDX -- and
  amendment markers on every decision row the restatement changes.

## Moved 2026-10-09 18:46:19 -04:00 -- DI-3.10.3.1: the renames

### <a id="di-31031"></a>DI-3.10.3.1 -- DI-D-50's renames in place: `IoTargetKey`, `IoTargetBusy`, `IoTargetWrites`, `AddTargetError`, `RemoveTargetError`, `add_target`, `remove_target`, `TargetSetup`, `FailureCause::Commit`. *(completed 2026-10-09 18:46:19 -04:00)*

Applied by a word-bounded script over `src/`, CONTRACT.md and `sabotage.json` (`fs::remove_file` excluded), then by hand for the variant's field and two public fields: `SuspectWrite::file` and `IoTargetBusy::file` became `target`, and `IoTargetBusy`'s display says "target". `TargetSetup`'s and `AddTargetError`'s `file: SharedHandle` stay, being the Win32 handle `DI-3.10.3.5` moves into the file target. API.md, DESIGN-NOTES.md and this archive keep the old names as history. The item as written:

**The renames, in place.** DI-D-50's names for what keeps its shape:
  `FileKey` -> `IoTargetKey`, `FileBusy` -> `IoTargetBusy`, `FileWrites` -> `IoTargetWrites`,
  `AddFileError` / `RemoveFileError` -> `AddTargetError` / `RemoveTargetError`, `add_file` /
  `add_file_with` / `remove_file` -> `add_target` / `add_target_with` / `remove_target`,
  `FileSetup` -> `TargetSetup`, `Cause` -> `FailureCause` and `Cause::Flush { file, .. }` ->
  `FailureCause::Commit { target, .. }`; tests, the oracle, the sabotage manifest's anchors and
  CONTRACT.md's examples follow. No behaviour changes.

## Moved 2026-10-09 18:57:28 -04:00 -- DI-3.10.3.2: IoFault and no short writes

### <a id="di-31032"></a>DI-3.10.3.2 -- `IoFault` on `Outcome::Failed`, `FailureCause::Commit` and `Nullified`; `ErrorCode`'s meaning the source's; no short writes, the Windows path turning a short count into a fault. *(completed 2026-10-09 18:57:28 -04:00)*

`IoFault { source, code, error }` is public; the Windows path builds every one through `windows_fault`, naming `"windows-ioring"`. `MarkingKind` lost `Short` and `Copy`; `WriteEnd` is `Succeeded` or `Failed(IoFault)`; the core's write records lost their requested length, which now rides in the sidecar for `relay::op_outcome`, unit-tested in `relay/tests.rs`. The oracle compares a nullifier's fault with its write's completion's. The assistant's choices are queued for review as `DI-3.9.6`. The item as written:

**`IoFault`, a portable `ErrorCode`, and no short writes.** `IoFault {
  source, code: Option<ErrorCode>, error }` on `Outcome::Failed`, `FailureCause::Commit` and
  `MarkingKind::Nullified { fault }`; `ErrorCode` a portable integer, its reading from an
  `io::Error` Windows-only; `MarkingKind::Short` and the core's requested-length check removed,
  the Windows path turning a short count into a fault (the `Fault` policy of DI-D-50).

## Moved 2026-10-09 19:04:17 -04:00 -- DI-3.10.3.3: the diagnostic sink

### <a id="di-31033"></a>DI-3.10.3.3 -- The diagnostic sink: `DiagnosticSink` with `NoDiagnostics`, `StderrDiagnostics` and `FanOutDiagnostics`, given in `Setup`, carrying the inconsistency report; the `trace` feature removed. *(completed 2026-10-09 19:04:17 -04:00)*

Built per [DI-D-51](DESIGN-NOTES.md#di-d-51) in `src/diagnostics.rs`. The core holds the sink and asks `wants` before it builds an event; `inconsistent` reports through it and then panics as before. Tests cover each sink, the fan-out in both directions, and the core's report with a sink that wants it and one that does not; four sabotages were added and all were caught. The assistant's choices are queued in `DI-3.9.6`. The item as written:

**The diagnostic sink** (DI-D-51): `DiagnosticSink`, `NoDiagnostics`,
  `StderrDiagnostics`, `FanOutDiagnostics`, a sink given at construction, the inconsistency report
  sent through it, and the `trace` feature removed. The Windows driver defaults to
  `NoDiagnostics` until `DI-3.6` builds `EtwDiagnostics` and makes it the default.

## Moved 2026-10-09 19:26:34 -04:00 -- DI-3.10.3.4: the platform-free core

### <a id="di-31034"></a>DI-3.10.3.4 -- The core made platform-free: `StampClock` and an `Instant` on `Identities`, `DioringIds<E, I>`, and no Windows type in `dioring/durability*` outside its tests. *(completed 2026-10-09 19:26:34 -04:00)*

`src/clock.rs` holds `StampClock` and the `Instant` bound; the interrupt-time clocks implement `StampClock` in `dioring.rs`, whose `WinInstant` names the Windows time type. The core's `Event`, `Due`, `Sealing`, `Failure`, `FileHolds` and `Durability` take the time type as a parameter; `Failed`, `Marking`, `FailureInfo` and the oracle's stamp rule carry `V::Instant`. A new test runs the core on a counter clock. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**The core made platform-free**: a portable clock trait the core is
  generic over, failure and marking stamps in its time type -- an `Instant` associated type of
  `Identities`, which the shared types (`Failed`, `Marking`, `FailureInfo`) and the oracle carry
  -- and nothing in `dioring/durability*` naming a Windows type. `DioringIds` gains the time type
  as a parameter, defaulted to interrupt time until `DI-3.10.3.6` moves that default to the
  Windows crate. The public `DurableIoCore` API moved to `DI-3.10.3.5`: it returns ring entries,
  whose shape the backend defines.

## Moved 2026-10-10 00:35:35 -04:00 -- DI-3.10.3.5.1: the target and backend traits

### <a id="di-310351"></a>DI-3.10.3.5.1 -- The target and backend traits: `IoTarget` and its description, `IoBackend` with the core's submission and completion shapes and the typed `RefusalCause`, and a minimal in-memory backend and target. *(completed 2026-10-10 00:35:35 -04:00)*

`src/io_target.rs` holds `IoTarget`, `IoTargetDescription`, `WarrantyClass`, `Geometry` and `Capabilities`; `src/backend.rs` holds `IoBackend`, `Submission`, `Completion` and `RefusalCause` (DI-D-52); `src/memory.rs` holds `MemoryBackend`, `MemoryTarget` and `MemoryNotification`. Nothing in the driver uses them yet: the core takes them in `DI-3.10.3.5.2`. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**The target and backend traits**: `IoTarget`, the backend trait, the
  core's submission and completion shapes, and the typed refusal cause (DI-D-52); with a minimal
  in-memory backend and target that fault on nothing, so the traits are exercised before anything
  is built on them. `DI-3.3` adds the fault script to that backend.

## Moved 2026-10-10 05:54:59 -04:00 -- DI-3.10.3.5.2: the public core

### <a id="di-310352"></a>DI-3.10.3.5.2 -- `DurableIoCore`, public: targets, writes and reads, completions, the seal and its commits, refusals, the entry queue and in-flight counts, over the in-memory backend. *(completed 2026-10-10 05:54:59 -04:00)*

`src/io_core.rs` holds `DurableIoCore` and `CoreActions`, over the durability core the Windows driver also uses; its tests drive it over `MemoryBackend`. The seal moved into this step from `DI-3.10.3.5.3` (commit `5e240c78`), since without it no commit exists to test. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**`DurableIoCore`: targets, operations and completions**: public;
  `add_target` and `remove_target` taking a target's description; writes and reads checked,
  recorded and emitted as submissions in `CoreActions`; `complete` turning a completion into
  entries; `refused` undoing a consumer operation with its cause, and keeping a commit to emit
  again; the entry queue and the in-flight counts; and `make_durable_through`, the commits it
  emits, and `durable_through`, `sealed_through` and `epoch_state`, since without a seal no
  commit exists and the commit path -- completed, failed and refused -- could not be reached by a
  test. Tested on the in-memory backend.

## Moved 2026-10-10 09:25:41 -04:00 -- DI-3.10.3.5.3: the core completed

### <a id="di-310353"></a>DI-3.10.3.5.3 -- `DurableIoCore` completed: gates and held operations, the provider's requests and answers, lineages, failures, and `close`, over the in-memory backend. *(completed 2026-10-10 09:25:41 -04:00)*

The core now carries the whole `DurableRing` surface without a ring: `src/io_core.rs` with gates, and its `lineages`, `failures` and `provider` parts, tested in `src/io_core/tests/`. `IoTargetWrites` lost its lent handle on both paths (DI-D-50 P6). The stale "interrupt time, dioring's one time base" on `Failed::observed` and in the oracle's stamp rule, left by `DI-3.10.3.4`, was corrected in the same sweep. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**`DurableIoCore`: gates, seals, providers, lineages and failures**: the
  rest of today's `Dioring` and `Core` state moved over the core -- held operations and their
  release, provider requests and answers,
  lineages minted, ended and retired, `resolve` and `import_failure` -- so the core alone carries
  the whole `DurableRing` surface. Tested on the in-memory backend.

## Moved 2026-10-10 09:44:08 -04:00 -- DI-3.10.3.5.4: the oracle beside the core

### <a id="di-310354"></a>DI-3.10.3.5.4 -- The oracle beside the core: the push hook takes each operation's length, a write completing other than in full is `WriteNotInFull`, and the core's tests run under the oracle. *(completed 2026-10-10 09:44:08 -04:00)*

`src/oracle.rs` gains the length and the rule; its own tests assert both directions. Every push hook caller -- the oracle's tests, the driver's harness, the delivery test -- passes the real length. The core's test rig in `src/io_core/tests.rs` reports to a `ConformanceOracle`, and a sabotage that drops its seal reports is caught by the oracle alone, which shows the binding is live. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**The oracle beside the core**: the push hook gains each write's length,
  and the rule that a write never completes short (DI-D-50), queued here from `DI-3.10.3.2`; the
  oracle run beside the core's tests on the in-memory backend.

## Moved 2026-10-10 12:59:25 -07:00 -- DI-3.10.3.5.5: the Windows backend and the Win32 file target

### <a id="di-310355"></a>DI-3.10.3.5.5 -- The Windows backend and the Win32 file target: `WindowsBackend` over `IoRing`, threadless, with its refusal causes mapped from the ring's reports, and `Win32FileTarget` over a `SharedHandle`. *(completed 2026-10-10 12:59:25 -07:00)*

`src/windows_backend.rs` holds `WindowsBackend` and its setup; `src/windows_backend/file_target.rs` holds `Win32FileTarget`; `src/windows_backend/tests.rs` drives a real ring, maps every `RingCondition` through a real completion's injected failure, and runs `DurableIoCore` over the backend under the oracle. Nothing in `Dioring` uses the backend yet: that is `DI-3.10.3.5.6`. Paths no test can reach are named in the sabotage manifest's `notCoveredHere`. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**The Windows backend and the Win32 file target**: the backend over
  `windows-ioring-sys`' `IoRing`, threadless -- push, submit, pop, and the ring's completion event
  as its notification -- its refusal causes mapped from `RingCondition`, with a test for each
  mapping a developer machine can reach; and the Win32 file target over a `SharedHandle`, carrying
  its flush domains and warranty class.

## Moved 2026-10-11 10:40:04 -04:00 -- DI-3.10.3.5.6.1: pool delivery over the threadless backend

### <a id="di-3103561"></a>DI-3.10.3.5.6.1 -- Pool delivery over the threadless backend: the portable `Engine` that drains a backend through the core, and `PoolDelivery` that drains it on the ring crate's `CompletionWake`, with an optional callback environment. *(completed 2026-10-11 10:40:04 -04:00)*

Built per [DI-D-53](DESIGN-NOTES.md#di-d-53), on the ring crate's `M33.1`. `src/engine.rs` holds the engine, tested over the in-memory backend under the oracle; `src/windows_backend/pool.rs` holds pool delivery, tested over a real ring and the thread pool under the oracle -- delivery, readiness on each transition, a backlog reached at setup, a callback environment's pool, and the engine handed back -- and run fifty times without a failure. Nothing in `Dioring` uses either yet: that is `DI-3.10.3.5.6.2`. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**Pool delivery over the threadless backend** (DI-D-52's Model A):
  the Windows backend handing out the ring crate's `CompletionWake` over its ring, with an optional
  callback environment so the pool may be bound to a NUMA node; and a driver-side delivery that,
  woken, takes the driver's lock and drains the backend through `DurableIoCore` to empty. Tested
  standalone, the core over the backend under the oracle, before the driver moves onto it.

  > **CROSS-COMPONENT PREREQUISITE:** `crates/windows-ioring-sys` -> `M33` -> `M33.1` (`CompletionWake`) must land first. See [CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md).

## Moved 2026-10-11 12:05:15 -04:00 -- DI-3.10.3.5.6.2.1: what the backend refuses for good, the core ends

### <a id="di-31035621"></a>DI-3.10.3.5.6.2.1 -- What the backend refuses for good, the core ends: a gated operation is put to the backend's `check` before it is held, and a submission of the core's own refused for a cause that is not retryable completes with a `REFUSED` fault. *(completed 2026-10-11 12:05:15 -04:00)*

The decision is [DI-D-54](DESIGN-NOTES.md#di-d-54); how it was found is in the session log's "DI-3.10.3.5.6.2 re-planned, and what the backend refuses for good". It rests on the ring crate's [M34.1](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m341), `RegisteredBuffers::check_span`, committed first. `IoBackend::check` is required of every backend: the in-memory backend's is its refusal less the queues, and the Windows backend's its pre-ring refusal plus the registration's span check, while its push still leaves spans to the ring. Each backend's `check` is bound to its `push` in both directions, case by case (`check_answers_what_push_does_whatever_the_queues_hold`, `check_answers_what_push_does`). The core's tests in `src/io_core/tests/refusals.rs` pin a gated operation refused at push for what the backend cannot perform or its target does not admit, and held when admitted; the core's own checks coming first; a released write and a released read ended as `Failed` with the cause recoverable, the write nullified so its epoch still becomes durable; a commit ended as its seal's failure; and both directions of `is_retryable` through `refused`. The engine's test pins that what an ended refusal makes due is acted on in the same call. Nine sabotages are caught and a control survives; three entries whose code moved were re-anchored and are still caught. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**What the backend refuses for good, the core ends**: the backend is asked, before the core holds an operation for its gate, whether it would admit it (`IoBackend::check`), so a gated operation is refused at push for the causes an ungated one is, after every check of the core's own; and a submission of the core's own -- an operation its gate released, or a commit -- that the backend refuses for a cause that is not retryable ends instead of being re-emitted: the operation completes as `Failed` with a fault carrying the cause, a write nullified as one its target failed is, and the commit fails its seal's flush of that target. Over the in-memory backend, with the Windows backend's `check` its pre-ring refusal plus the registration's own span check (`windows-ioring-sys`' M34.1).

## Moved 2026-10-11 12:10:38 -04:00 -- DI-3.10.3.5.6.2.2: readiness an associated type

### <a id="di-31035622"></a>DI-3.10.3.5.6.2.2 -- Readiness an associated type of the ring trait (DI-D-52 Q6); today's driver's is the `Event` it hands out now. *(completed 2026-10-11 12:10:38 -04:00)*

Per [DI-D-52](DESIGN-NOTES.md#di-d-52) Q6. `DurableRing` gains `type Readiness`, unbounded, and `readiness()` returns it; `Dioring` and the test fake name `Event`. The two consumers that wait on the signal through the thread pool -- `EntryDelivery` and the oracle's `check_readiness` -- bind `Readiness = Event`, and their docs say so; CONTRACT.md's "The readiness signal" names the associated type. A type-level change: the build is its proof, and the delivery and readiness tests over today's driver still pass. No sabotage is added, as nothing behavioural changed. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**Readiness an associated type** of the ring trait (DI-D-52 Q6), so an implementation names its own; today's driver's is the `Event` it hands out now.

## Moved 2026-10-11 12:16:21 -04:00 -- DI-3.10.3.5.6.2.3: targets as the implementation's own type

### <a id="di-31035623"></a>DI-3.10.3.5.6.2.3 -- Targets as the implementation's own type: the ring trait's `Target`, today's driver's a `Win32FileTarget`, which `TargetSetup` takes too; `FileOptions` removed. *(completed 2026-10-11 12:16:21 -04:00)*

Per [DI-D-50](DESIGN-NOTES.md#di-d-50) P5. `DurableRing` gains `type Target: IoTarget`; `add_target(key, target)` hands it back in a now-generic `AddTargetError<T>`, and `remove_target` returns it; `add_target_with` and `FileOptions` are gone, a target's flush domains being its own. `Dioring` keeps the `Win32FileTarget` in its file slot and hands it back; `TargetSetup` is `{ key, target }`; `DeliveryHandle` forwards the new shapes; the test fake's target is `MemoryTarget`. CONTRACT.md's three worked examples build a `Win32FileTarget`, and its "Vocabulary ahead of the code" note no longer mentions `FileOptions`. `FileOptions`' own tests went with it, and the rule one of them pinned -- declaring domains again replaces them -- is now pinned on `Win32FileTarget`. Every sabotage anchor still matches once. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**Targets as the implementation's own type**: the ring trait's `Target`, given to `add_target` and handed back by `remove_target` and `AddTargetError`; today's driver's is `Win32FileTarget`, which `TargetSetup` takes too, its flush domains now the target's; `FileOptions` removed. CONTRACT.md's examples, `EntryDelivery`, the fake and the tests follow.

## Moved 2026-10-11 12:26:08 -04:00 -- DI-3.10.3.5.6.2.4: write-only buffer types on the Windows backend

### <a id="di-31035624"></a>DI-3.10.3.5.6.2.4 -- Owned writes keep write-only buffer types on the Windows backend (DI-D-17): `WindowsBackend<B: IoBuf>`, an owned read pushed through a push made where `B: IoBufMut` holds. *(completed 2026-10-11 12:26:08 -04:00)*

Restores [DI-D-17](DESIGN-NOTES.md#di-d-17) and [API.md](API.md) section 8 on the new path: the backend built in `DI-3.10.3.5.5` bound `B: IoBufMut`, which the swap would have made the driver's bound, taking write-only buffer types such as `Arc<[u8]>` away; no test pinned it. `WindowsBackend` and pool delivery now need only `B: IoBuf`. `new` (for `B: IoBufMut`) makes the owned read's push; `without_owned_reads` does not, and an owned read on such a backend is refused as `Unsupported` at `check` and at `push`; a crate-private `enable_owned_reads` lets the driver make the push where its read's bound holds. The tests write an `Arc<[u8]>` through the backend -- its own buffer handed back, an owned read refused, a registered read still run -- and through the core and engine to two targets, reported durable; and show it is the constructor, not the type, that decides. Two sabotages are caught; a third, narrowing the backend's trait impl to `IoBufMut`, is refused by the build, in the library, where pool delivery is declared over `IoBuf`. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**Owned writes keep write-only buffer types on the Windows backend** ([DI-D-17](DESIGN-NOTES.md#di-d-17), [API.md](API.md) section 8): `WindowsBackend<B: IoBuf>`; an owned read is pushed through a push captured where `B: IoBufMut` holds, as today's driver does, and refused as `Unsupported` without one; a test writing an `Arc<[u8]>` through the backend, and through the core over it, pins the bound that was missing.

## Moved 2026-10-11 12:29:44 -04:00 -- DI-3.10.3.5.6.2.5: registered buffers reached through a closure

### <a id="di-31035625"></a>DI-3.10.3.5.6.2.5 -- Registered buffers reached through a closure: `with_registered_buffer` and `with_registered_buffer_mut`, run under the instance's lock (DI-D-17's guarded accessors). *(completed 2026-10-11 12:29:44 -04:00)*

[DI-D-17](DESIGN-NOTES.md#di-d-17) has the instance hold the registration and lend it through guarded accessors. The borrow-returning forms could not survive the swap, which puts the registration behind the engine's lock that pool drains share. `RegisteredBufferRing`'s accessors now run the caller's closure on the bytes under the instance's lock, and do not call it when they refuse; today's driver implements them over its registration, and `DeliveryHandle`, which already had the closure form, forwards to them. The driver's tests use them, and the refusal cases pass closures that would panic if called. Two sabotages, lending an empty slice without asking the registration, are caught. The assistant's choices are queued in `DI-3.9.6`. The item as it stood at completion:

**Registered buffers reached through a closure**: the registered-buffer ring's accessors become `with_registered_buffer` and `with_registered_buffer_mut`, run under the instance's lock -- DI-D-17's guarded accessors, in the shape `DeliveryHandle` already has -- because once the swap puts the registration behind the engine's lock, no borrow of it can outlive that lock; today's driver implements them.

## Moved 2026-10-11 13:48:35 -04:00 -- DI-3.10.3.5.6.2.6 and DI-3.10.3.5.6.2: the swap, and the switch-over it ends

### <a id="di-31035626"></a>DI-3.10.3.5.6.2.6 -- The swap: `Dioring` runs `DurableIoCore` over the Windows backend under pool delivery, and the old path is deleted. *(completed 2026-10-11 13:48:35 -04:00)*

Per [DI-D-53](DESIGN-NOTES.md#di-d-53): the merge-or-delete decision for the duplicated path, taken as delete. `Dioring` is a driver over an `Engine` of the core and the threadless Windows backend, in pool delivery's shared slot; a new `Hub` holds what its paths off the consumer's thread share -- the slot, the readiness event, the count of unanswered provider completions -- and is where a provider's answer and a lineage handle's release arrive. `Setup` gains `environment`, the callback environment delivery runs on, and two lifetimes. `PushRefusal::Ring` and `NoRegisteredBuffers` give way to the backend's refusal cause; `Core`, `push.rs`, `relay.rs` and `ending.rs` are deleted. `pop` submits, pops and makes the provider's requests; its retry of the core's refused submissions was found to be an equivalent mutant and removed. `end_lineage` reports a core refusal for a lineage its last handle names as an inconsistency, as the old core did -- the swap first ignored it, and the full sweep found that. Tests: an instance delivering on a named pool; a request made in the drain reaching the provider only through pops; the relay's `transfer` cases moved to the backend. Sabotages: 33 re-anchored where their rules now live, 10 retired as same-site duplicates or with the relay, two added; `pop`'s failed-submit report declared a blind spot, with the seam that would reach it proposed as `DI-3.10.3.5.6.3`. The full sweep behaved as declared but for that one guard, fixed and caught on re-run. The assistant's choices are queued in `DI-3.9.6`, and the findings are in the session log. The item as it stood at completion:

**The swap**: `Dioring` runs `DurableIoCore` over the Windows backend under pool delivery, with the callback environment in `Setup`; `PushRefusal::Ring` and `PushRefusal::NoRegisteredBuffers` replaced by the backend's refusal cause; `Core`, and the ring code of `push.rs` and `relay.rs`, deleted -- the merge-or-delete decision for the duplicated path; the tests and sabotages moved over. Today's tests and sabotages are the proof, the sabotages re-anchored where the code moved.

### <a id="di-3103562"></a>DI-3.10.3.5.6.2 -- The switch-over: the driver onto the core, in six steps, ending in the swap that runs `Dioring` on `DurableIoCore` over the Windows backend under pool delivery and deletes the old path. *(completed 2026-10-11 13:48:35 -04:00)*

Its steps, each archived above: [DI-3.10.3.5.6.2.1](#di-31035621), [DI-3.10.3.5.6.2.2](#di-31035622), [DI-3.10.3.5.6.2.3](#di-31035623), [DI-3.10.3.5.6.2.4](#di-31035624), [DI-3.10.3.5.6.2.5](#di-31035625) and [DI-3.10.3.5.6.2.6](#di-31035626). The item as it stood at completion, with both re-plans:

**The switch-over**: the driver onto the core, in the steps below. Re-planned from one step into four once mapping the old driver onto the new path showed two things. First, the new core has a defect the switch-over would expose: it holds a gated operation without asking the backend whether it would admit it, and re-emits forever a submission of its own that the backend refuses for a cause that cannot clear -- so a gated registered-span write on an instance with no registration, which today's driver refuses at push, would be accepted and then never complete, and its seal never resolve. Today's driver shares the second half ("no test reaches that"), so this is a fix to both, made in the core. Second, two of the step's API changes stand alone on today's driver, which keeps the switch-over itself to the swap and its deletions. Re-planned again, from four steps to six, on mapping the swap itself: the Windows backend built in `DI-3.10.3.5.5` bound its buffer to `IoBufMut`, which the swap would make the driver's bound and so take write-only buffer types away, against [DI-D-17](DESIGN-NOTES.md#di-d-17) and API.md section 8; and the registered-buffer accessors return a borrow, which no implementation can give once the registration lives behind the engine's lock that pool drains share.

## Moved 2026-10-11 15:59:15 -04:00 -- DI-M3 closed, its open work restructured into DI-M7 to DI-M12

<a id="di-m3-restructured"></a>The engineer, 2026-10-11: "please re-do the di-m3 plan, this depth of subitem is hard to work with."
DI-M3's items had nested six levels (`DI-3.10.3.5.6.2.6`). Its open work became six flat milestones;
its completed items were already archived above, each under its own heading, and their stubs were
removed. The old IDs map as follows.

| Old ID | New ID |
|---|---|
| `DI-3.10.3.5.6.3` | DI-7.1 |
| none: new, the construction-time hook | DI-7.2 |
| `DI-3.10.3.5.7` | DI-7.3 |
| `DI-3.10.3.5.8` | DI-7.4 |
| `DI-3.10.3.6` | DI-8.1, DI-8.2, DI-8.3 |
| `DI-3.9.3` | DI-9.1 |
| `DI-3.9.4` | DI-9.2 |
| `DI-3.9.5` | DI-9.3 |
| `DI-3.9.6` | DI-9.4, DI-9.5, DI-9.6, and DI-9.7 for what follows |
| `DI-3.10.4` | DI-10.1 |
| `DI-3.10.5` | DI-10.2 |
| `DI-3.3` | DI-11.1 |
| `DI-3.11` | DI-11.2 |
| `DI-3.6` | DI-12.1 |
| `DI-3.8` | DI-12.2 |
| `DI-3.4` | DI-12.3 |
| `DI-3.5` | DI-12.4 |

The umbrella items, whose own text was a plan for steps that now each have a home, are dissolved
rather than checked; each is kept here as it stood. `DI-3.7`, folded into `DI-3.2` on 2026-10-07, is
archived with them.

### <a id="di-39"></a>DI-3.9 -- Dissolved into DI-M9. *(dissolved 2026-10-11 15:59:15 -04:00)*

The item as it stood: **The engineer confirms or amends the shapes the assistant chose while
implementing `DI-3.2`**, one decision per step below; checked when the last of them is. The code
binds to each already, because the steps could not be built without choosing; what is open is the
review. A rejected point is reworked in its step, with its decision row amended. Placed before
`DI-3.3`, because a reworked point can change the trait the fault-injecting implementation
implements. Each step reviews only what its decision row marks as the assistant's: where the
engineer had already decided part of a row, that part is not reopened.

### <a id="di-310"></a>DI-3.10 -- Dissolved into DI-M7 to DI-M10. *(dissolved 2026-10-11 15:59:15 -04:00)*

The item as it stood: **The contract over targets, and dioring as a portable algorithm**
([DI-D-48](DESIGN-NOTES.md#di-d-48), [DI-D-49](DESIGN-NOTES.md#di-d-49)): the unit of I/O and
durability becomes a target, defined only by its obligations, files leave the contract's vocabulary,
and the core becomes a portable public state machine with targets and a backend beneath it and a
threaded driver above it. Checked when the last step below is. Placed before `DI-3.9.5` and
`DI-3.3`, which are written in the file vocabulary this restates; `DI-3.9.3` and `DI-3.9.4` do not
use it, and stay ahead.

### <a id="di-3103"></a>DI-3.10.3 -- Dissolved into DI-M7 and DI-M8. *(dissolved 2026-10-11 15:59:15 -04:00)*

The item as it stood: **The portable core, the Windows backend and the driver, restated in place**
as `DI-3.10.1` and `DI-3.10.2` settle them: the durability core as a public state machine naming no
platform type; targets and a backend beneath it, with `windows-ioring-sys` inside the Windows
backend; the threaded driver above it implementing `DurableRing`; the trait, types, oracle and fake
restated over targets. Split as [DI-D-50](DESIGN-NOTES.md#di-d-50) names: `durable-ioring`,
portable, and `win-durable-ioring` for the driver, the Windows backend and its targets, registered
for release. Today's tests and sabotages are the proof the Windows backend passes, and today's file
path stands in as its target until `DI-3.10.4`. The portable crate joins CI's `portable` job.
CONTRACT.md's worked examples are restated in the new API, and its "Vocabulary ahead of the code"
note is removed. Diagnostics per [DI-D-51](DESIGN-NOTES.md#di-d-51): the `DiagnosticSink` trait,
`NoDiagnostics`, `StderrDiagnostics` and `FanOutDiagnostics`, the inconsistency report sent through
the sink, and the `trace` feature removed. Done in the steps below, each compiling and passing the
tests and the sabotage sweep on its own.

### <a id="di-31035"></a>DI-3.10.3.5 -- Dissolved into DI-M7. *(dissolved 2026-10-11 15:59:15 -04:00)*

The item as it stood: **Targets, the backend, and the public core**, per
[DI-D-50](DESIGN-NOTES.md#di-d-50) P10-P11 and [DI-D-52](DESIGN-NOTES.md#di-d-52): built as a new
path beside today's driver (`DI-3.10.3.5.1` to `DI-3.10.3.5.5`), which the driver then moves onto,
deleting the old path (`DI-3.10.3.5.6`) -- the merge-or-delete decision for the duplication, taken
when the new path is proven. Done in the steps below.

### <a id="di-310356"></a>DI-3.10.3.5.6 -- Dissolved into DI-M7. *(dissolved 2026-10-11 15:59:15 -04:00)*

The item as it stood: **The driver moved onto the core, under pool delivery**, per
[DI-D-53](DESIGN-NOTES.md#di-d-53): delivery is a waker beside the threadless backend, never
delivery owning the backend's ring. Done in the steps below.

### <a id="di-37"></a>DI-3.7 -- Folded into `DI-3.2` by the engineer: the conformance oracle is built from `DI-3.2.2.1`. *(folded 2026-10-07; archived 2026-10-11 15:59:15 -04:00)*

The item as it stood: **Folded into `DI-3.2` by the engineer, 2026-10-07: the conformance oracle is
built from `DI-3.2.2.1`, and each later step adds its rules.** What the oracle is, and the readiness
signal's harness check beside it, are stated under `DI-3.2` and `DI-3.2.2.1`.

## Moved 2026-10-11 23:29:17 -04:00 -- DI-7.1: failed submissions reached by tests

### <a id="di-71"></a>DI-7.1 -- Failed submissions reached by tests, through the ring crate's safe submit-fault responder registered on the instance's ring. *(completed 2026-10-11 23:29:17 -04:00)*

Raised by implementing it: a test in this crate cannot implement the ring crate's `Responses`, whose
methods are `unsafe fn`, and the crate forbids `unsafe` ([DI-D-4](DESIGN-NOTES.md#di-d-4)). That
decision says to add the safe wrapper to the lower crate, so the ring crate gained
`sys::submit_faults` (its `M37.1`, [D-86](../windows-ioring-sys/DESIGN-NOTES.md#d-86)) and this item
waited on it. The tests, in [dioring/tests/submits.rs](src/dioring/tests/submits.rs) and the
backend's tests: a push whose submit fails is accepted and owed; a pop whose submit fails reports
the cause as an error that downcasts to its `RefusalCause`, repeatedly, and the work is issued and
completes once submits succeed; a drain's submit fails on a pool thread, shown by the thread the
responder records, leaving the second read queued until the next submit; a seal's commit, a
submission of the core's own, whose submit fails is made durable once they succeed; and the
backend's own failed submit after an accepted push, its reissue, that the ring is asked only while a
submit is owed, and each ring condition's cause through the real flow. Stable over 150 runs. The
blind-spot entry is `caught`, seven sabotages are added for the reissue, `pop`'s preference of an
entry over the report, and the drain's first submit, and the two sentences of the manifest's
`notCoveredHere` that these tests reach are removed. The assistant's choices are queued in `DI-9.7`.
The item as it stood at completion:

**Failed submissions reached by tests, through a responder on the instance's ring.** `pop` reports a
failed submission when it has no entry to return instead, and no test reaches that today, so it is a
declared blind spot in [sabotage.json](sabotage.json) ("a pop hides a failed submit"). The ring
crate's kernel responder, registered for one ring
([D-85](../windows-ioring-sys/DESIGN-NOTES.md#d-85)), answers that ring's calls on whatever thread
makes them, so it fails the submits this instance's pool threads make too, and a drain's real submit
cannot undo the injected failure. **The responder is the ring crate's safe `sys::submit_faults`, not
one implemented here**: `Responses`' methods are `unsafe fn`, which this crate's
`#![forbid(unsafe_code)]` rejects, and [DI-D-4](DESIGN-NOTES.md#di-d-4) says a build that fails on
the `forbid` is the moment to add the safe wrapper to the lower crate, not to relax the lint. The
tests enable `kernel-seam` as a dev-dependency feature and reach the ring through a test-only
accessor on the backend, which is why they live in this crate. Tests: a push whose submit fails and
a pop whose submit fails, the pop reporting the cause as an error that downcasts to its
`RefusalCause`, then the work issued and completed once submits succeed; a drain whose submit fails
on a pool thread -- held there by the engine's lock, so it is a guaranteed one -- leaving the work
queued for the next submit; and the Windows backend's own failed submit after an accepted push, and
its reissue. The blind-spot entry is turned to `caught`, sabotages are added for the backend's
reissue, and the paths now reached are removed from the manifest's `notCoveredHere`.

> **CROSS-COMPONENT PREREQUISITE:** `crates/windows-ioring-sys` -> `M37` -> `M37.1` (a safe responder that fails a ring's submits), landed first. See [CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md).

## Moved 2026-10-12 12:15:36 -04:00 -- DI-7.2: construction's registration paths reached by tests

### <a id="di-72"></a>DI-7.2 -- Construction's registration paths reached by tests, through a hook that registers the ring crate's safe call-fault responder on the instance's ring as it is created. *(completed 2026-10-12 12:15:36 -04:00)*

Raised by implementing it: `DI-7.1`'s `submit_faults` reaches no registration path, since those need
a `Build*` call refused, a completion's result failed, and a completion that does not arrive, and
the safe responder could do none of them. The item was re-planned and the ring crate gained
`sys::call_faults` (its `M38.1`, [D-87](../windows-ioring-sys/DESIGN-NOTES.md#d-87)), which this item
waited on. A crate-private `Construction`, carrying the registration bound and a test-only hook
called with the ring as soon as it exists, is handed from `Dioring::with_construction` to
`WindowsBackend::construct`; `with_clock` and `without_owned_reads` keep their signatures and pass
the production default. The tests: at the backend, in
[windows_backend/tests/construction.rs](src/windows_backend/tests/construction.rs), what comes back
for a refused file-registration build, a refused buffer-registration build, a failed submit of
each registration, a failed completion of each, a registration that does not complete within the
bound for files and for buffers, and a completion event that cannot be attached, with the targets
always back and the buffers back only while the kernel was not given them, and a fault armed for a
call construction does not make leaving it alone; and through `Dioring::new`, in
[dioring/tests/construction.rs](src/dioring/tests/construction.rs), that each becomes a
`SetupRefusal::Ring` carrying the ring's own error, with the files, the provider and the buffers
where due handed back. Stable over 30 runs. Fourteen sabotages and a control are added, one
control re-anchored, and the manifest's `notCoveredHere` no longer lists the registration paths or
the completion event's attach; what remains there for them is a ring without the registration
operation as a cause, whose path is the refused build's. The assistant's choices are queued in
`DI-9.7`. The item as it stood at completion:

**Construction's registration paths reached by tests, through a hook that
registers a responder on the instance's ring as it is created.** `Dioring::new` creates its ring,
then registers its files and buffers, submits each registration and waits for its completion, all
before the consumer can name the ring, so a responder registered afterwards cannot answer those
calls. [sabotage.json](sabotage.json)'s `notCoveredHere` lists them as unreached. A crate-private
`Construction`, passed down explicitly from `Dioring::with_clock` to the Windows backend -- never
through a thread-local or a global, per the ring crate's
[D-85](../windows-ioring-sys/DESIGN-NOTES.md#d-85) -- carries two things: a test-only hook,
called with the ring as soon as it exists and before anything is registered, and how long
construction waits for a registration, so a test can shorten the wait for one build rather than
the constant for every test. **The responder is the ring crate's safe `sys::call_faults`**, as
`DI-7.1`'s was `submit_faults`, for the same reason ([DI-D-4](DESIGN-NOTES.md#di-d-4)): the
paths need a `Build*` call refused, a completion's result failed, and a completion that does not
arrive, which no consumer that forbids `unsafe` can write. Tests, each through `Dioring::new`
with everything handed back in its `SetupError`, and at the backend where its own handing back is
the subject: a file registration whose build is refused; a buffer registration whose build is
refused, the buffers handed back; a registration whose submit fails; a registration whose
completion fails; one whose completion does not arrive within the bound, a stalled submit with
the wait shortened; and the completion event's attach failing, the buffers not handed back since
the kernel was given them. **A ring without the registration operation stays unreached as a
cause**: its path is the refused build's, which is reached, and the capability probe is outside
the seam ([D-60](../windows-ioring-sys/DESIGN-NOTES.md#d-60)). The paths reached come out of
`notCoveredHere`, with a sabotage for each.

> **CROSS-COMPONENT PREREQUISITE:** `crates/windows-ioring-sys` -> `M38` -> `M38.1` (a safe responder that fails any seam call, a completion's result, and stalls submits) must land first. See [CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md).

> **CROSS-COMPONENT PREREQUISITE:** `crates/windows-ioring-sys` -> `M38` -> `M38.1` (a safe responder that fails any seam call, a completion's result, and stalls submits), landed first. See [CHECKLIST.md](../windows-ioring-sys/CHECKLIST.md).

## Moved 2026-10-12 13:32:51 -04:00 -- DI-7.3: pumped delivery

### <a id="di-73"></a>DI-7.3 -- Pumped delivery: the consumer's own thread waits on the backend's notification and drains through `pump`, over the Windows backend and the in-memory one. *(completed 2026-10-12 13:32:51 -04:00)*

Per [DI-D-55](DESIGN-NOTES.md#di-d-55). Raised by implementing it: the wait on a handle is a Win32
call no lower crate wrapped safely, and this crate forbids `unsafe`
([DI-D-4](DESIGN-NOTES.md#di-d-4)), so `win-sync-sys` gained `wait_for_single_object` first, as the
first slice of its parked wait functions (its `WS-2.1`,
[WS-D-6](../win-sync-sys/DESIGN-NOTES.md#ws-d-6)), and this item waited on it. What was built, in
[pump.rs](src/pump.rs): the public `Waitable` trait and a portable `PumpedDelivery` over any backend
whose notification is one, sharing `Engine::drain` and the slot helpers, which moved to
[engine.rs](src/engine.rs); `Waitable` for `MemoryNotification` and, in
[windows_backend/wait.rs](src/windows_backend/wait.rs), for the `OwnedHandle` the Windows backend's
notification is. In [dioring.rs](src/dioring.rs), `Setup`'s `environment` became
`delivery: DeliveryMode`, `Pool(environment)` or `Pumped`, with the delivery enum in
[dioring/delivery.rs](src/dioring/delivery.rs), and `Dioring::pump`. Tests: over the in-memory
backend, in [pump/tests.rs](src/pump/tests.rs), a pump draining every completion at once, a wait that
times out after at least its timeout and a zero one that does not block, a raising before the first
pump not lost, one an earlier drain took satisfying exactly one more wait, a raising from another
thread waking a pump with no limit, the wait made without the engine's lock, the engine handed
back with its slot left empty, a notification that cannot be made handing the engine back with its
fault whole or by kind and text, and a wait that fails still draining and reporting why; the Windows
notification's own wait, in [windows_backend/wait/tests.rs](src/windows_backend/wait/tests.rs); and
through `Dioring::new` over a real ring and real files, in
[dioring/tests/pumped.rs](src/dioring/tests/pumped.rs), an instance delivering nothing until it is
pumped and then delivering, a seal made durable through pumps alone, the provider called from the
pump, an answer from another thread setting the readiness event, pumping an instance built with a pool
refused, and a pumped instance ending with work outstanding. Twenty sabotages and a control are added, all
behaving as declared, three of them declared blind spots: the Windows notification's wait failing or
ending abandoned, and `Dioring::pump` reporting such a wait's error, none reachable without
`unsafe`. The question of waiting beside other handles is `DI-7.5`, awaiting a ruling. The
assistant's choices are queued in `DI-9.7`. The item as it stood at completion:

**Pumped delivery** (DI-D-52's Model B): no pool threads; the consumer's own thread waits on the
backend's notification and drains through a `pump` call, over the Windows backend and the in-memory
one. **A portable `PumpedDelivery` over any `IoBackend` whose notification is `Waitable`**, a new
public trait -- wait up to a timeout for the notification to have been raised since the last wait
returned -- implemented for `MemoryNotification` and, for the Windows backend, for the `OwnedHandle`
its notification is, through `win-sync-sys`' safe `wait_for_single_object`, which this crate cannot
write itself (`forbid(unsafe_code)`). It sits beside `PoolDelivery`, sharing the engine's slot
helpers, which move from `pool.rs` to `engine.rs` since they are generic already. `pump(timeout)`
waits **outside the engine's lock**, so a push, an answer or a release is never held up by a wait;
then drains to empty under it, the one drain every delivery shares; then, with the lock released,
makes the provider's requests and sets the readiness event if entries became poppable. A wait that
times out drains too: a drain of nothing is a no-op, and a zero timeout is the non-blocking pump. A
return does not promise entries, since the ring crate's setup signal makes the first wait return at
once ([D-20](../windows-ioring-sys/DESIGN-NOTES.md#d-20)). **The driver** chooses its delivery at
construction: `Setup`'s `environment` becomes `delivery: DeliveryMode`, `Pool(environment)` or
`Pumped`, so an environment cannot be given where nothing uses it, and `Dioring::pump` on an
instance built with a pool is refused as `Unsupported`. Tests: over the in-memory backend, a pump
drains every completion at once, a wait times out after at least its timeout and a zero one never
blocks, a completion raised from another thread wakes a waiting pump, a backlog raised before the
pump is not lost, the engine is handed back at the end, and a notification that cannot be made or
waited on is reported with the engine handed back; over the Windows backend through `Dioring::new`,
nothing is delivered until a pump, a pumped instance delivers what an instance with a pool does, the
provider is called from the pump, and the instance ends with work outstanding. Sabotages for each,
in [sabotage.json](sabotage.json). A decision row, `DI-D-55`, records the shape.

> **CROSS-COMPONENT PREREQUISITE:** `crates/win-sync-sys` -> `WS-M2` -> `WS-2.1` (a safe wait on one handle), landed first. See [CHECKLIST.md](../win-sync-sys/CHECKLIST.md).

## Moved 2026-10-12 14:43:41 -04:00 -- DI-7.5: a third delivery mode, signalled

### <a id="di-75"></a>DI-7.5 -- A third delivery mode, `DeliveryMode::Signalled`: the pool tells the consumer through an event it can wait on beside its own handles, and the consumer drains; and a pump that drains before it waits. *(completed 2026-10-12 14:43:41 -04:00)*

Raised as a question after `DI-7.3`: a pumped consumer that mixes ring I/O with other waits wants the
ring's event, which the ring crate's [D-20](../windows-ioring-sys/DESIGN-NOTES.md#d-20) makes
reachable for it. The engineer asked whether a read-only handle would do, then whether a tee needs a
thread, and ruled "third delivery mode". Per [DI-D-55](DESIGN-NOTES.md#di-d-55). What was built:
[windows_backend/signalled.rs](src/windows_backend/signalled.rs), a `SignalledDelivery` in which the
ring crate's `CompletionWake` owns the ring's event and its callback sets an auto-reset `Event` of
dioring's own; `DeliveryMode::Signalled(environment)` and `Dioring::completion_signal`, which hands out
a duplicate of that event and is refused as `Unsupported` for an instance built another way, in
[dioring.rs](src/dioring.rs); the third variant of the delivery enum in
[dioring/delivery.rs](src/dioring/delivery.rs); and `PumpedDelivery::with_notification` in
[pump.rs](src/pump.rs), so the pump waits on the signal and never on the ring's event, which the wake
owns. **The pump changed for both pumped modes**: it drains before it waits, and returns at once if
that delivered entries, as the ring crate's D-19 says a waiter on an edge must, so a wait of the
consumer's own that took the signal first cannot leave a pump blocked while completions sit in the
ring. That altered `DI-7.3`'s tests, which now assert that a completion already in the backend is
delivered without a wait. Tests: over the in-memory backend, a pump whose edge another waiter took
still delivers at once, and a wait that fails still drains what completed during it; over a real
ring, in [dioring/tests/signalled.rs](src/dioring/tests/signalled.rs), a signalled instance
delivering nothing until its consumer drains though the pool has said so, the signal set when a
completion arrives and not before, the consumer's own wait then a zero pump delivering, a pump after
the consumer took the signal still delivering at once, a consumer that resets the signal costing
nothing but its own wakeup, one event behind every duplicate, an environment accepted, and the
instance ending with work outstanding while the consumer's handle stays good. Eight sabotages and a
control are added, all behaving as declared; the signalled mode's construction failures and its
callback environment are declared not covered, as the pool's are. The wait-only duplicate of the
ring's own event, `SYNCHRONIZE` access alone, is not built and stays available. The assistant's
choices are queued in `DI-9.7`. The item as it stood at completion:

**A third delivery mode, `DeliveryMode::Signalled`: the pool tells the consumer through an event it
can wait on beside its own handles, and the consumer drains.** The engineer's ruling, 2026-10-12, on
a consumer that mixes ring I/O with other waits, which the ring crate's
[D-20](../windows-ioring-sys/DESIGN-NOTES.md#d-20) makes the completion event reachable for. Handing
out the ring's own event would put a handle that can reset it in the consumer's hands
([DI-D-10](DESIGN-NOTES.md#di-d-10)) and let its wait take the edge the pump waits on, so the event
is a tee: the ring crate's `CompletionWake`, a thread-pool wait that holds no thread while it waits,
owns the ring's event and its callback sets an auto-reset `Event` of dioring's own, which
`Dioring::completion_signal` hands out a duplicate of. Nothing is shared with the ring: the consumer
may wait on it, set it or reset it, and the worst it costs is its own wakeup. The mode takes the
optional callback environment, as the pool's does. `pump` in this mode waits on that same event,
never the ring's, which the wake owns (D-21). **Every pump, in either pumped mode, drains before it
waits** and returns at once if that delivered entries, as the ring crate's D-19 says a waiter must: a
consumer's wait that took the edge first can no longer leave `pump(None)` blocked while completions
sit in the ring. `completion_signal` and the mode are refused as `Unsupported` where they do not
apply. Tests: over the in-memory backend, a pump delivers what is there when the edge was taken by
someone else, and a wait that fails still drains; over a real ring, a signalled instance delivers
nothing until the consumer drains, its signal is set when a completion arrives, the consumer's own
wait then `pump(Some(Duration::ZERO))` delivers, a consumer that resets the signal costs nothing but
its own wakeup, an environment is accepted, and the instance ends with work outstanding. Sabotages
for each. `DI-D-55` is amended.

## Moved 2026-10-12 15:08:45 -04:00 -- DI-7.4, completing DI-M7: a blocking push, as an example

DI-M7 (the driver's delivery, finished) completed with this item: `DI-7.1` to `DI-7.3` and `DI-7.5`
are archived above under their own anchors, and the milestone heading and its stubs left
[CHECKLIST.md](CHECKLIST.md) together.

### <a id="di-74"></a>DI-7.4 -- A blocking push, built as a maintained example on the public API rather than as an API of the crate. *(completed 2026-10-12 15:08:45 -04:00)*

Proposed with the engineer's ruling awaited; asked to explain it, the assistant set out the case for
and against and leaned to parking it. The engineer's reply turned it: "Can we build it as an
example, an on-the-side helper? I assume ... it's all built on public APIs, so what we would be
providing is a maintained source code example that clients could use literally or take and adapt."
Per [DI-D-56](DESIGN-NOTES.md#di-d-56). What was built, in
[examples/blocking_push/main.rs](examples/blocking_push/main.rs): `push_blocking`, which tries a
push through a closure given the buffer and context each time, and while the refusal is one
`RefusalCause::is_retryable` clears calls the caller's `make_room` and tries again, up to a patience,
handing the push back whole on a refusal waiting cannot clear, on timing out, and on `make_room`
failing; and a demonstration over a real file and ring in each delivery mode, with queues far smaller
than the writes pushed. Making room is `pump` for a pumped or signalled instance and a short sleep
for a pooled one. Tests, in [examples/blocking_push/tests.rs](examples/blocking_push/tests.rs), run
with the crate's through `test = true` on the example: a pumped and a signalled instance taking
many times their queues' worth of writes by pumping, with a refusal certain to occur; a pooled one
taking them all; a refusal that cannot clear returned at once with its buffer; a push that never
gets room timing out with its buffer; a failing `make_room` reported with the push handed back;
and a push the ring takes never calling `make_room`. Five sabotages, one a hang, are added in this
crate's manifest under `examples/`. **It found a defect below this crate**: its first full-queue test
dropped a pumped instance whose queues were full and the ring's rundown failed with
`IORING_E_COMPLETION_QUEUE_TOO_FULL`, tripping a debug assertion in `Drop`. The ring crate fixed it
at the source, its `M39.1` ([D-88](../windows-ioring-sys/DESIGN-NOTES.md#d-88)), and this item waited
on that commit. The assistant's choices are queued in `DI-9.7`. The item as it stood at completion:

**A blocking push -- proposed, awaiting the engineer's ruling** (DI-D-52 Q4): a driver policy above
refusal that submits, waits on the notification and retries; never from inside a callback, and
pumping for itself under pumped delivery. Removed if the engineer declines it.

## Moved 2026-10-12 22:40:00 -04:00 -- DI-8.1: the core's tests made portable

### <a id="di-81"></a>DI-8.1 -- The core's tests made portable, while still in one crate: a shared portable clock and fault in place of interrupt time and the Windows fault. *(completed 2026-10-12 22:40:00 -04:00)*

Taken as the narrowing the assistant set out when the engineer asked whether the item was ready:
its text was stale, since the core's, engine's and pump's tests already ran on a local clock that
counts its readings. What was Windows' in the tests of the portable modules, and is now not:

- **A shared `test_support` module** (test builds only) holds `Ticks`, the counting clock three
  test modules each had a copy of, `ManualClock`, which a test moves by hand, and `fault`, which
  builds an `IoFault` for an `io::Error` with a named test source in place of the Windows one. All
  three modules now use the one `Ticks`, and the module has tests of its own.
- **The durability state's tests** (`dioring/durability/tests*`) stamp in plain numbers, from a
  `ManualClock`, and name their identities with an explicit time type, so none reads interrupt
  time, borrows the driver's mock clock, or builds a Windows fault. The one test of the default
  clock stamping with interrupt time was a test of the test helper's default and is not carried over:
  the real instance's own test of it is in `dioring/tests/stamps.rs`.
- **The oracle's tests** record stamps as plain numbers and build faults with `test_support`; the
  three tests of the readiness check, which waits on a Windows event, are in a module of their own,
  `oracle/tests/readiness.rs`, so the split takes them whole.
- **`ids` and `types` tests** name the time type they use; the contract's test that runs a consumer's
  generic code over a real `Dioring` is in the driver's tests, where it belongs.

Checked by searching the tests of the portable modules for the Windows crates, interrupt time, the
driver and its fault: nothing is left but the readiness module, which is Windows' by nature, and the
durability events' own `Event` type, which is unrelated. The sabotages that bear on stamps, the
oracle's stamp order and faults were re-run, and all behave as declared. **The fault seam's half is
not here:** `fail_next_flush` and the ring's kernel seam are used by the driver's tests, which move
with the driver, and a portable scripted fault is `DI-11.1`. **The production code of the portable
modules still imports the Windows crates**; removing that is `DI-8.2`, and the item's text now says
what is left for it. The assistant's choices are queued in `DI-9.7`.

The item as it stood at completion:

- [ ] **DI-8.1** -- **The core's tests made portable, while still in one crate.** They read
  interrupt time and Windows faults today, so they move to a portable mock clock and fault, which
  the split needs: once `durable-ioring` has no Windows dependency, they could not build.

## Moved 2026-10-12 23:05:00 -04:00 -- DI-8.2: the split

### <a id="di-82"></a>DI-8.2 -- The split: `win-durable-ioring` created and registered for release, holding the driver, the Windows backend and file target, the readiness `Event` and its check, and the interrupt clock; `durable-ioring` left with no Windows dependency; the worked examples moved and linked. *(completed 2026-10-12 23:05:00 -04:00)*

Built in two commits so the second could be a move and not a rewrite. The first moved the durability
state out of the driver's module and cut the six helpers the core shares with the driver into
`domains.rs`, with the provenance trail a split needs, inside the one crate. The second made the
crate boundary: the Windows side was `git mv`'d into `crates/win-durable-ioring` and its imports
rewritten to name the portable crate. The decisions it took are
[DI-D-57](DESIGN-NOTES.md#di-d-57), the assistant's, queued in `DI-9.7`.

**What the compiler found that the plan did not name.** Four public names changed because Rust's
orphan rule forbids what the single crate did: a blanket `StampClock` for any `win-time-sys` clock,
`Waitable` for `OwnedHandle`, an inherent `ErrorCode::of`, and a default time type on `DioringIds`.
The portable crate also had to show the driver more than it had shown a consumer, which is a
`driver` module. All are in DI-D-57.

**Checked.** Both crates pass fmt, clippy over all targets, their tests and doctests, and rustdoc
with warnings denied; every crate in the workspace builds all targets. The portable crate builds all
targets for `x86_64-unknown-linux-gnu`, which is the first time anything showed it has no Windows
dependency left; running its tests there is `DI-8.3`'s. The sabotage manifest was split by file, with
formatting kept and three entries' targets moved or reworded for the moved code; the driver's runs
its own crate, and the core's runs both, because the driver's tests also pin the core.

The item as it stood at completion:

- [ ] **DI-8.2** -- **The split**: `win-durable-ioring` created and registered for release, holding
  the driver, the Windows backend and file target, `ErrorCode`'s reading from an `io::Error`, the
  readiness `Event`, the interrupt clock, the oracle's readiness check, and `DioringIds`' default
  time type; `durable-ioring` left with no Windows dependency. CONTRACT.md's worked examples drive
  the Windows driver, so they move in the same step, to `win-durable-ioring`'s documentation,
  compiled there as doctests, and CONTRACT.md links them; its "Vocabulary ahead of the code" note is
  removed. `IoBuf`, `IoBufMut` and `RegisteredSpan`, which `contract.rs` and the re-exports name,
  come from `io-buffers`, not from `windows-ioring-sys`, so the portable crate has no Windows
  dependency.

## Moved 2026-10-13 12:20:00 -04:00 -- DI-8.3: CI covers the portable crate; DI-M8 is complete

### <a id="di-83"></a>DI-8.3 -- `durable-ioring` added to CI's `portable` job, so its non-Windows build, tests and lints are checked on Linux and macOS. *(completed 2026-10-13 12:20:00 -04:00)*

`durable-ioring` joins `wtf-string` and `io-buffers` in the build, test and clippy steps of the
`portable` job in [ci.yml](../../.github/workflows/ci.yml), and in the list [DEVELOPMENT.md](../../DEVELOPMENT.md)
gives of crates with a portable core. The job's name, "wtf-string (non-Windows)", is unchanged: it
may be a required status check, which makes renaming it the engineer's call. Checked locally as far
as this host reaches: the crate's build and clippy over all targets for `x86_64-unknown-linux-gnu`
are clean. **Its tests have not been run on a non-Windows host**; the first CI run is the first time
they are, and a failure there is the item's finding, not a regression in the split.

The item as it stood at completion:

- [ ] **DI-8.3** -- **`durable-ioring` added to CI's `portable` job**, so its non-Windows build,
  tests and lints are checked on Linux and macOS, as [DEVELOPMENT.md](../../DEVELOPMENT.md)
  describes for a crate with a portable core.

## <a id="di-m8-complete"></a>DI-M8 is complete

`DI-8.1` to `DI-8.3` are archived above under their own anchors. The milestone heading and its stubs
left [CHECKLIST.md](CHECKLIST.md) with this item, which finished it.

## Moved 2026-10-13 12:57:00 -04:00 -- DI-M9 restructured into tagged, single-choice items

### <a id="di-m9-restructured"></a>DI-M9 -- each review item was one paragraph holding many choices, with no statement of what the engineer was to do; it is now one item per choice, tagged DECIDE, RATIFY or MINE. *(restructured 2026-10-13 12:57:00 -04:00)*

The engineer's complaint, on reading `DI-9.1` to `DI-9.7`, was that the action was not visible. The
groups keep their IDs, so every earlier citation of `DI-9.n` still names the same subject (the table
above that maps `DI-3.9.x` to them stands); each choice inside a group now has a decimal ID
(`DI-9.n.m`) and a tag in [CHECKLIST.md](CHECKLIST.md). Choices whose rows are wholly the engineer's
own, or already decided, were not listed before and are not now. The old wording, which the new items
condense, is in git at `939ad00a`, in this file's sibling `CHECKLIST.md`. Two findings of the
restructure: [DI-D-44](DESIGN-NOTES.md#di-d-44)'s "recorded as inconsistencies" for an unasked-for
answer had been amended by DI-D-46 and the old item still said it, and `DI-D-42`'s dropped oracle
tightening and the Windows default of `NoDiagnostics` had been buried in running text and are now
items of their own (`DI-9.1.4`, `DI-9.4.1`).

## Moved 2026-10-13 13:02:00 -04:00 -- DI-9.1.1 to DI-9.1.3: the engineer confirms three of DI-D-42's choices

### <a id="di-911"></a>DI-9.1.1 -- A lineage handle's release runs on whichever thread drops the last copy. *(completed 2026-10-13 13:02:00 -04:00)*

Accepted by the engineer: "agree, release on the same thread." Recorded in
[DI-D-42](DESIGN-NOTES.md#di-d-42). The alternative put to the engineer, a release queued and made at
the consumer's next call, is not taken.

### <a id="di-912"></a>DI-9.1.2 -- An ended lineage is not live from the moment it ends. *(completed 2026-10-13 13:02:00 -04:00)*

Accepted by the engineer: "agree, not live from the moment it ends." Recorded in
[DI-D-42](DESIGN-NOTES.md#di-d-42).

### <a id="di-913"></a>DI-9.1.3 -- Heals take effect as soon as nothing can delay them. *(completed 2026-10-13 13:02:00 -04:00)*

Accepted by the engineer: "agreed, take effect as soon as nothing can delay them." Covers both
halves of the item: ending a lineage a heal waits for releases the heal there, and a failure with an
empty suspect set heals at once. Recorded in [DI-D-42](DESIGN-NOTES.md#di-d-42).

## Moved 2026-10-13 15:56:00 -04:00 -- DI-9.1.4: the engineer accepts the oracle's weaker rule

### <a id="di-914"></a>DI-9.1.4 -- The oracle drops DI-D-39's tightening, and gains four rules for an ended lineage. *(completed 2026-10-13 15:56:00 -04:00)*

Accepted by the engineer: "just take the drop of the tightening as you suggest", after being told
what it costs. A `Durable` may pass a failure once the consumer has healed it, so the oracle cannot
tell a `Durable` from a seal that started after the heal from one that started before it and
finished after; the core's own tests pin that timing, and the limit is already listed in
[oracle.rs](src/oracle.rs) among what it cannot check. The four ended-lineage rules (reported ended
once; no `Durable` or `Blocked` names it afterwards; no write into it is accepted; no later failure
suspects a write of it) only add checks, and no sequence a correct driver produces is known to
break them. The engineer took the weaker rule rather than a per-lineage entry in the stream; that
entry is parked as `DI-6+.9` in [CHECKLIST.md](CHECKLIST.md) and the decision is recorded in
[DI-D-42](DESIGN-NOTES.md#di-d-42). [CONTRACT.md](CONTRACT.md) was not changed: the rule it states
for a heal is the core's, and is not what was weakened.

## Moved 2026-10-13 15:58:00 -04:00 -- DI-9.1.5: the engineer ratifies the lineage handles' shapes

### <a id="di-915"></a>DI-9.1.5 -- The lineage handles' shapes. *(completed 2026-10-13 15:58:00 -04:00)*

Ratified by the engineer: "ratified." That closes [DI-D-42](DESIGN-NOTES.md#di-d-42) and the
`DI-9.1` group. What was ratified: `Tag<'_, V>` borrows the handle, never the instance; construction
returns `(Dioring, LineageHandle)`, the default lineage's handle beside the instance;
`durable_through`, `sealed_through`, `epoch_state` (no longer panicking) and `import_failure` return
`Result<_, UnknownLineage>`; `EndLineageRefusal` and `RetireLineageRefusal` hand the handle back,
checked in the order `Foreign`, `Default`, `Shared`, then `Busy`; `LineageBusy.in_flight` clears as
completions arrive, not when popped; retiring reports no entry; `LineageEnded.abandoned_through` is
the highest epoch above the lineage's mark that it sealed or wrote.

## Moved 2026-10-13 16:40:00 -04:00 -- DI-9.2.1: the engineer accepts how a held write ends when its own epoch is abandoned

### <a id="di-921"></a>DI-9.2.1 -- A held write whose own epoch is abandoned ends as `NeverIssued` naming that epoch. *(completed 2026-10-13 16:40:00 -04:00)*

Accepted by the engineer: "yes", after the item was reworded to say what a held write is and to
state the tie rule as the contract has it. A held write is one pushed with a gate: accepted by the
core and kept, with nothing at the backend, until the gate's epoch is durable. If its own epoch is
abandoned while it waits, it can never be issued, so it ends at once as `NeverIssued` naming its own
epoch; when its gate is also abandoned, the gate is the one named. Recorded in
[DI-D-43](DESIGN-NOTES.md#di-d-43); the rule is stated in [CONTRACT.md](CONTRACT.md), "Gates".

## Moved 2026-10-13 16:44:00 -04:00 -- DI-9.2.2 and DI-9.2.3: the engineer accepts two more of DI-D-43's choices

### <a id="di-922"></a>DI-9.2.2 -- A held write is outstanding for its lineage's seals, and no failure suspects it until it is released. *(completed 2026-10-13 16:44:00 -04:00)*

Accepted by the engineer: "yes." A seal at or above a held write's epoch waits for it as for a write
in flight, so `Durable` still follows every write it covers; and since nothing was written until it
is released, no failure suspects it before then. Recorded in
[DI-D-43](DESIGN-NOTES.md#di-d-43); stated in [CONTRACT.md](CONTRACT.md), "Gates".

### <a id="di-923"></a>DI-9.2.3 -- `make_durable_through` returns `SealError::{UnknownLineage, GateCycle}` in place of `io::Error`. *(completed 2026-10-13 16:44:00 -04:00)*

Accepted by the engineer: "yes." A public API change for consumers, which now match on the cause
instead of reading an `io::Error`. Recorded in [DI-D-43](DESIGN-NOTES.md#di-d-43).

## Moved 2026-10-13 16:46:00 -04:00 -- DI-9.2.4: the engineer ratifies the gate and seal shapes

### <a id="di-924"></a>DI-9.2.4 -- The gate and seal shapes, and the oracle's gate rules. *(completed 2026-10-13 16:46:00 -04:00)*

Ratified by the engineer: "di-9.2.4: ratified" (first sent as `di-9.3.4`, corrected a minute later).
That closes [DI-D-43](DESIGN-NOTES.md#di-d-43) and the `DI-9.2` group. What was ratified: a gate
naming an ended or retired lineage is refused as `UnknownLineage`; a cycle is listed as each held
write's epoch followed by its gate; released operations are pushed at the consumer's next call on
the instance, normally the `pop` that returns the `Durable` releasing them, because a
registered-span push needs the registration and an owned push its buffer's bound, which a held owned
operation carries as a function pointer; and the oracle gains "a gated operation completes only
after its gate is reported durable", "`NeverIssued` comes only for one", and "no failure suspects a
held write".

## Moved 2026-10-13 16:50:00 -04:00 -- DI-9.3.1 to DI-9.3.3: the engineer confirms the provider choices

### <a id="di-931"></a>DI-9.3.1 -- The provider is called only from the consumer's next call, with dioring's lock released. *(completed 2026-10-13 16:50:00 -04:00)*

Accepted by the engineer: "yes." A seal's request is made by the consumer's next `pop`,
`make_durable_through`, `resolve` or `end_lineage`, never by a completion callback on a pool thread.
Two constraints force it: [DI-D-27](DESIGN-NOTES.md#di-d-27) lets a provider answer inside
`make_durable`, and that answer takes the lock; and a provider panic can only propagate to a caller.
Nothing is stranded, since a seal starts at its own call or at its last write's completion, which
queues an entry the consumer pops. Recorded in [DI-D-44](DESIGN-NOTES.md#di-d-44).

### <a id="di-932"></a>DI-9.3.2 -- Every provider request is built before any is handed over. *(completed 2026-10-13 16:50:00 -04:00)*

Accepted by the engineer: "yes." A panic drops the rest, and each completion dropped unanswered is a
failure of its domain, `Cause::ProviderAbandoned`. Recorded in
[DI-D-44](DESIGN-NOTES.md#di-d-44).

### <a id="di-933"></a>DI-9.3.3 -- The provider request's shape, and when a write is covered. *(completed 2026-10-13 16:50:00 -04:00)*

Ratified by the engineer: "ratified." A request is one per seal, naming only the served domains the
seal's writes touched, in interned-id order, each with its files and each file's writes in push
order; a seal naming no served write asks nothing; a write is covered when its file's flush
succeeded, if the default answers for the file, and every served domain of its file answered
success; a failure of domain D is observed with `Reach::Domains([D])`, which is
[DI-D-27](DESIGN-NOTES.md#di-d-27)'s rule, so needs no reach of its own. This closes
[DI-D-44](DESIGN-NOTES.md#di-d-44), as amended by DI-D-46 and DI-D-50, and the `DI-9.3` group.

## Moved 2026-10-13 16:55:00 -04:00 -- DI-9.4.1 to DI-9.4.4: the engineer confirms the fault, diagnostic and clock choices

### <a id="di-941"></a>DI-9.4.1 -- On Windows the default diagnostic sink is `NoDiagnostics`, until `DI-12.1`. *(completed 2026-10-13 16:55:00 -04:00)*

Accepted by the engineer: "yes." `Setup.diagnostics` is an `Option<Arc<dyn DiagnosticSink>>`, `None`
meaning the driver's default, so an inconsistency the core finds is reported nowhere unless the
consumer supplies a sink. `StderrDiagnostics` exists for one who does, prefixing each line
`dioring:`. `DI-12.1` replaces the default with the ETW sink
([DI-D-51](DESIGN-NOTES.md#di-d-51)).

### <a id="di-942"></a>DI-9.4.2 -- A diagnostic sink is called with dioring's lock held. *(completed 2026-10-13 16:55:00 -04:00)*

Accepted by the engineer: "yes." A sink must return promptly and must never call back into the
instance; one that does deadlocks, and one that is slow holds up every other path.

### <a id="di-943"></a>DI-9.4.3 -- Two `IoFault`s are equal when they name the same source and code and share the one report. *(completed 2026-10-13 16:55:00 -04:00)*

Accepted by the engineer: "yes." Sharing is `Arc` identity, so a nullifier agrees with its write's
completion only if it relays that very report, and a fault rebuilt from the same words is not equal.

### <a id="di-944"></a>DI-9.4.4 -- The fault, diagnostic and clock renames and shapes. *(completed 2026-10-13 16:55:00 -04:00)*

Ratified by the engineer: "ratified." That closes the `DI-9.4` group. What was ratified:
`SuspectWrite::file` and `IoTargetBusy::file` are renamed `target`, and `IoTargetBusy` displays
"target"; the Windows path's fault for a short write carries no code and the message "the write
completed short: n of N bytes"; a write's requested length rides in its sidecar and no longer in the
core; the inconsistency for a code-less failed write is gone, the code having become optional;
`DiagnosticKind` and `Diagnostic` are non-exhaustive, `Inconsistency` carrying the instance's number,
`what` and the formatted detail; the clock trait is `StampClock`, its reading bounded by a public
`Instant` trait (`std::time::Instant` shares the name), the time type an associated type of
`Identities`, with [DI-D-57](DESIGN-NOTES.md#di-d-57) amending how the driver names it.

## Moved 2026-10-13 17:00:00 -04:00 -- DI-9.5.1 to DI-9.5.4: the engineer confirms the backend trait, the in-memory backend and two Windows defaults

### <a id="di-951"></a>DI-9.5.1 -- Registered buffers are given at construction only. *(completed 2026-10-13 17:00:00 -04:00)*

Accepted by the engineer: "yes." They come through `WindowsBackendSetup`, whose refusal hands
everything back as `Setup`'s does; targets given at construction are registered and addressed by
index, and those attached later are addressed by their `SharedHandle`.

### <a id="di-952"></a>DI-9.5.2 -- What a Windows commit and a target claim by default. *(completed 2026-10-13 17:00:00 -04:00)*

Accepted by the engineer: "yes." A commit is an unordered flush in the default mode, as before; a
`Win32FileTarget` defaults to `BestEffort`, unaligned, with no capabilities, its class taken on
declaration until `DI-3.10.4`; and the in-memory backend ignores `WriteCaching`.

### <a id="di-953"></a>DI-9.5.3 -- The backend trait's surface. *(completed 2026-10-13 17:00:00 -04:00)*

Ratified by the engineer: "ratified." `IoBackend`, in a public `backend` module with the in-memory
backend in `memory`; `IoTarget`, `IoTargetDescription`, `WarrantyClass`, `Geometry`, `GeometryError`
and `Capabilities` at the crate root; the target module is `io_target` because the root
`.gitignore`'s `target` ignores any directory so named; `Geometry` holds three alignments (offset,
length, buffer address), each a power of two its constructors enforce, with `admits` and
`admits_buffer` the one statement of the rule; `Capabilities` names write-through only, its fields
private; `IoTarget` requires `Send`, `describe` provided; `SubmissionToken` a public `u64`,
`RegisteredRegion` three `u32`s, a completion's result a `u32` count or an `IoFault`, a commit
transferring zero; refusal causes `SubmitNeeded`, `DrainNeeded`, `SubmitAndDrainNeeded`, `Busy`,
`UnknownTarget`, `Unsupported`, `Invalid`, `Unusable(IoFault)`, `Unknown(IoFault)`, non-exhaustive,
with `needs_submit`, `needs_drain`, `is_retryable`; `submit`, `pop` and `notification` report an
`IoFault`; a submission for a detached target completes with a fault; registration is a backend's
construction concern.

### <a id="di-954"></a>DI-9.5.4 -- The in-memory backend. *(completed 2026-10-13 17:00:00 -04:00)*

Ratified by the engineer: "ratified." Buffers are `Vec<u8>`; it executes at submit; it refuses a
drain when queued and unpopped submissions fill the completion queue, checking the target, then the
submission's shape, then capacity; its notification is a count with `wait_past`; its faults name the
source `"memory"` with no code; queue sizes are `NonZeroU32`; a commit copies bytes to
`durable_contents`.

## Moved 2026-10-13 17:02:00 -04:00 -- DI-9.5.5 and DI-9.5.6: the engineer ratifies the core on the trait and the Windows backend's mapping

### <a id="di-955"></a>DI-9.5.5 -- The core on the backend trait. *(completed 2026-10-13 17:02:00 -04:00)*

Ratified by the engineer: "ratified." A consumer operation takes the caller's push as a closure and
is recorded only once the push accepts it; the core's own submissions come out in `CoreActions` and
return through `refused`, re-emitted with the next actions or by `retry`; `CoreActions` is `submit`
plus a `deliver` flag raised on the queue's empty-to-non-empty transition, `#[must_use]`; a seal
returns its answer and actions as a pair; backend refusals reach the consumer as
`PushRefusal::Backend(RefusalCause)`; a duplicate target hands back its description in
`DuplicateTarget`; `new` takes a clock and an `Arc<dyn DiagnosticSink>` with no default; a completion
with no record, a second completion, and a consumer operation handed back as refused are each an
inconsistency ([DI-D-46](DESIGN-NOTES.md#di-d-46)); the module is `io_core` because `core` would
shadow the `core` crate; `Event::into_entry` is the one event-to-entry conversion; a held operation
spends its id when held and is emitted in the actions of the change that released it, kept and
emitted again if refused; `write` and `read` take `WriteOptions` and `ReadOptions`; lineage calls
take a plain `Lineage`, an ended or retired one refused as `Foreign`; the core holds no provider,
`with_provider` takes the served domains and refuses a repeat as `DuplicateDomain`, requests come
out as `ProviderRequest`s in `CoreActions::requests`, answers go back through `provided` with a
public `DomainOutcome`; `import_reach` is the one function both paths share; `close` hands back the
held operations, failures and lineages; `pushed` and `pushed_gated` take a `length` for every
operation, a read's being its buffer and unchecked, and the short-write rule is exact (a write
transferring more than its length is `WriteNotInFull`, as a shorter one is).

### <a id="di-956"></a>DI-9.5.6 -- The Windows backend's mapping. *(completed 2026-10-13 17:02:00 -04:00)*

Ratified by the engineer: "ratified." That closes the `DI-9.5` group. Each push is issued at once,
the ring submitting a batch as it ends, and `submit` reissues what a failed submission left queued,
so the trait says a push is issued no later than the next `submit`, and `submit` reports a
`RefusalCause`; a full submission queue maps to `SubmitNeeded`, a full completion queue to
`DrainNeeded`, a submission running to `Busy`, a corrupt ring to `Unusable`, any other ring condition
to `Unknown` with the report, and the ring's `Unsupported` and `InvalidInput` to `Unsupported` and
`Invalid`; a target's geometry is checked before the ring is asked; the notification is the ring's
completion event, duplicated as an `OwnedHandle` and attached at construction; `fail_next_commit`
joins `fail_next_flush` in the fault seam; the short-write rule lives in the backend as `transfer`;
`register_files` takes handles rather than `TargetSetup`s; the backend and `Win32FileTarget` live in
a public `windows_backend` module.

## Moved 2026-10-13 17:14:00 -04:00 -- DI-9.6.1 and DI-9.6.2: the engineer confirms two driver-on-core choices

### <a id="di-961"></a>DI-9.6.1 -- What the backend refuses for good, the core ends ([DI-D-54](DESIGN-NOTES.md#di-d-54)). *(completed 2026-10-13 17:14:00 -04:00)*

Accepted by the engineer: "yes." A gated operation is put to `IoBackend::check` before it is held; a
submission of the core's own that the backend refuses for a non-retryable cause completes with a
`REFUSED` fault instead of being re-emitted; an `Unknown` cause counts as permanent. The alternative,
re-emitting every refusal, is the hang the row records.

### <a id="di-962"></a>DI-9.6.2 -- `Dioring` requires `R: Send + 'static`. *(completed 2026-10-13 17:14:00 -04:00)*

Accepted by the engineer: "yes." The registration lives behind the engine's lock, which pool threads
take, so the registered buffer type must be sendable and own its data.

## Moved 2026-10-13 17:57:00 -04:00 -- DI-9.6.3: the engineer accepts the release path's refusals, and it is built

### <a id="di-963"></a>DI-9.6.3 -- A lineage's last-handle release reports a core refusal as not live, and ignores the default lineage's. *(completed 2026-10-13 17:57:00 -04:00)*

Accepted by the engineer, as recommended: "yes I accept it." The question was which refusals of a
lineage's end are reported on the two paths that end one. The explicit end (`Dioring::end_lineage`
taking the last handle) already reported a core refusal as an inconsistency, the driver having
refused the default first. The release (the last copy of a handle dropped on any thread) ignored
every refusal, which the swap onto the core had carried over from the old relay without anyone
choosing it. It now ignores only `Default`, dropping the default lineage's handle being legitimate
and ending nothing, and reports `Foreign` as an inconsistency ([DI-D-46](DESIGN-NOTES.md#di-d-46)): a
handle releases only when no explicit end or retire disarmed it, so its lineage is live and a
refusal as not live means the bookkeeping is wrong. The report is made after the engine's lock is
released, since it panics, and while the thread is already unwinding it is reported only, so a drop
during a panic does not abort.

Built in commit `c98ff3b5`: `Hub::release` in `win-durable-ioring`'s
[hub.rs](../win-durable-ioring/src/dioring/hub.rs); two tests in `dioring/tests/ending.rs`, one for
each direction, the first proving the instance is still usable after the report; and two entries in
that crate's [sabotage.json](../win-durable-ioring/sabotage.json), one for each direction. Before
the change nothing tested a release of a lineage that is not live, so the first sabotage, which
ignores such a release again, would have survived; with it both are caught.

## Moved 2026-10-13 19:47:00 -04:00 -- DI-9.6.4 to DI-9.6.7: the engineer confirms the rest of the driver on the core

### <a id="di-964"></a>DI-9.6.4 -- A declared geometry is now enforced on the driver's path. *(completed 2026-10-13 19:47:00 -04:00)*

Accepted by the engineer: "yes." The backend's `check` and `push` refuse an operation that does not
fit a target's declared geometry, which the old driver did not.

### <a id="di-965"></a>DI-9.6.5 -- The driver's structure. *(completed 2026-10-13 19:47:00 -04:00)*

Ratified by the engineer: "ratified!" A portable `Engine` is the one place `CoreActions` become
backend pushes, with the drain every delivery shares; a drain runs until a pass pops nothing and does
not call `retry`; pool delivery tells the driver through an `on_ready` closure called after the lock
is released; `into_engine` stops delivery and returns the engine; one `Hub` holds the engine's slot,
the readiness event and the count of unanswered provider completions, and is where a provider's
answer and a handle's release arrive; the slot is an `Option` that ending empties; an answer is
counted down before it reaches the engine, which is safe because ending empties the slot before it
waits; the instance's own refusals of a duplicate key and of ending the default lineage stay in the
driver, ahead of the core's; `pop` submits and pops but no longer retries refused submissions, the
retry having been an equivalent mutant.

### <a id="di-966"></a>DI-9.6.6 -- The trait and type shapes. *(completed 2026-10-13 19:47:00 -04:00)*

Ratified by the engineer: "ratified." `Readiness` carries no bound, while `EntryDelivery` and
`check_readiness` bind it to `Event`; the ring trait's `Target` is bounded by `IoTarget`;
`add_target_with` goes with `FileOptions`, leaving one `add_target`; `AddTargetError` is generic,
carrying the target; the test fake's target is `MemoryTarget`; `TargetSetup` carries the
`Win32FileTarget` whole; two constructors, `new` for an `IoBufMut` buffer and `without_owned_reads`
for one that may be only `IoBuf`, with a crate-private `enable_owned_reads`; the owned read's push is
a function pointer, behind a `Via`; accessors are renamed `with_registered_buffer` and
`with_registered_buffer_mut`, generic over the closure's return, so the trait is not object-safe, as
it was not before ([DI-D-24](DESIGN-NOTES.md#di-d-24)); `Setup` has two lifetimes,
`Setup<'env, 'pool, ..>`, because a callback environment is invariant behind `&mut`;
`RefusalCause` implements `std::error::Error`; the ring crate gained a public `check_span` rather
than dioring repeating the bounds.

### <a id="di-967"></a>DI-9.6.7 -- The sabotage migration. *(completed 2026-10-13 19:47:00 -04:00)*

Ratified by the engineer: "ratified." That closes the `DI-9.6` group. Thirty-three of the old path's
sabotages were re-anchored where their rules now live and ten retired: nine sabotage the same
predicate at the same site in the same way as an existing core or backend entry, and one was the
relay's control, retired with the relay.

## Moved 2026-10-13 20:30:00 -04:00 -- DI-9.7.3: the engineer rejects a panic and an abort for an allocation that fails

### <a id="di-973"></a>DI-9.7.3 -- An allocation that fails is an error the caller receives, not a panic or an abort. *(completed 2026-10-13 20:30:00 -04:00)*

The item as the assistant wrote it asked whether "an unallocatable request is a panic where it was an
abort" -- the effect of `IB-2+.4`'s route (a), which took `AlignedBuf`'s allocation error with `expect` in
`windows-namespace-request-sys`. The engineer's answer was that neither is acceptable: "I do not find
either acceptable", after being told what a panic and an abort are and that the one site where a caller picks
the size is where a recoverable error matters most. This was **not** that crate's recorded policy; it was a
side effect of the move, and a wider one than the item named. A survey found the same shape in a second place,
`PageBuffers::new` in `windows-overlapped-io-sys`, whose `IB-2+.2` move had added a panic its `# Panics` list
did not mention. Both are fixed: every allocation of an `AlignedBuf` outside tests returns its failure.
[NR-3.1](../windows-namespace-request-sys/COMPLETED-CHECKLIST.md#nr-31) and
[M15.1](../windows-overlapped-io-sys/COMPLETED-CHECKLIST.md#m151) record the changes; the decision is in
`windows-namespace-request-sys`'s [D-8](../windows-namespace-request-sys/DESIGN-NOTES.md#d-8) and
`windows-overlapped-io-sys`'s notes. Both are breaking changes to published crates:
`SecurityDescriptor` and `SecurityAttributes` lose `Clone` for `try_clone`, and `PageBuffers::new` returns an
`io::Result`.

## Moved 2026-10-13 20:41:00 -04:00 -- DI-9.7.1, DI-9.7.2 and DI-9.7.4: the engineer approves three choices from DI-M7 on

### <a id="di-971"></a>DI-9.7.1 -- `pump` on a pool-built instance is a runtime `Unsupported`, not a type error. *(completed 2026-10-13 20:41:00 -04:00)*

Approved by the engineer. A pump that waited would be a second waiter on the ring's event, which the ring
crate's [D-21](../windows-ioring-sys/DESIGN-NOTES.md#d-21) forbids, so the call refuses; the crate split kept it
as it was. The alternatives, a type parameter on `Dioring` or a type per delivery mode, were rejected as churn
through every bound. Recorded in [DI-D-55](DESIGN-NOTES.md#di-d-55).

### <a id="di-972"></a>DI-9.7.2 -- `completion_signal` hands out a full-access duplicate of dioring's own auto-reset event. *(completed 2026-10-13 20:41:00 -04:00)*

Approved by the engineer. The consumer may wait on it, set it or reset it, and the worst that costs is its own
wakeup, since a pump drains before it waits. The wait-only duplicate that `win-sync-sys`' `Event::try_clone_wait_only`
([WS-D-7](../win-sync-sys/DESIGN-NOTES.md#ws-d-7)) now makes possible is therefore not handed out here; it
remains available to `DI-7+.1`, which is about the ring's own event and is unchanged. Recorded in
[DI-D-55](DESIGN-NOTES.md#di-d-55).

### <a id="di-974"></a>DI-9.7.4 -- `NumaBuffer`'s buffer impls move from the ring crate to `win-numa-sys`. *(completed 2026-10-13 20:41:00 -04:00)*

Approved by the engineer. `win-numa-sys` then implements a trait it does not define, ending its deliberately
trait-free stance in that one respect ([IB-D-4](../io-buffers/DESIGN-NOTES.md#ib-d-4)). Recorded in `win-numa-sys`'
[N-D-2](../win-numa-sys/DESIGN-NOTES.md#n-d-2).

## Moved 2026-10-13 20:43:00 -04:00 -- DI-9.7.5: the engineer approves the portable crate's public surface for other drivers

### <a id="di-975"></a>DI-9.7.5 -- The portable crate's public surface for other drivers. *(completed 2026-10-13 20:43:00 -04:00)*

Approved by the engineer. A public `driver` module re-exports `Engine`, `EngineGuard`, `Slot`, `lock`,
`present`, the pump types, `Home`, `Answer`, `DomainCall`, `InstanceId`, `first_repeat` and `inconsistent`;
`Lineage` and `OpId` have public fields; `LineageHandle::detached` and `mint` are public; `Waitable` is a
public one-method trait so a backend of another crate can implement it; and the Windows crate re-exports the
portable crate's API with `pub use durable_ioring::*`. Recorded in [DI-D-57](DESIGN-NOTES.md#di-d-57).

## Moved 2026-10-13 20:47:00 -04:00 -- DI-9.7.6: the engineer ratifies the split's renames, and the clock's bounds are recorded

### <a id="di-976"></a>DI-9.7.6 -- The renames the split forced, and the clock's bounds. *(completed 2026-10-13 20:47:00 -04:00)*

Ratified by the engineer, who also asked that the analysis of the clock's bounds be documented. The renames:
the driver's clock is a `StampClock`, not a `win-time-sys` clock, with `InterruptStamps` the default and
`FromClock` the adapter; the notification is `CompletionEvent`; `ErrorCode::of` is `error_code_of`;
`DioringIds` has no default time type and the driver names it `WindowsIds`. The analysis, asked as whether
`StampClock` needs bounds beyond itself: it does not. No marker for "never decreases" is possible or useful;
the one unenforced guarantee is that the core does not check a reading did not go backwards, and the option of
clamping it is recorded as a working position, not a decision, scheduling nothing. Both are in
[DI-D-57](DESIGN-NOTES.md#di-d-57), and the not-checked fact is now in
[`StampClock`'s documentation](src/clock.rs).

## Moved 2026-10-13 20:51:00 -04:00 -- DI-9.7.7 and DI-9.7.8: the engineer ratifies the delivery modes and construction

### <a id="di-977"></a>DI-9.7.7 -- Delivery modes. *(completed 2026-10-13 20:51:00 -04:00)*

Ratified by the engineer. `Setup`'s `environment` became `delivery: DeliveryMode` (`Pool`, `Pumped`,
`Signalled`), touching every construction site and `CONTRACT.md`'s three worked examples, over a flag or a
second constructor; `Dioring::pump` returns `io::Result<bool>`, whether entries became poppable; a pump in
either pumped mode drains before it waits and returns at once if that delivered, so `waited` is `false` when no
wait was made; a wait that failed or timed out drains too, and a failed one then reports; the readiness event
is still set under pumped delivery; the signalled mode's event is auto-reset, since a manual-reset one would
stay set and a pump's wait would return at once for ever; the blocking wait is `win-sync-sys`'
`wait_for_single_object`, its first slice of the parked waits
([WS-D-6](../win-sync-sys/DESIGN-NOTES.md#ws-d-6)); the engine's slot helpers moved to `engine.rs`; the delivery
enum is in `dioring/delivery.rs` and `SignalledDelivery` in `windows_backend/signalled.rs`; `MemoryNotification`
keeps a per-handle record of the raisings its own waits have seen, beginning at none. Recorded in
[DI-D-55](DESIGN-NOTES.md#di-d-55), which this confirms whole.

### <a id="di-978"></a>DI-9.7.8 -- Construction, the fault responder, and their tests. *(completed 2026-10-13 20:51:00 -04:00)*

Ratified by the engineer. The responder that fails submits is asked of the ring crate as a safe
`sys::submit_faults` (its `M37.1`) rather than implemented behind `expect(unsafe_code)` or a macro, per
[DI-D-4](DESIGN-NOTES.md#di-d-4); tests reach it through a `cfg(test)` `WindowsBackend::submit_faults`,
`windows-ioring-sys` being a dev-dependency enabling `kernel-seam`; the pool-thread test holds the engine's
lock across both pushes so the failing drain is guaranteed; the failure armed is `ERROR_IO_DEVICE`, surfacing
as `RefusalCause::Unknown`; construction's two knobs travel in one crate-private `Construction` rather than a
shortened constant under `cfg(test)` or a thread-local; a `RingHook` alias names the hook's type; the
construction tests are two sibling modules; a stalled registration is tested with a 50ms bound against a 600ms
stall, asserting on the bound the error names and on the ring running down, not on elapsed time; the ring
crate's `M38.1` is the stalled submit ([D-87](../windows-ioring-sys/DESIGN-NOTES.md#d-87)). Recorded in the ring
crate's [D-86](../windows-ioring-sys/DESIGN-NOTES.md#d-86) and D-87, which this confirms.

## Moved 2026-10-13 21:00:00 -04:00 -- DI-9.7.9 and DI-9.7.10: the engineer ratifies the example, the sibling crates and the buffer crate

### <a id="di-979"></a>DI-9.7.9 -- The example and the sibling crates. *(completed 2026-10-13 21:00:00 -04:00)*

Ratified by the engineer. `blocking_push` is an example, not an API ([DI-D-56](DESIGN-NOTES.md#di-d-56)): it takes
the buffer and context by value and the push as a closure, backs off in sleeps of at most a millisecond under
the pool, excludes a registered-span push, runs its tests with the crate's, and has its sabotages in this crate's
manifest. `win-sync-sys`: `duplicate_handle_with_access` over any handle with a typed `Access` mask;
`WaitOnlyEvent` is a type, not an `Event` that fails at run time, and guards against misuse without being a
boundary against a holder that duplicates it wider; alertability is an `Alertable` argument; the waits stay over
`AsHandle` with no `Waitable` trait, which the item had proposed; wait for any and wait for all are two
functions; a `Signalable` trait says what `SignalObjectAndWait` may signal; `queue_user_apc` takes a Rust closure.
Recorded in `win-sync-sys`' [WS-D-7](../win-sync-sys/DESIGN-NOTES.md#ws-d-7) and
[WS-D-8](../win-sync-sys/DESIGN-NOTES.md#ws-d-8).

### <a id="di-9710"></a>DI-9.7.10 -- The buffer crate and the core's tests. *(completed 2026-10-13 21:00:00 -04:00)*

Ratified by the engineer. That closes the `DI-9.7` group. The crate's name, `io-buffers`; `RegisteredSpan` moves
into it with the contract; the ring crate re-exports the contract at the old paths
([D-89](../windows-ioring-sys/DESIGN-NOTES.md#d-89)); `AlignedBuf` is fallible to build, has no `Clone` and cannot
grow ([IB-D-6](../io-buffers/DESIGN-NOTES.md#ib-d-6)). From `DI-8.1`: the core's tests share one portable clock
and fault in `test_support`; a stamp in them is a plain `u64`; the test of the driver's default clock stamping
with interrupt time stays in the driver's own tests; the readiness-check tests, being over a Windows event, sit
in a module of their own. Recorded in `io-buffers`' [IB-D-1](../io-buffers/DESIGN-NOTES.md#ib-d-1) and
[IB-D-2](../io-buffers/DESIGN-NOTES.md#ib-d-2).

## Moved 2026-10-13 21:12:00 -04:00 -- DI-9.8.1: the declared blind spots are covered, or say what would cover them

### <a id="di-981"></a>DI-9.8.1 -- Each declared blind spot is covered, or records what would cover it. *(completed 2026-10-13 21:12:00 -04:00)*

Done by the assistant, as the item was its own. Of the gaps the Windows driver's sabotage manifest declared:

- **Which pool a delivery's callback runs on** was declared unseeable because either pool runs the drain.
  It is covered for pool delivery and for signalled delivery: a test holds the only thread of the pool a
  callback environment names, so a delivery bound to that pool must wait and one on the default pool does
  not. The earlier reasoning, that a single-thread pool's repeated thread proves nothing, was about
  repetition; a thread held is a different observation. Three sabotages, at the backend's wake and at each
  delivery's call to it, are now caught.
- **The notification's wait failing, and `Dioring::pump` reporting it** were declared unreachable because
  a handle without `SYNCHRONIZE` access needed `unsafe`. `win-sync-sys`' `duplicate_handle_with_access`,
  which did not exist when `DI-7.3` declared it, makes one safely, so both are tested: the notification's
  own wait, and an instance given such a notification. The pump drains before it waits, so the error shows
  only when the drain delivers nothing, and the second test says so. Two sabotages are now caught.
- **Not covered, with what would cover them:** the wait ending *abandoned* needs a mutex to abandon, which
  is [WS-1+.3](../win-sync-sys/CHECKLIST.md); and the creation failures (events, thread-pool waits,
  duplicating the notification) need a fault seam in `win-sync-sys` and `windows-threadpool-sys`. Both are
  parked as [DI-6+.10](CHECKLIST.md), and the manifest's `notCoveredHere` says each.

Changed in `win-durable-ioring`: a test-only [held_pool.rs](../win-durable-ioring/src/held_pool.rs), four
tests, and the manifest.

### <a id="di-7+1"></a>DI-7+.1 -- A wait-only handle on the ring's own completion event, offered by a pumped instance as `Dioring::completion_event`. *(completed 2026-10-13 21:45:20 -04:00)*

Built as a second accessor, `Pumped` only, beside `completion_signal`; a fourth `DeliveryMode` or an option
on `Pumped` was rejected because either would configure nothing. The handle is a duplicate of the ring's
event with `SYNCHRONIZE` access alone, made by `win-sync-sys`' `duplicate_handle_with_access`
([WS-1+.5](../win-sync-sys/COMPLETED-CHECKLIST.md#ws-1+5)), and is an `OwnedHandle` rather than a
`WaitOnlyEvent` because a bare handle cannot be shown to be an event
([WS-D-4](../win-sync-sys/DESIGN-NOTES.md#ws-d-4)). Pool and signalled instances refuse it as
`Unsupported`. The decision, its edge-sharing hazard (wait and pump from one thread) and the bend of the
ring's one-waiter rule are in [DI-D-55](DESIGN-NOTES.md#di-d-55).

- **Declared blind spot:** widening the access mask survives the sweep, because setting an event is
  `unsafe` and this crate forbids it; `win-sync-sys` asserts the kernel's refusal for the same function.
- Four sabotage entries: three caught, the blind spot declared.

### <a id="di-7+2"></a>DI-7+.2 -- The engine rings the doorbell, a backend's push only queues, and a failed doorbell or an unreadable ring reaches the consumer once, with no timer. *(completed 2026-10-13 23:35:50 -04:00)*

From the review of [PR 126](https://github.com/MikeGrier/windows-threadpool-sys/pull/126), three findings that
were one gap, decided by the engineer in the session that followed; the decision and its reasons are
[DI-D-58](DESIGN-NOTES.md#di-d-58), and this records what was built.

- **The ring crate** gained `Batch::leave_queued` ([M40.1](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m401),
  [D-90](../windows-ioring-sys/DESIGN-NOTES.md#d-90)), because a batch submits as it drops.
- **The engine** rings the doorbell once at the end of `act`, and after a consumer's own write or read; a
  push refused for want of room is rung for and pushed once more. A doorbell that fails, in `act`, in
  `drain` or after a consumer's push, is reported as something to deliver, and `Engine::take_attention`
  tells the driver of a consumer's push. A drain whose backend cannot read its completions ends and keeps
  the first fault (`Engine::take_fault`), until a later read works.
- **The Windows backend's push** only queues, in a batch left queued, whether it is accepted or refused;
  `submit` issues what was queued. `Dioring` raises the readiness signal once for a consumer's push whose
  doorbell failed, and `pop` reports a fault the ring could not be read with, once, as an `IoFault`, which
  is now an `Error`. A pop's cause is reported only when it has no entry to return.
- **Contracts**: `IoBackend` says a push only queues and the engine rings; `detach` no longer promises a
  fault for a submission a backend cannot take back, which completes as pushed, exactly once.
  [DI-D-28](DESIGN-NOTES.md#di-d-28) allows the signal for a pop with a failure to report.
- **What a pool instance cannot do**: read a ring that could not be read. The ring's event is an edge, the
  wake that could not read took it, and the completion it left keeps the queue from going from empty to
  non-empty again; a pumped instance's next pump reads. A test records each.
- **Tests**: the engine's, over a scripted backend, in
  [doorbell.rs](src/engine/tests/doorbell.rs); the Windows backend's; and the driver's in
  [submits.rs](../win-durable-ioring/src/dioring/tests/submits.rs), over real files with the ring crate's submit
  and call responders. Sabotage entries for the engine, the driver and the backend, all caught; one engine
  entry removes both of two redundant clearing sites at once, which `why` says. Entries that named the old
  eager push were rewritten, and one whose pattern had stopped being unique since `DI-7+.1` was fixed.

### <a id="di-7+3"></a>DI-7+.3 -- The Windows backend's ring is an owned one: its pop has no case of a completion the ring held nothing for, and the panic and error rule is recorded. *(completed 2026-10-14 17:17:20 -04:00)*

Raised by the engineer's question of whether `WindowsBackend::pop`'s "a completion for an operation the
backend did not push" was a fault in the backend or in what it is layered on: it was the backend's, since
the ring reports a completion it held nothing for as a legitimate outcome for a caller who queues one, and
this crate queues none after construction. Decided in [DI-D-59](DESIGN-NOTES.md#di-d-59), with the converse
of [DI-D-46](DESIGN-NOTES.md#di-d-46): a panic is for a programming error, never for normal error reporting.

- The ring crate's `M41.1` ([D-91](../windows-ioring-sys/DESIGN-NOTES.md#d-91)) types a ring by whether it can
  have such an operation in flight.
- `WindowsBackend::new` seals its ring with `into_owned` once its registrations are made and waited on, the
  field is `IoRing<_, _, Owned>`, and `pop` takes the flat `(completion, buffer, sidecar)`: the branch that
  returned an error is deleted. A registration still in flight at the seal is an inconsistency and
  panics; no test can reach it, since making one needs `unsafe`, so the manifest's `notCoveredHere` says so.
- The manifest's `notCoveredHere` no longer says a failing pop and a full submission queue are
  untestable: a failing pop is reached through the ring crate's call responder, and a push no longer submits.
### <a id="di-7+4"></a>DI-7+.4 -- Model A's handler is told when the instance's pop fails, and the in-memory backend refuses what it cannot hold. *(completed 2026-10-14 18:04:18 -04:00)*

From the second round of the review of [PR 126](https://github.com/MikeGrier/windows-threadpool-sys/pull/126).

- **Model A.** `EntryDelivery`'s deliverer collapsed a failed `pop` into an empty queue. After
  [DI-D-58](DESIGN-NOTES.md#di-d-58) a failed pop is how a doorbell that failed, or a ring that could not be
  read, reaches the consumer, and the wake that took the readiness edge is the only thing that told it, so the
  report was lost. The handler now receives a `DeliveryEvent`, an entry or a failure, which is told once
  per wake; delivery goes on. The type forces a handler to choose what to do with one. Tested with a fake
  whose pop fails, and two sabotage entries.
- **The in-memory backend.** A write ending past `isize::MAX`, which a `Vec` cannot hold, is refused as
  malformed at check and push; a write the target cannot find memory for completes with a fault and its
  buffer, as every allocation that can fail is reported here and not aborted on; and `with_registered`
  panics if an operation is queued or a completion is waiting, since replacing the buffers would
  invalidate a region it names, a caller's programming error ([DI-D-59](DESIGN-NOTES.md#di-d-59)). Three
  sabotage entries.
## Moved 2026-10-14 19:26:34 -04:00 -- DI-9.8.2: the portable job's first CI run

### <a id="di-982"></a>DI-9.8.2 -- The portable CI job's first run is read: the crate's tests run on Linux and macOS. *(completed 2026-10-14 19:26:34 -04:00)*

Draft PR 126 gave the branch a pull request, which is what [ci.yml](../../.github/workflows/ci.yml)'s trigger waits for. Its `portable` job builds, tests and lints `wtf-string`, `io-buffers` and `durable-ioring` on `ubuntu-latest` and `macos-latest`, and both runs passed; the checks are listed on the pull request as `wtf-string (non-Windows)`, a name the job shares with its first crate. Nothing in the crate needed to change for a non-Windows host.
