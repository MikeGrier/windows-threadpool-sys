# Design session 2026-10-22: durable-ioring without std, and with every allocation made at construction

Component: [crates/durable-ioring](../crates/durable-ioring/COMPONENT.md). The engineer drove; the assistant
investigated and proposed. Decisions that resulted, all in
[DESIGN-NOTES.md](../crates/durable-ioring/DESIGN-NOTES.md):

- [DI-D-97](../crates/durable-ioring/DESIGN-NOTES.md#di-d-97) -- the goal: `no_std` with `alloc`, every allocation the
  core needs made at construction, and none after it.
- [DI-D-98](../crates/durable-ioring/DESIGN-NOTES.md#di-d-98) -- capacity: what is refused at admission, and what a seal
  is charged to.
- [DI-D-99](../crates/durable-ioring/DESIGN-NOTES.md#di-d-99) -- a seal that covers no writes makes no record.
- [DI-D-100](../crates/durable-ioring/DESIGN-NOTES.md#di-d-100) -- a failure holds the capacity of what it puts at risk.
- [DI-D-101](../crates/durable-ioring/DESIGN-NOTES.md#di-d-101) -- `Durable` entries coalesce.
- [DI-D-102](../crates/durable-ioring/DESIGN-NOTES.md#di-d-102) -- a fault's report is an associated type. Superseded by
  DI-D-105.
- [DI-D-103](../crates/durable-ioring/DESIGN-NOTES.md#di-d-103) -- the clean paths return no allocated collection.
- [DI-D-104](../crates/durable-ioring/DESIGN-NOTES.md#di-d-104) -- every allocation is tagged by the code that makes it
  (continued 2026-10-23).
- [DI-D-105](../crates/durable-ioring/DESIGN-NOTES.md#di-d-105) -- a fault's report is a static or a shared `dyn Error`,
  shared through a new crate, `rc-errors` (continued 2026-10-23).
- [DI-D-106](../crates/durable-ioring/DESIGN-NOTES.md#di-d-106) -- the core's tables are fixed-capacity and preallocated,
  from a new crate, `fixed-hash-table` (continued 2026-10-23).
- [DI-D-107](../crates/durable-ioring/DESIGN-NOTES.md#di-d-107) -- what capacity the consumer declares (continued
  2026-10-23).

The work is queued as DI-M23 in [CHECKLIST.md](../crates/durable-ioring/CHECKLIST.md).

## Why

The engineer: "my goals are for a no-std user mode filesystem driver stack that I have. This is the first issue with
durable-ioring that came up." Then: "It's not acceptable for the driver to panic due to OOM in the middle of things."

## The std census

Asked to investigate making the crate `no_std`, the assistant read the 32 non-test source files and `io-buffers`.

- **Mechanical.** `fmt`, `mem`, `ops`, `cmp`, `hash`, `marker`, `num`, `ptr::addr_eq`, atomics and `time::Duration` are
  in `core`; `std::error::Error` is `core::error::Error` (stable since 1.81); `Arc`, `Box`, `Vec`, `VecDeque`, `BTreeMap`
  and `BTreeSet` are in `alloc`. `io-buffers` uses only `alloc` and `core` items. The engine allocates throughout, so the
  target is `no_std` with `alloc`, not `core` alone; `Arc` also needs pointer-width atomics.
- **Blockers.** `io::Error` in the public contract (`IoFault::error`, `FailureCause::Provider::error`, the backend traits'
  `io::Result`s, the pump's `Notification::wait`, `provider.fail`, `RefusalCause`'s mapping to `io::ErrorKind`);
  `Mutex` (the engine's slot, the pump, `lock_watch`) and the in-memory backend's `Condvar`; `HashMap` in seven files,
  which needs `RandomState`; `std::thread::panicking()` guarding the inconsistency panic ([DI-D-46](../crates/durable-ioring/DESIGN-NOTES.md#di-d-46));
  and `StderrDiagnostics`' `eprintln!`, which an additive `std` feature can gate.
- No design note or checklist mentioned `no_std` before this session.

## Errors

The engineer: "For Io::Error, we'll have to build our own type." The assistant observed that `io::Error` plays two
roles: a report the core carries and never interprets (it stores, compares by `Arc` identity and displays), and the
error a call returns. Callers downcast through `get_ref()` today (to `RefusalCause`, `SharingViolation`), so whatever
replaces it must keep downcasting.

The assistant proposed keeping `IoFault` (source, `ErrorCode`, report) with the report an `Arc<dyn core::error::Error +
Send + Sync>`. The engineer: "IoFault is good. I don't understand the kind question. Might as well use Arc."

The kind question, explained: `io::Error` carries an `ErrorKind` category a caller can branch on without knowing the
platform's codes; an `IoFault` with a `dyn Error` report has none. Nothing in the core branches on a kind, so none is
needed now.

The `Arc` answer was later reversed by the preallocation goal below: an `Arc` made when a backend fails allocates at
failure time. Asked what no-alloc, no-std crates do, the assistant reported the ecosystem's answer: `embedded-io` (and
`embedded-hal` 1.0, per peripheral) gives each I/O trait an associated `type Error`, bounded by a crate trait that is
`core::error::Error` plus `fn kind(&self) -> ErrorKind`; errors are small `Copy` values with no payload, and nothing
allocates because the implementer chooses the type. The engineer had suggested the same direction ("I feel like we're
going to be delegating everything but maybe this indicates that the error type is yet another generic parameter?"), and
accepted the plan: associated types on the traits the instance is already generic over, not new parameters on the
instance. Recorded as DI-D-102.

## Allocation through an interface

The engineer asked whether allocation could be driven through an interface. The assistant set out three levels:

1. **The global allocator.** A `no_std` crate with `alloc` allocates through whatever `#[global_allocator]` the binary
   installs. Free once the crate is `no_std`.
2. **A per-instance allocator, like the clock.** `Vec::new_in`, `Arc::new_in` and the `Allocator` trait are unstable on
   1.98 (checked: `allocator_api` is E0658). The stable routes are `allocator-api2`, which has no `Arc` or `BTreeMap`,
   or a trait of our own threaded through every container.
3. **No allocation after construction**, which is usually what a kernel or real-time consumer wants. An engine
   property, not an interface.

The engineer: "if the correct analysis is that a global allocator is required, just good allocation discipline is
needed that's a decent finding. It needs to be specific and we would need to make it clear."

## The allocation census

The assistant attributed every allocation site in the 16 core source files (excluding the `memory` backend and the
`oracle`) to its enclosing function and classified it by phase:

- **Construction.** The core's maps and queues, the router's routes, domain interning, the pump's `Arc<Mutex<...>>`.
- **Per write, submit and completion.** Grow-only containers that settle at their high-water mark (`io_core`'s
  `in_flight` and `per_target` `HashMap`s, its `queue` and `unpushed`, durability's `seals`), and `BTreeMap` and
  `BTreeSet` tables that allocate a node as one fills or a table refills after emptying (`io_core.held`; durability's
  `writes`, `held`, `in_flight_by_epoch`, `gated`, `waiting`, `held_by_epoch`, `abandoned`). A gated write's cycle check
  allocates a `Vec` on every call (`write_cycle` calls `needs(vec![gate], ...)`), and more through `trace` when it finds
  a cycle.
- **Per action.** `Due::default()` starts empty `Vec`s of events, flushes, submissions and provider calls, each of which
  allocates at its first push; several `.collect()`s make temporaries.
- **Per seal.** `start_seals`' file and part `Vec`s, its flush pairs and provider calls.
- **Per failure.** The `Failure` record, suspect set and markings, the `Arc` report, a `FailureToken`'s
  `Arc<AtomicBool>`.
- **Per lineage.** A `LineageHandle`'s `Arc` and its description's `Arc<str>`.

## The goal

The engineer: "I want to work through what it will take to make the durable-ioring work entirely with pre-allocated
memory allocations. Meaning that we allocate all heap needed for operation during construction." Scope: "I think
that's a good place to start" -- the core first.

The assistant laid out what it takes: every growth point bounded by a capacity declared at construction; growth the
consumer initiates refused at admission; growth the core only observes reserved by an earlier admission; the `Arc`
sharing in the contract replaced by fixed storage; owned collections handed out replaced; the `BTreeMap`s replaced;
per-action scratch reused; construction fallible through `try_reserve`; and a counting global allocator in an
integration test as the proof. Recorded as DI-D-97.

## Admission and the seal

The engineer: "submit and mint seem ok but it seems like seal should be able to resolve always."

The assistant read `seal` and `start_seals`. A seal's work is fixed by writes already admitted (guarantee 6 refuses a
later write at or below the seal), and `start_seals` names each write exactly once, pushing a file and a flush per
distinct file, a part per domain, a flush pair per part and one provider call. So a seal's cost can be charged to the
writes and the lineage that precede it, and a seal needs no capacity refusal. Its contractual refusals (a gate cycle, a
lineage that is not live) stay; the engineer confirmed.

The engineer on the charging: "I don't see that every write must pay for the cost of a potential flush, only the first
for a lineage, yes?" The assistant showed from the code that a lineage can have several seals flushing at once, each
flushing its own files, and that lineages flush separately, so the unit is the (lineage, file, seal span): the first
write to a file in the lineage's current unsealed span pays for the flush, the flush pair and its provider parts. The
engineer: "(lineage, file, seal span) - agreed. I just don't want every write to turn into a cacophany of memory
allocations." Recorded as DI-D-98.

## A seal that covers no writes

The assistant noted that a consumer can seal ever-higher empty epochs, each making a record today. The engineer: "agreed.
It should call any diagnostic channels to announce its having been sealed and depending on the environment, *they* may
allocate but the core algorithm forces no allocation."

The engineer left the heal rule to the assistant ("I will let you work this through"). Worked through against the code:
a heal records, per lineage, `Release { after, through }` (the lineage's next seal number and highest pushed epoch); a
seal releases it when it finishes successfully with `seq >= after` and a `through` that reaches it. Seals finish
independently (`finish_seals`) but start in order (`start_seals` waits for every write in flight at or below a seal's
`through`), and the high-water mark advances strictly in queue order. So an elided seal must still take a sequence
number -- otherwise a heal with nothing to re-issue, followed by an empty seal, would never release -- and must be
evaluated where it stands in order. Both fit in fixed space: one "trailing empty seal" field on each recorded seal, and
one per-lineage slot when none is queued, holding the highest sequence number and `through`, which satisfy every release
and mark advance the earlier ones would have.

The assistant raised the diagnostic pairs ([DI-D-96](../crates/durable-ioring/DESIGN-NOTES.md#di-d-96) holds a producer to
ending every begin) and proposed ending an overwritten empty seal's pair at once with a `Superseded` outcome. The
engineer agreed. Recorded as DI-D-99.

## Failures and capacity

The assistant proposed that a failure keep holding the capacity of the writes it puts at risk until it is resolved,
since otherwise failures outlive the reservations that bound them, with the visible effect that unresolved failures
shrink the submit window. The engineer: "agreed. make this clear and the documentation must include examples of how to
address." Recorded as DI-D-100.

## Coalescing `Durable`

The assistant proposed one un-popped `Durable` per lineage, so a consumer slow to pop cannot exhaust the completion
queue. Asked whether there is a controversy: none against the contract, which defines `Durable { through: n }` as the
high-water mark having reached n, but the contract's rule that `Healed` comes before the `Durable` a heal lets through
means the older entry is removed and the new one appended at the tail, never updated in place. The engineer: "ok so
coalescing Durable is approved!" Recorded as DI-D-101.

## Collections returned

The engineer asked which returned collection has the highest typical cardinality. The assistant listed every one:

| Collection | Bounded by | Path |
|---|---|---|
| `SuspectSet` | the write window | failure |
| `markings` | the failure's suspect set | failure |
| `Leftovers { held, failures, lineages }` | gated operations, failures, lineages | `close()` |
| `failures()` | unresolved failures | query |
| `take_requests()` | a seal's domain parts | per seal with a provider route |
| `lineages()` | live lineages | query |
| `IoTargetBusy { lowest_gates, uncovered_through, failures }` | lineages and failures | refused `remove_target` |
| `LineageBusy { failures }` | failures | refused `retire_lineage` |
| `GateCycle { cycle }` | gated operations, typically two | refused gate or seal |
| `Imported { unmatched }` | the domains the consumer named | import |
| `ResolveError { returned }` | the tokens in the call | refused resolve |

The engineer then asked which belong to an I/O lifecycle with no errors: "some number of reads and writes, seals,
lineages beginning and ending". The assistant: `take_requests()` per seal with a provider route; `Leftovers` once at
`close()`, where `lineages` is never empty; `lineages()` if queried; and `IoTargetBusy` and `LineageBusy` when a
consumer removes a target or retires a lineage too early, an ordering refusal rather than a failure. `GateCycle` is a
consumer mistake. Suspect sets, markings, `failures()`, `FailureInfo`, `ResolveError` and `Imported` arise only from a
failure.

The engineer: "just for lack of a better term, let's call these the clean paths. for these clean paths, let's ensure
that these are allocation free, so either the caller passes in arrays to fill or the collection elements are accessed by
calling a function that invokes a functor for each collection element." Recorded as DI-D-103.

## Allocation tags (continued 2026-10-23)

With the measurement of `DI-23.1` in place, counting allocations per phase, the engineer: "I would consider going even
stricter: give each allocation a unique label, at least by source code location and establish counts by source code
location. Is there any capability for that in the allocation discipline?"

The assistant: a global allocator is given only a `Layout`, and `#[track_caller]` does not reach it through the standard
containers. It can walk the stack itself and attribute an allocation to the innermost frame in the crate, through the
`backtrace` crate (whose frame access is specified; `std::backtrace`'s is unstable, and parsing its text would bind to an
unspecified format), debug builds only, keyed by function so an edit above a site does not move it.

The engineer: "I was hoping for caller assigned tags. That's not possible?" The assistant: it is, as a facility in the
core rather than in the test -- the code sets a thread's current tag around its allocating code, the allocator reads it,
the tags are named constants, `#[track_caller]` adds the location, the facility is behind a feature only the test enables,
and an allocation counted with no tag set fails the test, so the discipline is enforced where an allocation is added.

The engineer: "oh that's a good idea, I've done that before too. make the tagging RAII based so that the discipline is
trivial. please schedule this before going further in the allocation analysis, I don't want to do further analysis
without tagging." Recorded as DI-D-104 and scheduled as `DI-23.2`, ahead of every item that analyses or removes an
allocation.

The assistant's question of how coarse a tag should be -- per table and container, or per operation -- was answered by the
engineer: "If I had my choice, I would have a unique tag for each and every memory allocation. I realize that we cannot
change, for example, a container's source code, so setting an 'allocation context' around calls into a container's
mutation will have to be fine grained enough." So a tag names one allocating call, each guard is scoped to that call, and
the assistant proposed checking uniqueness from the location the guard records. Recorded in DI-D-104.

## The fault report without the global allocator (continued 2026-10-23)

Starting DI-23.4, the assistant stopped before writing code: the core is not generic over its backend in the way DI-D-102
assumed, the core makes refusal faults itself, and the ring trait, the notification and the pump's setup all report
`io::Error`. It proposed error types on `Identities`. The engineer approved, then asked: "Isn't there an alternative here
with dyn traits though? I know that if we have one provider stack top to bottom avoiding dyn traits is a good thing but
here since we don't know what may come up, I wonder if it's a false efficiency." The assistant agreed that dispatch cost
on an error path is irrelevant and that storage is the real constraint, and proposed a report of two forms: a static
`dyn Error`, and an `Arc<dyn Error>`.

**`Arc` without `std`.** The engineer asked the status of no-std `Arc`. Checked against the stable documentation (1.99 at
the time): `alloc::sync::Arc` works without `std`, but `try_new`, `new_in` and every fallible or allocator-aware
constructor is unstable behind `allocator_api`, and `UniqueArc` is not on stable. An `Arc` made at failure time is an
infallible allocation after construction.

**What an `Arc` without weak references needs.** The engineer asked what the real allocation requirements are if weak
references are dropped. The assistant: one fallible allocation of header and payload, a deallocation callable from any
thread with the layout recovered from the value, and the destruction of the value and the freeing of the block in one
step. Stable `alloc::alloc::alloc` returns null on failure, so such a type can be fallible on stable. Unsizing, which the
engineer asked to have explained, is the conversion of a thin pointer to a fat one carrying a length or a vtable; `Arc`
opts into it through the unstable `CoerceUnsized`, while a raw `NonNull` coerces on stable, so a type of ours can unsize
inside its constructor. Asked whether `Arc` relies on unsafe and layout assumptions: it is all `unsafe`, over a
`#[repr(C)]` header, with features only the standard library may use; ours would be `unsafe` too, and `repr(C)` removes the
one layout question (whether freeing with `Layout::for_value` matches the allocation) that is otherwise not documented.

**"Arc, just without weak support."** The engineer: "I rarely see a utility for weak references and see the provision for
them as a waste in any case so to me this has been something I've wanted to build for a while. My mental model here is
'take the Arc source code, delete the weak reference parts, fix what's broken, see what's left'", with "We would specify
our own allocation api which would be a copy of the current allocator API. The goal would be to move to the final
allocator API when possible", and "this would be a derivative work, would be credited as such but would have its own
destiny." The assistant surveyed upstream's `library/alloc/src/rcs/arc.rs` (5,212 lines, about 1,900 of code): deleting
weak removes the weak count (a word of header), the weak-count lock in `is_unique`, a case of `make_mut`, two-phase
destruction, `new_cyclic` and most of `UniqueArc`; what breaks on stable is mostly not weak but the language features only
the standard library may use -- `Unsize`, `CoerceUnsized`, `DispatchFromDyn`, `may_dangle`, specialization, intrinsics, and
the allocator traits, which upstream has recently split into four.

**Dispatch.** The engineer asked why `DispatchFromDyn` is harder than unsizing, and whether the compiler knows `Arc`
specifically. It does: `self: Arc<Self>` is accepted for `Box`, `Rc`, `Arc` and `Pin` because the standard library
implements unstable traits for them. The general mechanism, `derive(CoercePointee)` with arbitrary self types, is
unstable (#123430, #44874), confirmed by compiling on the pinned 1.98. Unsizing is a value conversion made once, which a
library can do on a raw pointer; dispatch is code the compiler emits for every `dyn` call, which only the compiler can
make. Linux's `kernel::sync::Arc` is the prior art: no weak references, a smaller header, and those features reached
through the kernel's nightly build. A caller-supplied coercion closure would be safe code able to forge a length or a
vtable, so it would have to be an `unsafe fn`.

The engineer: "This has soured me on the Arc replacement. The Linux parallel is a good one, I want the same efficiency
and to replace the allocate/deallocate." Replacing `Arc`'s deallocation per instance is impossible on stable; only the
process-wide `#[global_allocator]` is replaceable, and "a library may not require a global allocator override so that's
really out of the question."

**The choice.** Three ways to carry a fault without the core depending on the global allocator: a small one-count shared
pointer for fault reports only, which loses nothing to the dispatch limitation since `Error`'s methods take `&self`; a
fault pool owned by the core; or the report's type chosen by the backend, as DI-D-102 had it. The engineer chose the
first. Two questions followed: whether the pointer reaches its deallocator through a type parameter or through an
allocator recorded in its header, where the engineer chose the header ("A: ii"); and whether the core keeps a static
form for its own refusals, so it never allocates a fault ("B: sounds good").

**Where it lives.** Recording it, the assistant found that the pointer needs `unsafe` and durable-ioring is
`#![forbid(unsafe_code)]`, with DI-D-4 saying that is the moment to fix the layer below. The engineer chose a new crate
over io-buffers and over amending DI-D-4, proposing `durable-ioring-rc-errors-sys`: "please use it unless you have an
objection. If you think that it will be applicable more broadly, feel free to shorten it." The assistant objected to
`-sys`, which marks a thin unsafe Win32 wrapper in this workspace, and shortened the name to `rc-errors`, free on
crates.io. Recorded as DI-D-105.

## Capacity and the tables (continued 2026-10-23)

Starting DI-23.5, the assistant stopped to ask the shape of the capacity declaration DI-D-98 left open, finding that the
core's growable state is `io_core`'s maps and `durability`'s tables, most of them B-trees. It proposed `operations`,
`lineages` and `imports`, with the queue derived and reads sharing the window, and asked whether targets were declared, whether
a default was wanted, and which signatures would break.

The engineer: "Seems like the collections need to move to closed hashing." The assistant noted that `std`'s `HashMap` is
already closed hashing (hashbrown's SwissTable), and that what rules it out is growth on insert past its capacity, being
`std`-only with an OS-seeded hasher, and the B-trees allocating a node per insert; it read the point as fixed-capacity,
preallocated tables that refuse when full. The engineer: "yes fixed capacity preallocated tables."

On the follow-ups: "build, put it in a separate crate, call it fixed-hash-table, it should support as much of the usual
collection affordances as possible, trimmed towards no-std, no-alloc, with generics to allow for clients who do want std and
alloc"; the hasher a type parameter, "ideally have a generic T which can provide a functor that does not vary by instance and
saves storage as well as allow for an instance which permits per collection functors" -- which `core::hash::BuildHasher`
already does, a zero-sized one being per type -- the capacity declaration confirmed, and targets declared and refused past
capacity ("5: a"). Recorded as DI-D-106 and DI-D-107; DI-23.5 is re-planned with the crate first.

## Working positions and open questions (not decisions)

- **The failure's slot.** How a failure's fixed storage holds its suspect set and markings, and how the events and the
  inventory lend them out. The next design topic; it decides the failure paths' shape, as DI-D-103 decides the clean
  paths'.
- **The `Arc`-shared contract types** (`SuspectSet`, `LineageHandle`, `FailureToken`, a lineage's description) become
  fixed storage, by index and generation or by borrow. Not chosen.
- **The ordered tables.** The eight `BTreeMap` and `BTreeSet` tables are mostly keyed by an increasing sequence number
  or epoch, so most are queues with out-of-order removal; the ones with ranged queries (`gated.range(...)` in the cycle
  check) need a closer read before a structure is chosen. `HashMap::with_capacity` suffices where a `HashMap` is used.
- **`Mutex` without `std`.** A lock the driver supplies, locking moved out of the core, or a lock crate. Not discussed.
- **`thread::panicking()`** has no `core` equivalent. Not discussed.
- **A portable `ErrorKind`.** Not needed by the core; offered for the case where the engineer's stack is generic over
  backends. Not answered.
- **The inconsistency panics of DI-D-46** are a different class from OOM; the assistant assumed them out of scope and
  the engineer has not said otherwise.
