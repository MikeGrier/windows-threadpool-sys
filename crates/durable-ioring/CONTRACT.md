# The durable-ioring contract

This is dioring's specification: what it guarantees, what it does not, what it requires, and what
it assumes. It was written before any code, as the `epoch_log` sample's
[contract.rs](../windows-ioring-sys/examples/epoch_log/contract.rs) was, and the code that comes
later has to satisfy it rather than define it. Where a dependency ever behaves differently from what
is written here, the dependency is wrong and is constrained, wrapped, or replaced; this file does not
change to match it.

It is written in terms of durability, not in terms of how Windows' ring happens to behave
([DI-D-3](DESIGN-NOTES.md#di-d-3)). The decisions behind each part are in
[DESIGN-NOTES.md](DESIGN-NOTES.md), and the reasoning in
[DESIGN-SESSION-2026-10-05-epoch-ring.md](../../design-sessions/DESIGN-SESSION-2026-10-05-epoch-ring.md).

**Names are illustrative.** The API's concrete shape is [API.md](API.md); names there are still
placeholders. The behaviour is what this document fixes; the spelling is not.

## The guarantee, in one sentence

**When dioring reports that a lineage is durable through epoch id n, every write in that lineage
tagged at or below n that completed successfully is on stable media -- unless the consumer abandoned
the epoch it belongs to -- for as long as the device keeps what it was told to keep.**

Everything below either sharpens one word of that sentence or states a condition it rests on.

## Terms

- **Instance.** One dioring ring. It owns its Windows ring outright; the consumer reaches that ring
  only through dioring ([DI-D-10](DESIGN-NOTES.md#di-d-10)).
- **Observation order.** The single sequence of events an instance itself performs or sees: each
  operation it pushes to its ring, and each completion it pops from it. "Before" and "after" in this
  document always mean positions in this sequence -- never wall-clock time, device time, or anything
  about when an event happened in the storage beneath.
- **Lineage.** One independent durability sequence within an instance: its own epoch space, seal
  point, high-water mark and set of failures ([DI-D-19](DESIGN-NOTES.md#di-d-19)). An instance has
  a default lineage and may mint more at any time. Every rule in this document about epochs, seals
  and the high-water mark applies within one lineage.
- **Epoch, epoch id.** Every write carries an epoch: a lineage plus an **epoch id** the consumer
  chooses, of a type the consumer chooses (a `u64` by default) -- a journal transaction ID or a log
  sequence number can be the id directly. "Id" rather than "number", because it need not be one.
  An epoch is the set of writes in one lineage carrying one id. Ids may be issued in any order,
  higher and lower intermixed. (A *tag* below means a write's epoch.)
- **Scope of an epoch id: one lineage.** Every statement in this document about an epoch -- sealed,
  durable, abandoned, the high-water mark -- is about that id *in that lineage*. The same id in two
  lineages, or on two instances, names unrelated epochs. dioring relates lineages of one instance
  only where the consumer asks it to, through a gate that names another lineage's epoch. Any meaning
  a consumer gives an id across instances, and any join between instances, is the consumer's
  ([DI-D-14](DESIGN-NOTES.md#di-d-14)).
- **Seal.** `make_durable_through(n)` *seals* every epoch of n's lineage at or below n: no further
  write may join them, and dioring is asked to make them durable. Epochs above n remain open, and
  other lineages are untouched.
- **High-water mark** (of durability). `durable_through(lineage)`, the highest n for which dioring
  reports every epoch of that lineage at or below n durable or abandoned. Also announced by the
  `Durable { through: n }` event, which names its lineage.
- **Durability provider.** What makes completed writes durable: dioring's built-in default, which
  flushes each file through the instance's own ring, and at most one provider the consumer supplies,
  serving the flush domains it states when the instance is built. A provider may flush by any
  means, and answers once for each flush domain it is asked about
  ([DI-D-27](DESIGN-NOTES.md#di-d-27)).
- **Covered.** A write is *covered* once dioring has observed it complete and named it to the
  durability providers responsible for its file's flush domains. It is *covered successfully* once
  each of those domains has answered success for a request that named it, and is then durable. A
  flush that merely happens to reach a write dioring did not name covers nothing.
- **Flush regime.** The set of writes whose fate one flush failure can affect. It is at least
  everything in the same device's volatile cache, which includes other processes' writes to that
  device; it may be larger. dioring cannot observe it.
- **Durability failure.** A flush that failed, or an external failure the consumer imported. Nothing
  else: a failed or short *write* is an ordinary operation error ([DI-D-12](DESIGN-NOTES.md#di-d-12)).
- **Suspect set.** The writes a durability failure puts at risk -- defined below.
- **Resolve.** What the consumer does with a durability failure: **heal**, **abandon**, or **close**.
- **Gated operation.** An operation the consumer submits for release only once a named epoch is
  durable (release-after-durable). dioring holds it; the kernel does not see it until it is released.

## What this contract guarantees

1. **Durable means on stable media, for what succeeded.** A durable epoch's writes that completed
   successfully are on stable media, up to the byte count each completion reported. A write that
   failed, or transferred fewer bytes than asked, is reported on its own completion and is not
   covered by any durability report for the bytes it did not transfer.
2. **The prefix property.** Durability is reported only as a prefix of a lineage: "durable through
   n" covers every epoch of n's lineage at or below n, never n alone
   ([DI-D-9](DESIGN-NOTES.md#di-d-9)).
3. **Finality.** Once dioring has reported a write durable, no later event retracts the report. A
   later failure's suspect set never reaches back past a write already covered successfully.
4. **Every durability request is answered.** Each `make_durable_through(n)` produces `Durable`,
   `Failed`, or `Blocked` as soon as the writes it covers have completed and the durability
   providers it needs have answered ([DI-D-13](DESIGN-NOTES.md#di-d-13)). dioring itself never
   stalls a request silently; a request waiting on a provider that has not answered stays pending,
   and that wait is reported through dioring's ETW events ([DI-D-26](DESIGN-NOTES.md#di-d-26)).
5. **Durable follows the writes it covers.** `Durable { through: n }` is delivered after the
   completion of every write of n's lineage tagged at or below n. Nothing further is promised about
   order between lineages ([DI-D-18](DESIGN-NOTES.md#di-d-18)).
6. **A late write is refused, not absorbed.** Once n is sealed, a write tagged at or below n in n's
   lineage is an API violation, refused before anything is reserved; nothing reaches the kernel and
   the buffer is returned.
7. **Asking again about a sealed epoch reports its state.** `make_durable_through(m)` for m at or
   below an already-sealed number is a no-op that reports m's current state -- durable, pending,
   failed, or abandoned.
8. **Failures are reported, inventoried, and left to the consumer.** Every durability failure is
   delivered as a `Failed` event carrying its suspect set, stays in the inventory until resolved, and
   is resolved only by the consumer. dioring never resolves one on its own and never judges whether a
   repair was sufficient.
9. **Gated operations wait for durability, not for resolution.** A gated operation on epoch n is
   released when the high-water mark of n's lineage reaches n and n is durable. The gate may name
   any lineage of the instance, including another than the operation's own. If n is abandoned, the operation fails
   with an error saying it was never issued, so the consumer knows no bytes were written.

## What this contract does not guarantee

Stated as plainly as the guarantees, because a contract that lists only its promises gets read as
promising everything.

- **No per-write durability.** A write completing means the device accepted its bytes, not that they
  will survive. Durability is a property of a sealed epoch and nothing smaller.
- **No ordering within an epoch, and none between overlapping writes.** Two writes whose ranges
  overlap and are in flight together land in an unspecified order. Ordering them is the consumer's
  responsibility ([DI-D-6](DESIGN-NOTES.md#di-d-6)). A read that overlaps an in-flight write is
  likewise unordered.
- **Nothing about writes outside the seal.** A write of an epoch above the seal point, or of another
  lineage, may be made durable by a commit incidentally; that is not incorrect, only a cost, and it
  does not make the write covered. It is not reported durable until its own lineage seals through
  its epoch, and it may appear in a failure's suspect set.
- **No bound on what a commit waits for.** A commit waits for its own lineage's writes and its
  providers' answers ([DI-D-22](DESIGN-NOTES.md#di-d-22)), and may also be delayed by unrelated work
  sharing the instance -- submission-queue capacity, the deliverer, dioring's lock, or the ring's
  barrier where dioring chooses to use it. That is a cost, not a semantic, and dioring reports it
  ([DI-D-26](DESIGN-NOTES.md#di-d-26)).
- **No detection of failures nobody reports.** A failure in the instance's flush regime that is not
  reported to it -- another process's failed flush, the system's own failed write-back -- is invisible
  to it. A `Durable` reported in that window may be false, and the import cannot retract it.
- **No timeout.** A failure the consumer never resolves stalls the high-water mark indefinitely; the bound is
  the consumer's ([D-67](../windows-ioring-sys/DESIGN-NOTES.md#d-67)). Likewise a durability
  provider that never answers leaves its seal pending; dioring sets no timeout on it
  ([DI-D-27](DESIGN-NOTES.md#di-d-27)).
- **No atomicity beyond the device's.** A write larger than the device's power-fail atomic unit can
  tear across power loss. dioring does not query that unit.
- **Nothing across a process lifetime**, except that what was reported durable was durable.
  Unresolved failures, inventories and gated operations end with the instance; recovery after a crash
  is the consumer's own.
- **No cancellation in v1** (DI-2.6, parked as DI-6+.4).

## What this contract requires of the handle

- **Positioned, overlapped I/O.** Every write names its own offset; the ring's operations are
  asynchronous.
- **That a completed flush has reached stable media.** A warranty the caller gives about the
  device, on which the built-in default provider rests. Nothing in dioring can verify it.

dioring does not otherwise inspect or gate on how the handle was opened -- the `epoch_log` sample's
contract records why a pre-flight check points the wrong way. Files are given to dioring in owning
forms, never as a raw handle ([DI-D-4](DESIGN-NOTES.md#di-d-4)), and named in failure reports by an
identity the consumer supplies, so a report stays usable after its handle is gone
([DI-D-11](DESIGN-NOTES.md#di-d-11)).

## What this contract requires of the consumer

- **No write at or below a sealed epoch** (guarantee 6 refuses it).
- **An epoch-id type whose ordering dioring can rely on.** Its `Ord` must be a total order that stays
  consistent for the life of the instance, and its values must never wrap: a sequence compared with
  wraparound is not a total order and must first be mapped onto one that does not wrap. dioring only
  compares and stores epochs, so it trusts this ordering completely; a wrong one cannot cause memory
  unsafety, but it makes every durability report wrong.
- **One author per epoch space.** Because a seal covers every id at or below it, a lineage's epoch
  space must have a single author. Independent counters use separate lineages, or share one
  allocator; splitting the ids between them (one even, one odd) does not work, since a seal cuts
  across any split. Lineages separate durability, not everything: the lineages of one instance
  still share its ring, queues, delivery, lock and lifecycle. **A client that wants no interaction
  at all uses a separate instance** ([DI-D-19](DESIGN-NOTES.md#di-d-19)).
- **Ordering of overlapping writes**, as above.
- **Resolving failures.** Heal, abandon, or close each one; the high-water mark waits until it does.
- **Honest heals.** A heal is the consumer's assertion that it has re-issued, under higher tags,
  everything the failure put at risk that it needs. dioring cannot check it.
- **Reporting outside failures it learns of**, through the import, when other writers share its flush
  regime. Without that, guarantee 1 rests on an assumption that does not hold for it.
- **Truthful provider answers, if it supplies a provider** ([DI-D-27](DESIGN-NOTES.md#di-d-27)).
  A domain's success means every write named for that domain is durable, however the provider got
  there. dioring cannot check it; a false success is the second way, beside an incomplete
  declaration below, that the consumer can make this contract false. The provider returns promptly
  from each call, answers each domain once, and may answer from any thread at any time.
- **Complete flush-domain declarations, if it declares any** ([DI-D-21](DESIGN-NOTES.md#di-d-21)).
  A file's declared flush domains must include **every** domain its writes depend on: each disk a
  striped or spanned volume covers, the backing file's domains for a virtual disk, the share for
  a network path. dioring cannot check this.
  - **What a missing domain costs.** A flush failure in an undeclared domain does not reach the
    file, so its uncovered writes are left out of the suspect set and can be reported durable when
    they are not. This is the one way a declaration can make this contract false.
  - **What is always safe.** Declaring nothing: the file is unknown and shares fate with every
    file, which is the contract's default. Declaring *more* domains than a file depends on: an
    extra domain only widens which files share a failure, so it costs precision, never
    correctness.
  - **Example.** A file on a volume striped over disks A and B is declared `{A, B}`. A file on
    disk B alone is declared `{B}`. A failed flush on either reaches the other, because the sets
    intersect at B. Had the first been declared `{A}` only, a failure on B would never reach it.

## What this contract assumes

- **The device honours the flush.** When the operating system asks it to commit its volatile cache,
  it does. A device that loses cached data without failing a flush defeats this contract and every
  other built on the same primitive.
- **Every failure affecting the instance's flush regime is reported to it**, by its own ring or
  through the import. "Durable" is truthful relative to what the instance was told, and no more.
- **A durable write stays durable.** Media failure after the fact is out of scope.

## How a seal is made durable

Once `make_durable_through(n)` is called, dioring waits until every write of n's lineage at or below
n has completed -- gated writes once released -- and then names them to the durability providers
responsible for their files' flush domains ([DI-D-22](DESIGN-NOTES.md#di-d-22),
[DI-D-27](DESIGN-NOTES.md#di-d-27)). It never asks a provider about a write it has not seen
complete.

- The **built-in default** flushes each of its files through the instance's ring.
- A **consumer provider** is given, for each flush domain it serves, the files and the identities
  of the writes named to it, and answers each domain once.
- The seal's epochs are durable when every write in them is covered successfully: every domain of
  every file they wrote has answered success.

A write whose gate could never be released, because releasing it already depends on the write's own
epoch, is refused when it is pushed ([DI-D-23](DESIGN-NOTES.md#di-d-23)).

## Failures in detail

### What a failure puts at risk

A failure's **suspect set** is every write the instance pushed **before the failure was observed**
that has not been covered successfully. Three consequences:

- It includes writes still in flight when the failure arrived, and writes pushed after the failing
  flush itself, which the ring does not hold back. A bound based on *completion* order would be
  unsound: a write accepted before the failure can have its completion posted after it.
- It is **frozen** when the failure is observed and never grows. A write pushed afterwards was
  accepted after the failure, and is covered successfully later in the ordinary way.
- It is defined by push order, not by tag. So failures and epochs are **many-to-many**: one failure
  can span a sealed epoch, a pending one and an open one, and one open epoch can sit in several
  failures. The example at the end of this document shows both.

**Which files a failure reaches.** dioring cannot see which writes share a device's volatile cache,
so it relies on what the consumer declares ([DI-D-21](DESIGN-NOTES.md#di-d-21)):

- A file may be given a set of **flush domains** when it is given to dioring -- the disks, caches,
  virtual disks or shares its writes depend on. The set is fixed for the file's life in the
  instance.
- A failed flush by the built-in default on file F reaches the uncovered writes to every file
  whose declared domains **intersect** F's.
- A consumer provider answers per domain, so its failure of domain D -- including a domain it
  never answered -- reaches the uncovered writes to every file whose declared domains **contain**
  D ([DI-D-27](DESIGN-NOTES.md#di-d-27)).
- A file declared with **no** domains is unknown and intersects every file. With nothing
  declared, the scope is the whole instance -- one flush domain -- which is the default.
- An imported failure is scoped to the whole instance, one lineage, or a set of domains.

The declarations must be complete; see what this contract requires of the consumer. Each suspect
write is reported with its identity, the consumer's file identity and its epoch -- not its extent,
which the consumer supplied and can track by the write's identity -- and a failure
belongs to every lineage that has a write in its suspect set.

### How a failure is named

Each failure has a **`Copy` identity**, never reused within an instance, used in events and queries;
and a **token**, the only thing that can resolve it. The token is affine and move-only: it comes with
the `Failed` event, can be passed around, and is consumed by heal, abandon or close. A failure has at
most one live token; dropping a token is `close()`, and the failure's token can be taken from the
inventory again. Another instance's identity or token is refused.

### Resolving a failure

- **Heal.** The consumer asserts it has re-issued what it needs, under tags above everything already
  sealed (guarantee 6 forces that). The heal takes effect when the commit of the **first seal made
  after the heal** completes successfully -- so whatever was re-issued before the heal is covered
  successfully later. Then the failure is resolved as healed.
- **Abandon.** The consumer declares the suspect writes lost. Takes effect immediately.
- **Close.** The consumer sets the failure aside, unresolved; it stays in the inventory.

Several failures may be resolved in **one call**, mixing heal and abandon. The call is validated whole
before anything changes, applies atomically at one point in observation order, and if refused hands
its tokens back. Resolution is per failure ([DI-D-12](DESIGN-NOTES.md#di-d-12)).

### What resolution does to epochs and the high-water mark

- An epoch passes the high-water mark only when **every** failure containing it is resolved.
- It is **durable** if every one of those failures was healed, and **abandoned** if any was abandoned.
- Once resolved either way, it is business as usual: the high-water mark may move past an abandoned epoch,
  which is why "durable through n" means **durable or abandoned**.
- A request left `Blocked` by a failure receives its `Durable` as soon as the failure is resolved.
- Gated operations on an abandoned epoch fail (guarantee 9).

### The inventory

Unresolved failures can be enumerated and each queried for its identity, cause and suspect set.
Resolved failures leave no memory. Because identities are never reused, an identity no longer in the
inventory means resolved; the `Durable` or `Abandoned` event was the record.

### Importing a failure

The consumer can tell an instance that a failure happened outside it -- another writer's failed
flush, learned through the consumer's own coordination. dioring then records a failure with cause
"imported", its suspect set built by the same rule as any other, and it is resolved the same way. An
import answers "writes not yet durable may already have been lost"; it never says "writes reported
durable were not", and it cannot retract a `Durable` already reported. It is only as useful as how
promptly the consumer learns.

## The completion queue

The consumer pops **dioring's** completion queue, not the kernel ring's. It holds every completion
the consumer should observe -- reads and writes, passed through -- plus five events dioring
synthesizes ([DI-D-13](DESIGN-NOTES.md#di-d-13), [DI-D-30](DESIGN-NOTES.md#di-d-30)):

| Event | When |
|---|---|
| `Durable { through: n }` | the high-water mark has reached n |
| `Failed { failure, token, cause, suspect }` | a durability failure was observed (a flush, or a provider's domain, failed; or an import) |
| `Blocked { through: n, by: failure }` | a request's flushes succeeded, but an unresolved failure at or below n prevents reporting it |
| `Abandoned { failure, suspect }` | the consumer abandoned a failure |
| `LineageEnded { lineage, abandoned_through }` | the consumer ended a lineage; every epoch of it not yet durable is abandoned |

## The readiness signal

An instance has a **readiness signal**: an auto-reset event it owns, of which the consumer can take
a duplicate, as a `win-sync-sys` `Event` ([DI-D-28](DESIGN-NOTES.md#di-d-28)). These rules bind
every implementation of this contract and whoever waits on the signal, and the conformance module's
`check_readiness` checks the instance's half of them:

- **The instance** sets the signal only after an entry has become poppable, and at least every time
  its queue goes from empty to non-empty. It sets it for every source of entries alike: kernel
  completions, provider answers, and what dioring synthesizes.
- **The waiter** pops until the queue is empty after every wake, before waiting again. A waiter that
  waits again with entries left can sleep forever.
- **A wake with nothing to pop is normal**, not an error.
- **One waiter per instance**, matching the single deliverer
  ([DI-D-18](DESIGN-NOTES.md#di-d-18)).

## Ending an instance

An instance ends by `close()` or by being dropped. Both end it the same way; `close()` also hands
back what is left ([DI-D-25](DESIGN-NOTES.md#di-d-25)).

- **No implicit durability.** Ending issues no final seal and no flush. A write not already
  reported durable gets no promise.
- **Ending waits for what is outstanding.** It returns only once every operation the kernel holds
  and every durability-provider call in flight has completed, because each may still be using
  memory or state the instance owns. dioring adds no other waiting. If the ring cannot be shown to
  have finished, its buffers are leaked rather than freed. A provider call that never completes
  means ending never completes; nothing asks the provider to stop early.
- **What is left.** Gated operations the kernel never saw, unresolved failures, and each lineage's
  final high-water mark are returned by `close()` and discarded by a drop. Nothing reported is
  undone: an epoch reported durable stays durable whether or not its event was popped.
- **No callback after the end.** Under Model A (threadless dispatch), no delivery callback runs once
  ending has returned.
- **While unwinding,** ending reports problems by tracing rather than panicking.
- A failure token's drop is its `close()`.

## Ending a lineage

A holder that will not finish a lineage ends it ([DI-D-30](DESIGN-NOTES.md#di-d-30)).

- **Every epoch of it not yet durable is abandoned**, sealed ones whose flushes are still in flight
  included; a flush that finishes afterwards reports nothing. Epochs already durable stay durable.
- **Its dependants end rather than wait.** A gated operation on one of its epochs, in any lineage,
  ends as never issued, and a new gate on one is refused (guarantee 9).
- **It waits for nothing and cancels no I/O.** Operations in flight complete normally and their
  completions are delivered. There is no lifetime or durability dependency to wait for, unlike
  ending an instance.
- **Its handle is refused at once**, and the lineage is retired once its operations in flight have
  drained.
- **A failure shared with another lineage stays unresolved** for that lineage; ending one lineage
  resolves nothing on another's behalf.
- **The default lineage cannot be ended**; a consumer walking away from it ends the instance.

## Not yet specified

Nothing is left unspecified at the contract's level. The concrete types are specified in
[API.md](API.md), and completion routing in [DI-D-18](DESIGN-NOTES.md#di-d-18).

## Worked examples

The traces use illustrative names. `->` is a call the consumer makes; `<-` is an event it pops.
Every write is in the default lineage, so the lineage is not shown.

### Healing a failure

```text
-> write(tag 41, A) ; write(tag 41, B) ; write(tag 41, C)
-> make_durable_through(41)                 // seals everything <= 41
<- WriteDone(A) ; WriteDone(B) ; WriteDone(C)
<- Failed { failure: F1, cause: Flush, suspect: [A@41, B@41, C@41] }
                                            // durable_through() stays 40
-> write(tag 41, A')                        // refused: 41 is sealed (guarantee 6)
-> write(tag 42, A') ; write(tag 42, B') ; write(tag 42, C')
-> heal(F1)                                 // "I have re-issued what I need"
-> make_durable_through(42)                 // the first seal after the heal
<- WriteDone(A') ; WriteDone(B') ; WriteDone(C')
<- Durable { through: 42 }                  // F1 healed; 41 and 42 durable
```

Without the heal, the last line is `Blocked { through: 42, by: F1 }`: 42's own flush succeeded, but
the high-water mark cannot pass the unresolved failure at 41.

### Failures and epochs are many-to-many

```text
-> write(tag 41, A)
-> make_durable_through(41)                 // seals 41
-> write(tag 43, P)                         // 43 is open
<- WriteDone(A) ; WriteDone(P)
<- Failed { failure: F1, cause: Flush, suspect: [A@41, P@43] }
                                            // one failure, two epochs: sealed 41 and open 43
-> write(tag 43, R)                         // still allowed: 43 is open
<- WriteDone(R)
-> import_failure()                         // the consumer learned of an outside failure
<- Failed { failure: F2, cause: Imported, suspect: [A@41, P@43, R@43] }
                                            // epoch 43 now sits in F1 and F2
```

Two ways it can end:

- **Heal both.** Re-issue A, P and R under tag 44, then `resolve([F1: heal, F2: heal])` in one
  atomic call and `make_durable_through(44)`. `Durable { through: 44 }` follows: 41 and 43 are durable,
  having passed only once both failures containing them were resolved.
- **Heal F1, abandon F2.** Epochs 41 and 43 both sit in F2, so both become **abandoned** -- one
  abandonment is enough -- even though F1 was healed. The high-water mark moves past them, and any operation
  gated on 41 or 43 fails as never issued.
