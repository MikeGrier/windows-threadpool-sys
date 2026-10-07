# durable-ioring: the API shape (DI-2.1, DI-2.2)

**Approved by the engineer, 2026-10-05.** Sections 1-6 are DI-2.1's concrete types for the tag, a
write's push, and dioring's completion queue ([DI-D-15](DESIGN-NOTES.md#di-d-15)). Sections 7-10
are DI-2.2's composition with `windows-ioring-sys` ([DI-D-17](DESIGN-NOTES.md#di-d-17)), and amend
sections 2 and 4 where noted. Section 11 is the options pattern
([DI-D-20](DESIGN-NOTES.md#di-d-20)), section 12 durability lineages (DI-2.9,
[DI-D-19](DESIGN-NOTES.md#di-d-19)), section 13 flush domains (DI-2.10,
[DI-D-21](DESIGN-NOTES.md#di-d-21)), section 14 ending an instance (DI-2.7,
[DI-D-25](DESIGN-NOTES.md#di-d-25)), section 15 durability providers (DI-2.12,
[DI-D-27](DESIGN-NOTES.md#di-d-27)), section 16 the readiness signal (DI-2.14,
[DI-D-28](DESIGN-NOTES.md#di-d-28)), section 17 the contract as a trait (DI-2.15,
[DI-D-24](DESIGN-NOTES.md#di-d-24)), and section 18 ending a lineage (DI-2.13,
[DI-D-30](DESIGN-NOTES.md#di-d-30)). The document spells out [CONTRACT.md](CONTRACT.md); where
they disagree, the contract wins.

The sketch at the end compiles against `windows-ioring-sys` by path
([DI-D-16](DESIGN-NOTES.md#di-d-16)) under the pinned toolchain, clippy clean, with bodies
`todo!()`. It is marked `ignore` until DI-3.1 gives it a crate.

## The choices

### 1. The epoch-id type is the consumer's

*(Amended by section 12: an epoch is now a lineage plus an epoch id. `E` is the epoch-id type,
and the trait is `EpochId`, formerly `EpochTag`. Amended by section 17: the contract is the
`DurableRing` trait, dioring's implementation is `Dioring<B, E = u64, ...>`, and an
implementation's epoch-id type is its `Identities::EpochId`. The examples below name dioring's
type.)*

The ring is generic over its epoch-id type: `DurableRing<B, E = u64>`, with `E: EpochId`, where
`EpochId` is simply `Copy + Ord + Debug`. dioring never does arithmetic on epochs -- it compares and
stores them -- so a `u64`, an `Lsn` newtype, or a `(segment, offset)` pair all work unconverted, and
the default keeps the simple case simple. (The engineer's suggestion; it replaced a `u64` newtype.)

**Comparability across instances is the consumer's choice, through types.** A database keeps its
log on one instance and its data files on another, each its own failure domain:

```text
log ring:   DurableRing<_, Lsn>             data ring:  DurableRing<_, CheckpointNo>

log:   write(Lsn(1000), log record) ; make_durable_through(Lsn(1000))
       <- Durable { through: Lsn(1000) }
data:  page P has page-LSN 1000, so it may be written only once the log is durable through it:
       the consumer checks log.durable_through() >= Some(page_lsn), then
       write(CheckpointNo(7), P)
checkpoint: data.make_durable_through(CheckpointNo(7)) -> Durable { through: CheckpointNo(7) };
       only then may the log be trimmed below the checkpoint's redo LSN
```

Comparing the log's high-water mark with a page's LSN compiles, because both are `Lsn`; passing an
LSN as a data-ring tag, or comparing it with a checkpoint number, does not. Durability itself never
crosses: dioring never relates two instances, and the join is the consumer's
([DI-D-14](DESIGN-NOTES.md#di-d-14)). The per-instance guard on what dioring mints stays: `OpId` and
`FailureId` carry the instance, `OpId` is only `PartialOrd`, and another instance's token or identity
is refused. *(With lineages, section 12, the log and the data can instead be two lineages of one
instance, and a data write can be gated on the log lineage's epoch, so dioring does the join. The
lineages of one instance share its epoch-id type `E`. A consumer that wants the compiler to keep
LSNs and checkpoint numbers apart, as above, keeps two instances.)*

**What dioring relies on, and the compiler cannot check:** `E`'s `Ord` is a total order that stays
consistent for the life of the instance, and its values never wrap. A sequence compared with
wraparound -- jbd2's 32-bit transaction IDs are one -- is not a total order and must be mapped onto
one that does not wrap. A wrong `Ord` cannot cause memory unsafety, but it makes dioring's reports
wrong, so CONTRACT.md lists both as requirements on the consumer.

### 2. A write carries its tag

`write(file, offset, buffer, epoch, gate)`; reads take no epoch. Both take an optional `gate`, the
epoch they wait on. *(Amended by section 11: the gate moves into `WriteOptions` and
`ReadOptions`, taken by the `_with` forms.)* Refusals happen before anything is reserved and return the buffer in
`PushError<B, E>`: `Sealed` (contract guarantee 6), `GateAbandoned` (the gate can never be
satisfied), or `Ring`. *(Amended by section 10: `PushError<B, E, C>` also returns the consumer's
context, its buffer is `Option<B>`, and two refusals are added.)*

### 3. `make_durable_through` answers at once when it can

It returns `Submitted` for a new seal, answered later on the queue, or `AlreadySealed(EpochState)`
for the no-op case (guarantee 7). `EpochState` is `Open`, `Pending`, `Blocked(FailureId)`, `Durable`
or `Abandoned`, and also answers `epoch_state(epoch)`. The high-water mark and seal point are
`Option<E>`, `None` until something is durable or sealed. *(Amended by section 12: both are per
lineage, `durable_through(lineage)` and `sealed_through(lineage)`, and every epoch is an
`Epoch<E>`.)*

### 4. One queue entry type

`Entry<B, E>`: `Op(OpCompletion<B, E>)`, `Durable { through }`, `Failed(Failed<E>)`,
`Blocked { through, by }`, `Abandoned { failure, suspect }` ([DI-D-13](DESIGN-NOTES.md#di-d-13)).
`Blocked` is fixed-size, so it is cheap to produce.

*(Amended by sections 7 and 9: `Entry<B, E, C>`, and an `OpCompletion` also carries the
consumer's context; its buffer is `Option<B>`, `None` for a registered-span operation.)*

An `OpCompletion` carries its `OpId`, kind (`Read` or `Write { epoch }`), outcome and buffer. The
outcome is `Transferred(u32)` (short counts included), `Failed(io::Error)`, or
`NeverIssued { abandoned }` for a gated operation whose epoch was abandoned.

### 5. A failure arrives with its token

`Failed` carries the failure's `FailureId`, `FailureToken`, `Cause` (`Flush { file, error }`,
`Imported`, and -- section 15 -- `Provider { domain, error }` or `ProviderAbandoned { domain }`)
and `SuspectSet`. The error is `Arc<io::Error>` because the inventory reports it again.
`SuspectSet` is a shared, frozen `Arc<[SuspectWrite<E>]>`; each entry names the write's `OpId`,
`FileKey` and epoch. The extent is not reported: the consumer supplied it, and tracks it by `OpId`
if it needs it ([DI-D-6](DESIGN-NOTES.md#di-d-6)).

`OpId` is ordered by push order, which answers [DI-D-6](DESIGN-NOTES.md#di-d-6)'s open question: the
retention layer gets the order it needs for newest-wins replay at no extra cost.

### 6. Resolution

`FailureToken` is `#[must_use]`; `heal`, `abandon` and `close` consume it, and dropping it closes it
([DI-D-12](DESIGN-NOTES.md#di-d-12)). `resolve(Vec<(FailureToken, Resolution)>)` resolves one or many;
a refused call returns the tokens. `import_failure(scope)` returns an identity (section 12: the
scope is one lineage or all of them), and the token arrives with
the `Failed` entry. `failures()` returns owned `FailureInfo`; `take_token(id)` hands out a token when
none is live.

The only borrow returned is `SuspectSet::writes()`, into frozen shared data that nothing can change.

### 7. Composition: dioring's sidecar carries the consumer's

*(DI-2.2.)* `Dioring<B, E = u64, C = (), R = Vec<u8>>`, dioring's implementation of the contract
(section 17; formerly the concrete `DurableRing`). `C` is the consumer's
per-operation context: it goes in with a push and comes back on the operation's `OpCompletion` (or
in a refusal). Inside, dioring runs an `IoRing<B, Sidecar<E, C>>`, where its own sidecar records
what each operation is -- a consumer operation carrying `C`, or one of dioring's covering flushes.
So routing a completion needs no side table, and a consumer that needs no context writes nothing.

### 8. Files

*(DI-2.2.)* A ring has one file registration in its life, so files enter two ways:

- **At construction**, in `Setup::files`, as `(FileKey, SharedHandle)` pairs. These are registered,
  and the ring holds them for its life (`windows-ioring-sys`'
  [D-81](../windows-ioring-sys/DESIGN-NOTES.md#d-81)). A duplicate key refuses construction.
- **Later**, with `add_file(key, SharedHandle)`. These are not registered: each operation holds a
  clone of the handle until its completion. A duplicate key is refused and the file handed back.

The consumer sees the same behaviour either way, and a different cost. Every report names files by
`FileKey` ([DI-D-11](DESIGN-NOTES.md#di-d-11)).

`remove_file(key)` returns the consumer's `SharedHandle`. It is refused while the file has an
operation in flight or held for a gate, **or a write, in any lineage, not yet covered
successfully** -- named to a durability provider that answered success for every domain of the
file (section 15) -- which includes every write in an unresolved failure's suspect set. The second
condition goes beyond "nothing in flight": once a file is gone dioring can no longer flush it, so
an epoch containing an uncovered write to it could never become durable. The refusal is
descriptive, at the engineer's request: `FileBusy` reports every hold at once -- operations in
flight, operations held for a gate and the lowest gate, uncovered writes and the epoch to seal
through, and the unresolved failures involved -- and its `Display` says what clears each. A file
given at construction stays open after removal, until the ring is dropped, because the
registration holds it.

### 9. Buffers: owned, or spans of the registration

*(DI-2.2.)* Two kinds, as decided:

- **Owned `B`**, per operation: `write` needs `B: IoBuf`, `read` needs `B: IoBufMut`. A write-only
  type such as `Arc<[u8]>` stays usable for writes.
- **Spans of the registered buffers**, `write_registered` and `read_registered`, addressed by
  `RegisteredSpan`.

A ring has one buffer registration in its life, so it is made at construction from `Setup::buffers`
(empty registers nothing). The buffers arrive already allocated and placed; placement is the
topology planner's, and dioring neither allocates nor places a buffer.

**dioring owns the registration.** A gated operation is held by dioring and issued later -- from
inside `pop` or a seal, where no borrow from the consumer is available -- so dioring must be able to
reach the registration on its own. The consumer reaches the bytes through
`registered_buffer(i)` and `registered_buffer_mut(i)`, which the ring crate refuses while the kernel
is using that buffer. The registered type is its own parameter `R` (default `Vec<u8>`) rather than
`B`. Sharing `B` would force `B: IoBufMut` on the whole ring, and so take write-only buffer types
away from owned writes.

### 10. Every refusal hands back what it took

*(DI-2.2.)* `PushError<B, E, C>` returns the buffer (`None` for a registered-span
operation, which has none) and the context. This now holds for a `Ring` refusal too, because
`windows-ioring-sys` hands both back (its [D-80](../windows-ioring-sys/DESIGN-NOTES.md#d-80)). Two
refusals are added: `UnknownFile(FileKey)`, and `NoRegisteredBuffers` for a registered-span
operation on an instance built without registered buffers. A failed construction returns its files
and buffers in `SetupError<R>`. The one exception is when the kernel's registration completion
itself fails: the ring crate drops the buffers then, an open question recorded in its
[M30.1 archive](../windows-ioring-sys/COMPLETED-CHECKLIST.md#m301).

### 11. Advanced parameters ride in options

Rust has no optional parameters, so each operation has a plain form for the common case and a
`_with` form that also takes an options value: `write` and `write_with(..., WriteOptions)`, `read`
and `read_with(..., ReadOptions)`, and the same pair for the registered-span operations. The
options are small `Copy` values passed by value and built with setters --
`WriteOptions::new().gate(n)` -- following `windows-ioring-sys`' `PushOptions`. Their fields are
private, so a new option is not a breaking change. The plain form is the `_with` form with default
options.

The gate moves here from section 2's positional parameter. A gate is an `Epoch`, so it may name
any lineage of the instance (section 12).

*(DI-2.6.)* `WriteOptions::caching` passes a per-write `WriteCaching` through to the ring, so
dioring does not hide a knob the platform offers. It shapes latency and never durability: a
write-through write is durable only at its seal. **dioring offers no flush of its own**: durability
comes only from `make_durable_through`, through the durability provider
([DI-D-27](DESIGN-NOTES.md#di-d-27)), because a consumer's flush would bypass the coverage
accounting the contract rests on.

### 12. Lineages, and what an epoch is

*(DI-2.9, [DI-D-19](DESIGN-NOTES.md#di-d-19).)* An instance holds several **durability
lineages**. Each has its own epoch space, seal point, high-water mark and failure set, and every
rule in sections 1-11 and in [CONTRACT.md](CONTRACT.md) applies within a lineage.

**An epoch is a lineage plus an epoch id: `Epoch<E> { lineage, id }`.** Every place the API takes
or reports an epoch carries one: a write's epoch, a gate, `make_durable_through`, `epoch_state`,
an operation's kind, `NeverIssued`, `SuspectWrite`, `Durable`, `Blocked`, and the `Sealed`,
`GateAbandoned` and `GateCycle` refusals. *(`GateCycle` added by DI-2.4,
[DI-D-23](DESIGN-NOTES.md#di-d-23).)* `durable_through` and `sealed_through` take a `Lineage` and
return the id. The value inside is the **epoch id**, the engineer's term, because it need not be a number.
`Epoch` is only `PartialOrd`: epochs of different lineages are unrelated, so comparing them
answers `None`, as `OpId` does across instances.

**The lineage comes from each write, not from the file.** A file is added with no lineage. A
file written by several lineages shares its failure scope among them: a failed flush on it puts
at risk, at the least, the uncovered writes of every lineage that wrote there, and the failure
belongs to each. A flush made for one lineage may make another's completed writes durable but does
not cover them; each lineage advances only by its own seal. So `FileBusy` reports its gates and
uncovered writes per lineage.

**Failures per lineage** (DI-2.9 point 5). A failure belongs to every lineage with a write in its
suspect set, and each suspect write carries its `Epoch`, so the lineages are read from the set
rather than stored beside it. Which writes the set holds is [CONTRACT.md](CONTRACT.md)'s rule:
today every uncovered write in the instance, because dioring cannot see which writes share a
device's cache. So a failed flush reaches every lineage with an uncovered write anywhere, not only
those that wrote the failed file. A consumer narrows it by declaring each file's flush domains
([DI-D-21](DESIGN-NOTES.md#di-d-21), DI-2.10). An imported failure names its
scope: one lineage, or all of them (`ImportScope`). Within each lineage the failure behaves as
[DI-D-12](DESIGN-NOTES.md#di-d-12) says, and one resolution -- heal, abandon, close -- applies
to it in every lineage it belongs to.

**Ordering per lineage** (DI-2.9 point 6). `Durable { through }` follows the completions of its
own lineage's writes at or below `through`, and one deliverer serves every lineage, in queue order
([DI-D-18](DESIGN-NOTES.md#di-d-18)). Nothing further is promised about order between lineages.

**Lineages are dioring-minted handles.** `mint_lineage(description)` makes one at any time, with
an optional UTF-8 description that is not part of the identity. `lineages()` enumerates them as
owned `LineageInfo`. `default_lineage()` names the lineage every instance has; the enumeration
marks it, and it cannot be retired. `retire_lineage` retires an empty lineage -- nothing in
flight or held, no uncovered write, no unresolved failure -- and a refusal reports each hold in
`LineageBusy`. A retired handle is refused from then on and never reused.

**A gate may cross lineages.** A data write can be held until the log lineage is durable through
the page's LSN, which is section 1's database example inside one instance.

### 13. Flush domains

*(DI-2.10, [DI-D-21](DESIGN-NOTES.md#di-d-21).)* A file may be given a set of **flush domains** --
the disks, controller caches, virtual disks or shares its writes depend on -- through
`FileOptions::domains`, taken by `add_file_with` and by `FileSetup` at construction. The set is
fixed for the file's life in the instance; to change it, remove the file and add it again, which
`remove_file` refuses while the file still has uncovered writes.

A `FlushDomain` is the consumer's exact identifier bytes, so two instances or processes name one
domain alike; dioring interns each distinct domain into a word-sized value for its own
comparisons. A failed flush on a file reaches every file whose set intersects the failed file's.
A consumer provider answers per domain (section 15), so its failure of domain D is placed more
precisely: it reaches every file whose set contains D ([DI-D-27](DESIGN-NOTES.md#di-d-27)).
A file declared with none is unknown and intersects every file, so declaring nothing is one
flush domain for the whole instance. `ImportScope::Domains` scopes an imported failure the same
way, and `Cause::Imported` reports the scope it was given.

Declarations must be complete: extra domains are safe, a missing one is the one way a declaration
can make the contract false ([CONTRACT.md](CONTRACT.md)). Deriving the bytes from Windows' storage
topology is a locality helper's job outside this crate (root `M39`).

### 14. Ending an instance

*(DI-2.7, [DI-D-25](DESIGN-NOTES.md#di-d-25).)* `close(self)` and drop end an instance the same
way -- no final seal or flush, and a wait for every kernel operation and durability-provider call
still in flight -- and `close()` also returns `Leftovers`: the gated operations the kernel never
saw (`HeldOperation`, with buffer and context), the unresolved failures, and every lineage with its
final high-water mark. Dropping discards the same things. [CONTRACT.md](CONTRACT.md) states the
rules.

### 15. Durability providers

*(DI-2.12, [DI-D-27](DESIGN-NOTES.md#di-d-27).)* A seal is made durable by **durability
providers**. The built-in default flushes each file through the instance's own ring and is the only
code that touches it. `Setup::provider` may add **one** consumer `DurabilityProvider`, which states
the flush domains it serves when the instance is built (a domain named twice refuses construction);
everything else, files declared with no domains included, is the default's. Several providers are
composed by a router dioring ships, itself a provider (DI-3.8).

Once every write a seal covers has completed, dioring hands the provider one `FlushRequest`: per
domain it serves, the files (`FileKey`, `SharedHandle`) and the `OpId`s named for each -- identities,
never extents -- and one `DomainCompletion`. The provider flushes by any means, returns promptly,
and answers each domain once with `succeed()` or `fail(error)`, from any thread, at any time. A
completion dropped unanswered is `Cause::ProviderAbandoned`; a failure is `Cause::Provider`. A write
is durable when every domain of its file has succeeded. dioring sets no timeout: a call that never
answers leaves its seal pending, visible through the ETW provider-boundary event
([DI-D-26](DESIGN-NOTES.md#di-d-26)), and ending the instance waits for it.

### 16. The readiness signal

*([DI-D-28](DESIGN-NOTES.md#di-d-28).)* `readiness()` hands back a duplicate of an auto-reset event
the instance owns, set after an entry becomes poppable -- by dioring's `EventDelivery` callback as
it records ring completions, and by provider answers. The waiter -- one per instance -- pops until
`None` after every wake before waiting again; a wake with nothing to pop is normal.

### 17. The contract as a trait

*(DI-2.15, [DI-D-24](DESIGN-NOTES.md#di-d-24).)* The contract is the trait `DurableRing`, generic
only, with the registered-buffer operations in an extension, `RegisteredBufferRing`. dioring's
implementation of both is `Dioring<B, E, C, R>`; construction stays on it (`Dioring::new(Setup)`),
as it will on every implementation.

- **Identity types are each implementation's.** `Lineage`, `OpId`, `FailureId` and the failure
  token are bundled, with the consumer's epoch-id type, in one trait, `Identities`, implemented by a
  zero-sized marker -- dioring's is `DioringIds<E>`. `DurableRing::Ids` names it.
- **Shared types that carry an identity are generic over that one parameter**: `Epoch<V>`,
  `Entry<V, B, C>`, `SuspectWrite<V>`, `PushError<V, B, C>`, `WriteOptions<V>` and the rest. That
  was forced rather than chosen: DI-D-24 kept `Epoch` a shared value, but an epoch holds a lineage,
  so the shared types could not stay concrete once the identities became associated types. One
  bundle keeps it to one parameter; `Entry` alone would otherwise need seven.
- **Plain values stay plain**: `FileKey`, `FlushDomain`, `FileOptions`, `WriteCaching`,
  `Resolution`, `DomainCompletion`.
- **Shorthands** name an implementation's types from the ring type: `Lin<D>`, `OpIdOf<D>`,
  `FailureIdOf<D>`, `TokenOf<D>`, `EpochIdOf<D>`, `EntryOf<D>`, `PushResult<D>`.
- **A consumer written against the contract** bounds the trait and, if it needs one, the epoch-id
  type: `D: DurableRing<Ids: Identities<EpochId = Lsn>>`. The sketch's `seal_log` is one, and it
  works unchanged over dioring, the fault-injecting implementation (DI-3.3), or a layer above.
- **The durability provider is generic too**, `DurabilityProvider<V>`, since its request names the
  implementation's `OpId`s.
- **A file is a `SharedHandle`** ([DI-D-29](DESIGN-NOTES.md#di-d-29)), from the workspace crate
  `win-shared-os-owned-handle`, so no ring-crate type appears in the core trait. It is an owning,
  cheaply shared handle that lends its handle to consumers and providers. dioring hands the ring
  crate the same handle, never a duplicate, which needs `windows-ioring-sys` `M31.3`.

### 18. Ending a lineage

*(DI-2.13, [DI-D-30](DESIGN-NOTES.md#di-d-30).)* `end_lineage(lineage)` is for a holder that will
not finish a lineage. It abandons every epoch of the lineage not yet durable -- sealed ones whose
flushes are still in flight included -- refuses the handle at once, and retires the lineage once
its operations in flight have drained, so the consumer need not retry `retire_lineage`. It waits
for nothing and cancels no I/O: operations in flight complete normally and come back through
`pop`. Gated writes on its epochs, in any lineage, end as `NeverIssued`, and new gates on them are
refused as `GateAbandoned`, as for any abandoned epoch. A failure shared with another lineage stays
unresolved for that lineage. `LineageEnded` reports it, in order with every other entry. The
default lineage cannot be ended (`EndLineageError::Default`).

## Left to other items

- Names `Entry`, `DurableRing`, `Dioring`, `Identities`, `EpochId`, `FileKey`, `Setup` and
  `Lineage` are placeholders.
- **Not proposed:** a gate naming another instance's epoch, which would be C-3's cross-instance join.
  Today the consumer does that join.

**The gate rule** in contract guarantee 9 -- released when the high-water mark reaches the gate and the
gate epoch is durable; fails if it was abandoned -- was derived from DI-1.2 and confirmed by the
engineer.

## The sketch

```rust,ignore
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::io;
use std::marker::PhantomData;
use std::os::windows::io::OwnedHandle;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::{Arc, Weak};

use win_shared_os_owned_handle::SharedHandle;
use windows_ioring_sys::{
    IoBuf, IoBufMut, IoRing, RegisteredBuffers, RegisteredFile, RegisteredSpan, SharedFile,
};

// ---------------------------------------------------------------------------------------------
// The contract as traits (DI-2.15, DI-D-24). Everything below the traits that is generic over
// `V: Identities` is shared by every implementation; the identity types themselves are each
// implementation's own.
// ---------------------------------------------------------------------------------------------

/// What dioring requires of an epoch id, the ordered value within a lineage that names an epoch.
/// The consumer chooses the type -- a `u64` (the default), an `Lsn` newtype, a
/// `(segment, offset)` pair -- and dioring only ever compares and stores it. "Id" rather than
/// "number" because it need not be one.
///
/// Two requirements the compiler cannot check, stated in CONTRACT.md:
/// - `Ord` is a total order that stays consistent for the life of the instance;
/// - values never wrap: a sequence that compares with wraparound (as jbd2's 32-bit transaction IDs
///   do) is not a total order, and must be mapped onto one that does not wrap.
pub trait EpochId: Copy + Ord + Debug {}

impl<T: Copy + Ord + Debug> EpochId for T {}

/// The identity types an implementation mints, and the consumer's epoch-id type (DI-D-24). Each
/// identity carries its implementation's private provenance, so only that implementation can
/// make one, and another instance's is refused. A zero-sized marker implements this; the shared
/// types below are generic over it, so a consumer names one parameter, `D::Ids`, not five.
pub trait Identities: Copy + Debug + Eq + Hash + 'static {
    /// The consumer's epoch-id type.
    type EpochId: EpochId;
    /// A durability lineage (DI-D-19).
    type Lineage: Copy + Debug + Eq + Hash;
    /// One pushed operation. Ordered by push order within an instance; `None` across instances.
    type OpId: Copy + Debug + Eq + Hash + PartialOrd;
    /// A durability failure's `Copy` identity, used in events and queries.
    type FailureId: Copy + Debug + Eq + Hash;
    /// The only thing that can resolve a failure: affine, move-only, and dropping it is closing
    /// it (DI-D-12).
    type FailureToken: Debug;

    /// The failure a token resolves.
    fn token_id(token: &Self::FailureToken) -> Self::FailureId;
}

/// The contract: an instance that takes tagged writes and reports durability by epoch
/// (CONTRACT.md). Generic only -- consumers write `D: DurableRing` -- and construction stays on
/// each implementation (DI-D-24). The registered-buffer operations are an extension,
/// [`RegisteredBufferRing`], since an implementation without a wioring may have none.
pub trait DurableRing {
    type Ids: Identities;
    /// The owned buffer type of writes and reads.
    type Buffer;
    /// The consumer's per-operation context, handed back with each completion.
    type Context;

    /// A duplicate of the readiness signal (DI-D-28): an auto-reset event the instance owns, set
    /// after an entry becomes poppable and at least whenever the queue goes from empty to
    /// non-empty. One waiter; pop until `None` after every wake; a wake with nothing to pop is
    /// normal.
    fn readiness(&mut self) -> io::Result<OwnedHandle>;

    /// `add_file_with` with default options: no declared flush domains.
    fn add_file(&mut self, key: FileKey, file: SharedHandle) -> Result<(), AddFileError> {
        self.add_file_with(key, file, FileOptions::new())
    }

    fn add_file_with(
        &mut self,
        key: FileKey,
        file: SharedHandle,
        options: FileOptions,
    ) -> Result<(), AddFileError>;

    /// Returns the consumer's file, or everything still holding it.
    fn remove_file(&mut self, key: FileKey) -> Result<SharedHandle, RemoveFileError<Self::Ids>>;

    /// Mint a lineage. The description, if any, is for people reading reports and enumerations;
    /// it is not part of the identity and need not be unique.
    fn mint_lineage(&mut self, description: Option<String>) -> Lin<Self>;

    /// Retire an empty lineage. Its handle is refused from then on and never reused.
    fn retire_lineage(&mut self, lineage: Lin<Self>) -> Result<(), RetireLineageError<Self::Ids>>;

    /// End a lineage its holder will not finish (DI-2.13, DI-D-30): every epoch of it not yet
    /// durable is abandoned, its handle is refused at once, and it is retired once its operations
    /// in flight have drained. Waits for nothing and cancels no I/O. The default lineage cannot be
    /// ended.
    fn end_lineage(&mut self, lineage: Lin<Self>) -> Result<(), EndLineageError<Self::Ids>>;

    /// The lineage that always exists.
    fn default_lineage(&self) -> Lin<Self>;

    /// Every live lineage, the default among them.
    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>>;

    /// `write_with` with default options: no gate, cached.
    fn write(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        epoch: Epoch<Self::Ids>,
        context: Self::Context,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBuf,
    {
        self.write_with(file, offset, buffer, epoch, context, WriteOptions::new())
    }

    fn write_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        epoch: Epoch<Self::Ids>,
        context: Self::Context,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBuf;

    /// `read_with` with default options: no gate.
    fn read(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        context: Self::Context,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBufMut,
    {
        self.read_with(file, offset, buffer, context, ReadOptions::new())
    }

    fn read_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: Self::Buffer,
        context: Self::Context,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        Self::Buffer: IoBufMut;

    fn make_durable_through(
        &mut self,
        through: Epoch<Self::Ids>,
    ) -> io::Result<DurabilityRequest<Self::Ids>>;

    fn durable_through(&self, lineage: Lin<Self>) -> Option<EpochIdOf<Self>>;

    fn sealed_through(&self, lineage: Lin<Self>) -> Option<EpochIdOf<Self>>;

    fn epoch_state(&self, epoch: Epoch<Self::Ids>) -> EpochState<Self::Ids>;

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>>;

    fn resolve(
        &mut self,
        items: Vec<(TokenOf<Self>, Resolution)>,
    ) -> Result<(), ResolveError<Self::Ids>>;

    fn import_failure(&mut self, scope: ImportScope<Self::Ids>) -> FailureIdOf<Self>;

    fn failures(&self) -> Vec<FailureInfo<Self::Ids>>;

    fn take_token(&mut self, failure: FailureIdOf<Self>) -> Option<TokenOf<Self>>;

    /// End the instance and hand back what is left (DI-2.7). Waits, as drop does, until every
    /// kernel operation and every durability-provider call in flight has completed.
    fn close(self) -> Leftovers<Self::Ids, Self::Buffer, Self::Context>
    where
        Self: Sized;
}

/// The registered-buffer extension (DI-D-24): operations on spans of buffers registered with the
/// instance's ring when it was built.
pub trait RegisteredBufferRing: DurableRing {
    /// The registered-buffer type.
    type Registered: IoBufMut;

    /// The bytes of registered buffer `i`; refused while an operation is writing into it.
    fn registered_buffer(&mut self, i: u32) -> io::Result<&[u8]>;

    /// The bytes of registered buffer `i`, mutably; refused while any operation uses it.
    fn registered_buffer_mut(&mut self, i: u32) -> io::Result<&mut [u8]>;

    fn write_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        epoch: Epoch<Self::Ids>,
        context: Self::Context,
    ) -> PushResult<Self> {
        self.write_registered_with(file, offset, span, epoch, context, WriteOptions::new())
    }

    fn write_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        epoch: Epoch<Self::Ids>,
        context: Self::Context,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>;

    fn read_registered(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: Self::Context,
    ) -> PushResult<Self> {
        self.read_registered_with(file, offset, span, context, ReadOptions::new())
    }

    fn read_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: Self::Context,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>;
}

/// Shorthands for an implementation's identity types.
pub type Lin<D> = <<D as DurableRing>::Ids as Identities>::Lineage;
pub type OpIdOf<D> = <<D as DurableRing>::Ids as Identities>::OpId;
pub type FailureIdOf<D> = <<D as DurableRing>::Ids as Identities>::FailureId;
pub type TokenOf<D> = <<D as DurableRing>::Ids as Identities>::FailureToken;
pub type EpochIdOf<D> = <<D as DurableRing>::Ids as Identities>::EpochId;
pub type EntryOf<D> =
    Entry<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>;
pub type PushResult<D> = Result<
    OpIdOf<D>,
    PushError<<D as DurableRing>::Ids, <D as DurableRing>::Buffer, <D as DurableRing>::Context>,
>;

// ---------------------------------------------------------------------------------------------
// Shared values. Those that carry an identity are generic over the implementation's
// `Identities`; the rest are plain.
// ---------------------------------------------------------------------------------------------

/// The consumer's identity for a file, supplied when the file is given to dioring and used in
/// every report, so a report stays meaningful after the file's handle is gone (DI-D-11).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileKey(pub u64);

/// An epoch: an epoch id within a lineage. Every place the API takes or reports an epoch uses
/// this, so the lineage is always explicit.
///
/// Only `PartialOrd`: epochs of different lineages are unrelated, so `partial_cmp` is `None`
/// for them, and within one lineage they order by id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Epoch<V: Identities> {
    pub lineage: V::Lineage,
    pub id: V::EpochId,
}

impl<V: Identities> Epoch<V> {
    pub fn new(lineage: V::Lineage, id: V::EpochId) -> Self {
        Self { lineage, id }
    }
}

impl<V: Identities> PartialOrd for Epoch<V> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (self.lineage == other.lineage).then(|| self.id.cmp(&other.id))
    }
}

/// One lineage, as the enumeration reports it. Owned, so holding it borrows nothing.
#[derive(Clone, Debug)]
pub struct LineageInfo<V: Identities> {
    pub lineage: V::Lineage,
    /// The consumer's description, if one was supplied when the lineage was minted.
    pub description: Option<Arc<str>>,
    /// The instance's default lineage, which always exists and cannot be retired.
    pub is_default: bool,
    pub durable_through: Option<V::EpochId>,
    pub sealed_through: Option<V::EpochId>,
}

/// One write a failure put at risk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuspectWrite<V: Identities> {
    pub op: V::OpId,
    pub file: FileKey,
    pub epoch: Epoch<V>,
}

/// A failure's suspect set: frozen when the failure is observed, shared rather than copied.
#[derive(Clone, Debug)]
pub struct SuspectSet<V: Identities>(Arc<[SuspectWrite<V>]>);

impl<V: Identities> SuspectSet<V> {
    pub fn writes(&self) -> &[SuspectWrite<V>] {
        &self.0
    }
}

/// Where an imported failure lands (DI-2.10 point 5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportScope<V: Identities> {
    /// Every uncovered write in the instance.
    All,
    /// The uncovered writes of one lineage.
    Lineage(V::Lineage),
    /// The uncovered writes to every file whose declared flush domains intersect these, files
    /// declared with none included.
    Domains(Vec<FlushDomain>),
}

/// A flush domain: a unit whose volatile cache a flush commits and whose failure everything
/// behind it shares -- a disk, a controller cache, a virtual disk, a remote share (DI-D-21).
/// Named by the consumer's exact identifier bytes, so two instances or processes name one
/// domain alike. dioring interns each distinct domain into a word-sized value of its own, so
/// comparisons after declaration are cheap, and reports return the bytes.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FlushDomain(Arc<[u8]>);

impl FlushDomain {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self(bytes.into())
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

/// The advanced parameters of giving a file to dioring, taken by `add_file_with` and
/// `FileSetup`. Set once, when the file is given; to change them, remove the file and add it
/// again (DI-2.10 point 3).
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct FileOptions {
    domains: Vec<FlushDomain>,
}

impl FileOptions {
    /// No declared flush domains: the file is unknown and shares fate with every file.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare every flush domain the file's writes depend on. The declaration must be
    /// complete; extra domains are safe, a missing one is not (CONTRACT.md).
    pub fn domains(mut self, domains: impl IntoIterator<Item = FlushDomain>) -> Self {
        self.domains = domains.into_iter().collect();
        self
    }
}

/// Why a failure exists. A failed or short *write* is never a cause (DI-D-12 (a)). The lineages a
/// failure belongs to are those of the writes in its suspect set.
#[derive(Clone, Debug)]
pub enum Cause<V: Identities> {
    /// The built-in default provider's flush of `file` failed.
    Flush { file: FileKey, error: Arc<io::Error> },
    Imported { scope: ImportScope<V> },
    /// The consumer's provider reported `domain` failed (DI-D-27).
    Provider { domain: FlushDomain, error: Arc<io::Error> },
    /// The consumer's provider dropped `domain`'s completion without answering (DI-D-27).
    ProviderAbandoned { domain: FlushDomain },
}

/// The payload of a `Failed` entry: the one place a token is dispensed besides the inventory.
#[derive(Debug)]
pub struct Failed<V: Identities> {
    pub id: V::FailureId,
    pub token: V::FailureToken,
    pub cause: Cause<V>,
    pub suspect: SuspectSet<V>,
}

/// What a consumer operation was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind<V: Identities> {
    Read,
    Write { epoch: Epoch<V> },
}

/// How a consumer operation ended.
#[derive(Debug)]
pub enum Outcome<V: Identities> {
    /// Completed; the count is what the completion reported, which may be short.
    Transferred(u32),
    /// The operation reached the kernel and failed.
    Failed(io::Error),
    /// A gated operation whose epoch was abandoned: it never reached the kernel, so nothing was
    /// written.
    NeverIssued { abandoned: Epoch<V> },
}

/// A consumer operation's completion, passed through the instance's queue. `buffer` is `None`
/// for an operation on a registered span: the bytes belong to the registration, not to the
/// operation.
#[derive(Debug)]
pub struct OpCompletion<V: Identities, B, C> {
    pub id: V::OpId,
    pub kind: OpKind<V>,
    pub outcome: Outcome<V>,
    pub buffer: Option<B>,
    pub context: C,
}

/// One entry of the instance's completion queue (DI-D-13).
#[derive(Debug)]
pub enum Entry<V: Identities, B, C> {
    Op(OpCompletion<V, B, C>),
    Durable { through: Epoch<V> },
    Failed(Failed<V>),
    /// Fixed-size and allocation-free, so producing it is cheap.
    Blocked { through: Epoch<V>, by: V::FailureId },
    Abandoned { failure: V::FailureId, suspect: SuspectSet<V> },
    /// `end_lineage` took effect: every epoch of `lineage` not yet durable, through
    /// `abandoned_through`, is abandoned (`None` if none was pending).
    LineageEnded {
        lineage: V::Lineage,
        abandoned_through: Option<V::EpochId>,
    },
}

/// An epoch's state as the instance sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochState<V: Identities> {
    Open,
    Pending,
    Blocked(V::FailureId),
    Durable,
    Abandoned,
}

/// The immediate answer to `make_durable_through`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurabilityRequest<V: Identities> {
    /// A new seal; its answer arrives on the queue as `Durable`, `Failed` or `Blocked`.
    Submitted,
    /// At or below an existing seal: a no-op reporting the epoch's state.
    AlreadySealed(EpochState<V>),
}

/// Why a push was refused before anything was reserved.
#[derive(Debug)]
pub enum PushRefusal<V: Identities> {
    /// The write's epoch is at or below its lineage's seal point (DI-D-9 rule a).
    Sealed { epoch: Epoch<V>, sealed_through: V::EpochId },
    /// The lineage was never minted by this instance, or has been retired.
    UnknownLineage(V::Lineage),
    /// The gate names an epoch already abandoned, so the operation could never be released.
    GateAbandoned { gate: Epoch<V> },
    /// The gate would close a cycle: releasing it already depends on this write's own epoch
    /// becoming durable ([DI-D-23](DESIGN-NOTES.md#di-d-23)). `cycle` names the epochs involved,
    /// starting at the write's own and ending at the gate.
    GateCycle { cycle: Vec<Epoch<V>> },
    /// No file was given to the instance under this key.
    UnknownFile(FileKey),
    /// A registered-span operation on an instance built without registered buffers.
    NoRegisteredBuffers,
    /// The underlying ring refused the push.
    Ring(io::Error),
}

/// A refused push hands back what it was given: the buffer (`None` for a registered-span
/// operation, which has none) and the consumer's context. `windows-ioring-sys` hands both back
/// on its own refusals (its D-80), so this holds for a `Ring` refusal too.
#[derive(Debug)]
pub struct PushError<V: Identities, B, C> {
    pub reason: PushRefusal<V>,
    pub buffer: Option<B>,
    pub context: C,
}

/// Whether a write may rest in the system cache (DI-2.6). The contract's own type, so an
/// implementation needs nothing from the ring crate for it; dioring passes it through as the
/// write's flag. It shapes latency only: a write-through write is still durable only when its
/// epoch is sealed, because device-level write-through (FUA) is not relied on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WriteCaching {
    /// The write may be satisfied into the system cache.
    #[default]
    Cached,
    /// Ask the system not to leave the data in its cache.
    WriteThrough,
}

/// The advanced parameters of a write, taken by `write_with` and `write_registered_with`.
/// Passed by value and built with setters, like `windows-ioring-sys`' `PushOptions`; the fields
/// are private, so adding one later is not a breaking change. A gate may name an epoch in any
/// lineage of this instance, including another lineage than the write's own.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct WriteOptions<V: Identities> {
    gate: Option<Epoch<V>>,
    caching: WriteCaching,
}

impl<V: Identities> WriteOptions<V> {
    /// No gate, and `WriteCaching::Cached`.
    pub fn new() -> Self {
        Self {
            gate: None,
            caching: WriteCaching::Cached,
        }
    }

    /// Whether the write may rest in the system cache (DI-2.6).
    pub fn caching(mut self, caching: WriteCaching) -> Self {
        self.caching = caching;
        self
    }

    /// Hold the write until `epoch` is durable; it fails as `NeverIssued` if `epoch` is
    /// abandoned (contract guarantee 9).
    pub fn gate(mut self, epoch: Epoch<V>) -> Self {
        self.gate = Some(epoch);
        self
    }
}

impl<V: Identities> Default for WriteOptions<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// The advanced parameters of a read, taken by `read_with` and `read_registered_with`. Reads take
/// no part in durability, so a gate is their only option.
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct ReadOptions<V: Identities> {
    gate: Option<Epoch<V>>,
}

impl<V: Identities> ReadOptions<V> {
    /// No gate.
    pub fn new() -> Self {
        Self { gate: None }
    }

    /// Hold the read until `epoch` is durable.
    pub fn gate(mut self, epoch: Epoch<V>) -> Self {
        self.gate = Some(epoch);
        self
    }
}

impl<V: Identities> Default for ReadOptions<V> {
    fn default() -> Self {
        Self::new()
    }
}

/// How a failure is resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Heal,
    Abandon,
}

/// Why a resolution call was refused as a whole.
#[derive(Debug)]
pub enum ResolveRefusal<V: Identities> {
    Foreign(V::FailureId),
}

/// A refused resolution hands every token back.
#[derive(Debug)]
pub struct ResolveError<V: Identities> {
    pub reason: ResolveRefusal<V>,
    pub returned: Vec<(V::FailureToken, Resolution)>,
}

/// One unresolved failure, as the inventory reports it. Owned, so holding it borrows nothing.
#[derive(Clone, Debug)]
pub struct FailureInfo<V: Identities> {
    pub id: V::FailureId,
    pub cause: Cause<V>,
    pub suspect: SuspectSet<V>,
    pub token_live: bool,
}

/// An operation held for its gate and never issued, handed back by `close()` (DI-2.7).
#[derive(Debug)]
pub struct HeldOperation<V: Identities, B, C> {
    pub id: V::OpId,
    pub kind: OpKind<V>,
    /// The epoch it was waiting on.
    pub gate: Epoch<V>,
    /// `None` for an operation on a registered span.
    pub buffer: Option<B>,
    pub context: C,
}

/// What `close()` hands back once the instance has ended (DI-2.7). Dropping the instance discards
/// the same things.
#[derive(Debug)]
pub struct Leftovers<V: Identities, B, C> {
    /// Gated operations the kernel never saw, in push order.
    pub held: Vec<HeldOperation<V, B, C>>,
    /// Failures still unresolved.
    pub failures: Vec<FailureInfo<V>>,
    /// Every lineage, carrying its final high-water mark.
    pub lineages: Vec<LineageInfo<V>>,
}

/// A refused `add_file`: a file is already present under `key`. The file and its options are
/// handed back.
#[derive(Debug)]
pub struct AddFileError {
    pub key: FileKey,
    pub file: SharedHandle,
    pub options: FileOptions,
}

/// Everything still holding a file that `remove_file` refused, and what clears each hold. Every
/// field is reported, not only the first that applies, so one refusal says all a consumer has
/// to do.
#[derive(Debug)]
pub struct FileBusy<V: Identities> {
    pub file: FileKey,
    /// Operations the kernel has not completed. Cleared by popping their completions.
    pub in_flight: usize,
    /// Operations held until their gate epoch is durable, and the lowest gate in each lineage
    /// gated on. Cleared when the gates are released, or fail as `NeverIssued`.
    pub held_for_gate: usize,
    pub lowest_gates: Vec<Epoch<V>>,
    /// Writes to the file not yet covered successfully -- named to a provider that answered
    /// success for every domain of the file -- outside any failure's suspect set, and the highest
    /// epoch among them in each lineage that wrote the file. A file stays busy while any lineage
    /// has such a write. Cleared by `make_durable_through` at or above each.
    pub uncovered: usize,
    pub uncovered_through: Vec<Epoch<V>>,
    /// Unresolved failures whose suspect sets include writes to the file. Cleared by resolving
    /// them: heal and seal again, or abandon.
    pub failures: Vec<V::FailureId>,
}

impl<V: Identities> std::fmt::Display for FileBusy<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "file {:?} is still in use:", self.file)?;
        if self.in_flight > 0 {
            write!(
                f,
                " {} operation(s) in flight, pop their completions;",
                self.in_flight
            )?;
        }
        if !self.lowest_gates.is_empty() {
            write!(
                f,
                " {} held until each of {:?} is durable or abandoned;",
                self.held_for_gate, self.lowest_gates
            )?;
        }
        if !self.uncovered_through.is_empty() {
            write!(
                f,
                " {} write(s) not yet covered, seal through each of {:?};",
                self.uncovered, self.uncovered_through
            )?;
        }
        if !self.failures.is_empty() {
            write!(
                f,
                " writes suspect in unresolved failure(s) {:?}, heal or abandon them",
                self.failures
            )?;
        }
        Ok(())
    }
}

/// Why `remove_file` was refused.
#[derive(Debug)]
pub enum RemoveFileError<V: Identities> {
    /// No file is present under this key.
    UnknownKey(FileKey),
    /// The file is still held; see [`FileBusy`].
    Busy(FileBusy<V>),
}

/// Everything still holding a lineage that `retire_lineage` refused, and what clears each hold.
#[derive(Debug)]
pub struct LineageBusy<V: Identities> {
    pub lineage: V::Lineage,
    /// Operations in the lineage the kernel has not completed. Cleared by popping them.
    pub in_flight: usize,
    /// Operations held for a gate in the lineage, or gated on one of its epochs.
    pub held_for_gate: usize,
    /// Writes in the lineage not yet covered successfully. Cleared by sealing them.
    pub uncovered: usize,
    /// Unresolved failures in the lineage. Cleared by healing or abandoning them.
    pub failures: Vec<V::FailureId>,
}

/// Why `end_lineage` was refused.
#[derive(Debug)]
pub enum EndLineageError<V: Identities> {
    /// Never minted by this instance, or already ended or retired.
    UnknownLineage(V::Lineage),
    /// The default lineage cannot be ended; end the instance instead.
    Default,
}

/// Why `retire_lineage` was refused.
#[derive(Debug)]
pub enum RetireLineageError<V: Identities> {
    /// Never minted by this instance, or already retired.
    UnknownLineage(V::Lineage),
    /// The default lineage always exists.
    Default,
    /// The lineage is not empty; see [`LineageBusy`].
    Busy(LineageBusy<V>),
}

/// A consumer-supplied durability provider (DI-2.12, DI-D-27). It makes writes the instance has
/// seen complete durable, by any means, and answers per flush domain. An instance holds at most
/// one; several are composed by a router that is itself a provider. Every other domain, and
/// every file declared with none, is the built-in default's.
pub trait DurabilityProvider<V: Identities>: Send + Debug {
    /// The flush domains this provider serves, read once when the instance is built.
    fn domains(&self) -> Vec<FlushDomain>;

    /// Make the named writes durable. Returns promptly; the work may finish later, on any thread,
    /// through each domain's completion. Holds no reference to the instance.
    fn make_durable(&mut self, request: FlushRequest<V>);
}

/// One seal's work for the consumer's provider.
#[derive(Debug)]
pub struct FlushRequest<V: Identities> {
    pub domains: Vec<DomainRequest<V>>,
}

/// One flush domain's part of a request, and the handle that answers it.
#[derive(Debug)]
pub struct DomainRequest<V: Identities> {
    pub domain: FlushDomain,
    pub files: Vec<FileWrites<V>>,
    pub completion: DomainCompletion,
}

/// A file in one domain's part, and the writes to it named for that domain: identities, never
/// extents. Every one has completed.
#[derive(Debug)]
pub struct FileWrites<V: Identities> {
    pub key: FileKey,
    pub file: SharedHandle,
    pub writes: Vec<V::OpId>,
}

/// Answers one domain of one request, once, from any thread at any time -- inside
/// `make_durable` included. Success warrants that every write named for the domain is durable
/// (CONTRACT.md). Dropped unanswered, it is a failure of the domain, `Cause::ProviderAbandoned`.
/// Answering records the outcome and sets the readiness signal; it never runs delivery.
#[derive(Debug)]
pub struct DomainCompletion {
    core: Weak<()>,
}

impl DomainCompletion {
    pub fn succeed(self) {
        todo!()
    }

    pub fn fail(self, error: io::Error) {
        todo!()
    }
}

// ---------------------------------------------------------------------------------------------
// dioring: the implementation over `windows-ioring-sys`.
// ---------------------------------------------------------------------------------------------

/// Which instance minted a value. Private: it exists so foreign values can be refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct InstanceId(u64);

/// dioring's lineage: minted at any time, never reused, including after retirement. Carries its
/// instance, so another instance's lineage is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Lineage {
    instance: InstanceId,
    seq: u64,
}

/// dioring's identity for one pushed operation. Ordered by push order within an instance;
/// identities from two instances are incomparable (`partial_cmp` is `None`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OpId {
    instance: InstanceId,
    seq: u64,
}

impl PartialOrd for OpId {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (self.instance == other.instance).then(|| self.seq.cmp(&other.seq))
    }
}

/// dioring's failure identity: `Copy`, never reused within an instance. It cannot resolve
/// anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FailureId {
    instance: InstanceId,
    seq: u64,
}

#[derive(Debug)]
struct FailureCell {
    token_live: AtomicBool,
}

/// dioring's failure token; consumed by `heal`, `abandon` or `close`, and dropping it is `close`
/// (DI-D-12). At most one is live per failure.
#[must_use = "an unresolved failure stalls the high-water mark; heal, abandon, or close it"]
#[derive(Debug)]
pub struct FailureToken {
    id: FailureId,
    cell: Arc<FailureCell>,
}

impl FailureToken {
    pub fn id(&self) -> FailureId {
        self.id
    }

    /// Set the failure aside, unresolved; its token can be taken from the inventory again.
    pub fn close(self) {}
}

impl Drop for FailureToken {
    fn drop(&mut self) {
        self.cell.token_live.store(false, AtomicOrdering::Release);
    }
}

/// dioring's identities, with the consumer's epoch-id type `E`. Zero-sized; it exists only to
/// name them, so its comparison and hashing traits hold for every `E`.
pub struct DioringIds<E>(PhantomData<fn() -> E>);

impl<E> Clone for DioringIds<E> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<E> Copy for DioringIds<E> {}

impl<E> Debug for DioringIds<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DioringIds")
    }
}

impl<E> PartialEq for DioringIds<E> {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl<E> Eq for DioringIds<E> {}

impl<E> Hash for DioringIds<E> {
    fn hash<H: Hasher>(&self, _: &mut H) {}
}

impl<E: EpochId + 'static> Identities for DioringIds<E> {
    type EpochId = E;
    type Lineage = Lineage;
    type OpId = OpId;
    type FailureId = FailureId;
    type FailureToken = FailureToken;

    fn token_id(token: &FailureToken) -> FailureId {
        token.id()
    }
}

/// What a dioring instance is built with. Both registrations are the ring's only ones (one of
/// each per ring), so they are made here or never.
pub struct Setup<E: EpochId + 'static, R> {
    pub submission_queue_size: u32,
    pub completion_queue_size: u32,
    /// Registered with the ring. A file added later with `add_file` is not registered.
    pub files: Vec<FileSetup>,
    /// Already allocated and placed by the consumer; dioring neither allocates nor places them.
    /// Empty registers nothing.
    pub buffers: Vec<R>,
    /// The consumer's durability provider, if any. `None` leaves every domain to the built-in
    /// default.
    pub provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
}

/// One file given at construction.
#[derive(Debug)]
pub struct FileSetup {
    pub key: FileKey,
    pub file: SharedHandle,
    pub options: FileOptions,
}

/// Why construction failed.
#[derive(Debug)]
pub enum SetupRefusal {
    /// Two files share a key.
    DuplicateKey(FileKey),
    /// The provider named a domain twice.
    DuplicateProviderDomain(FlushDomain),
    /// Creating the ring or making a registration failed.
    Ring(io::Error),
}

/// A failed construction hands back what it was given. `buffers` is `None` only when the
/// kernel's registration completion itself failed: `windows-ioring-sys` drops the buffers then.
#[derive(Debug)]
pub struct SetupError<E: EpochId + 'static, R> {
    pub reason: SetupRefusal,
    pub files: Vec<FileSetup>,
    pub buffers: Option<Vec<R>>,
    pub provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
}

/// How dioring addresses one of the consumer's files.
enum FileSlot {
    /// Given at construction: the ring holds it for its life (`windows-ioring-sys` D-81).
    Registered { index: RegisteredFile, file: SharedFile },
    /// Added later: pushed through its handle, guarded per operation.
    Shared(SharedFile),
}

/// The ring crate's view of a consumer's file: the same handle, never a duplicate. Needs
/// `windows-ioring-sys` `M31.3` (`SharedFile` adopts `SharedHandle`); until then this is the one
/// place the two types meet.
fn ring_file(file: &SharedHandle) -> SharedFile {
    todo!()
}

/// A flush domain interned by this instance: a word-sized stand-in for its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DomainId(u32);

/// One of the consumer's files, with its declared flush domains. An empty set means unknown,
/// which intersects every file.
struct FileRecord {
    target: FileSlot,
    domains: Box<[DomainId]>,
}

/// dioring's record of each operation, carried as the ring crate's sidecar. The consumer's
/// context rides inside it, so routing a completion needs no side table.
enum Sidecar<E: EpochId + 'static, C> {
    Consumer {
        id: OpId,
        kind: OpKind<DioringIds<E>>,
        file: FileKey,
        offset: u64,
        context: C,
    },
    Commit {
        through: Epoch<DioringIds<E>>,
        file: FileKey,
    },
}

/// dioring: `B` is the owned buffer type, `E` the epoch-id type, `C` the consumer's
/// per-operation context, and `R` the registered-buffer type.
pub struct Dioring<B, E: EpochId + 'static = u64, C = (), R: IoBufMut = Vec<u8>> {
    instance: InstanceId,
    ring: IoRing<B, Sidecar<E, C>>,
    files: HashMap<FileKey, FileRecord>,
    /// The interning table. An entry lives for the instance's life: the number of distinct
    /// domains is the number of devices and shares the consumer touches.
    domains: HashMap<FlushDomain, DomainId>,
    registered: Option<RegisteredBuffers<R>>,
    provider: Option<Box<dyn DurabilityProvider<DioringIds<E>>>>,
}

impl<B, E: EpochId + 'static, C, R: IoBufMut> Dioring<B, E, C, R> {
    /// Build an instance and make the ring's registrations. Blocks until both have completed.
    pub fn new(setup: Setup<E, R>) -> Result<Self, SetupError<E, R>> {
        todo!()
    }
}

impl<B, E: EpochId + 'static, C, R: IoBufMut> DurableRing for Dioring<B, E, C, R> {
    type Ids = DioringIds<E>;
    type Buffer = B;
    type Context = C;

    fn readiness(&mut self) -> io::Result<OwnedHandle> {
        todo!()
    }

    fn add_file_with(
        &mut self,
        key: FileKey,
        file: SharedHandle,
        options: FileOptions,
    ) -> Result<(), AddFileError> {
        todo!()
    }

    /// A file given at construction stays open until the ring is dropped, because the ring's
    /// registration holds it.
    fn remove_file(&mut self, key: FileKey) -> Result<SharedHandle, RemoveFileError<Self::Ids>> {
        todo!()
    }

    fn mint_lineage(&mut self, description: Option<String>) -> Lineage {
        todo!()
    }

    fn retire_lineage(&mut self, lineage: Lineage) -> Result<(), RetireLineageError<Self::Ids>> {
        todo!()
    }

    fn end_lineage(&mut self, lineage: Lineage) -> Result<(), EndLineageError<Self::Ids>> {
        todo!()
    }

    fn default_lineage(&self) -> Lineage {
        todo!()
    }

    fn lineages(&self) -> Vec<LineageInfo<Self::Ids>> {
        todo!()
    }

    fn write_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: B,
        epoch: Epoch<Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBuf,
    {
        todo!()
    }

    fn read_with(
        &mut self,
        file: FileKey,
        offset: u64,
        buffer: B,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self>
    where
        B: IoBufMut,
    {
        todo!()
    }

    fn make_durable_through(
        &mut self,
        through: Epoch<Self::Ids>,
    ) -> io::Result<DurabilityRequest<Self::Ids>> {
        todo!()
    }

    fn durable_through(&self, lineage: Lineage) -> Option<E> {
        todo!()
    }

    fn sealed_through(&self, lineage: Lineage) -> Option<E> {
        todo!()
    }

    fn epoch_state(&self, epoch: Epoch<Self::Ids>) -> EpochState<Self::Ids> {
        todo!()
    }

    fn pop(&mut self) -> io::Result<Option<EntryOf<Self>>> {
        todo!()
    }

    fn resolve(
        &mut self,
        items: Vec<(FailureToken, Resolution)>,
    ) -> Result<(), ResolveError<Self::Ids>> {
        todo!()
    }

    fn import_failure(&mut self, scope: ImportScope<Self::Ids>) -> FailureId {
        todo!()
    }

    fn failures(&self) -> Vec<FailureInfo<Self::Ids>> {
        todo!()
    }

    fn take_token(&mut self, failure: FailureId) -> Option<FailureToken> {
        todo!()
    }

    fn close(self) -> Leftovers<Self::Ids, B, C> {
        todo!()
    }
}

impl<B, E: EpochId + 'static, C, R: IoBufMut> RegisteredBufferRing for Dioring<B, E, C, R> {
    type Registered = R;

    fn registered_buffer(&mut self, i: u32) -> io::Result<&[u8]> {
        todo!()
    }

    fn registered_buffer_mut(&mut self, i: u32) -> io::Result<&mut [u8]> {
        todo!()
    }

    fn write_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        epoch: Epoch<Self::Ids>,
        context: C,
        options: WriteOptions<Self::Ids>,
    ) -> PushResult<Self> {
        todo!()
    }

    fn read_registered_with(
        &mut self,
        file: FileKey,
        offset: u64,
        span: RegisteredSpan,
        context: C,
        options: ReadOptions<Self::Ids>,
    ) -> PushResult<Self> {
        todo!()
    }
}

// The defaults, a consumer's own epoch-id type and context, and one buffer type for both roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Lsn(pub u64);

pub type DefaultRing = Dioring<Vec<u8>>;
pub type LogRing = Dioring<Vec<u8>, Lsn, u32>;
pub type SegmentRing = Dioring<Arc<[u8]>, (u32, u64)>;

/// A consumer written against the contract rather than against dioring: it works with any
/// implementation whose epoch ids are `Lsn`s -- dioring, the fault-injecting one (DI-3.3), or a
/// layer above.
pub fn seal_log<D>(ring: &mut D, lineage: Lin<D>, lsn: Lsn) -> io::Result<DurabilityRequest<D::Ids>>
where
    D: DurableRing<Ids: Identities<EpochId = Lsn>>,
{
    ring.make_durable_through(Epoch::new(lineage, lsn))
}
```
