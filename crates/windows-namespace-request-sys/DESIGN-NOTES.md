# Design notes: windows-namespace-request-sys

Decisions for this crate, recorded before implementation. Pending work is in the
workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md),
milestones M24 to M26. The workspace-level context -- the data plane versus the
namespace plane, and why the namespace plane is synchronous-only -- is in the
workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md), which is authoritative for
anything this file does not cover.

## Intent

Win32's namespace and metadata surface -- opening, querying, closing -- is
synchronous-only. A call that blocks on a dead network path blocks the thread
that made it, and no overlapped form exists. This crate makes such a call
**capturable as a value**: an owned parameter set that can be built on one
thread and executed faithfully on another.

It does not schedule anything. It is the catalogue-plus-faithful-execution
layer, testable with no ring, no pool, and no async anywhere near it.

## <a id="d-1"></a>D-1: A request excludes ambient context

A request carries call **parameters**. It does not carry the impersonation
token, error mode, or any other thread-scoped state the call runs under; that is
[windows-thread-ambient-sys](../windows-thread-ambient-sys/DESIGN-NOTES.md)'s
subject, and this crate does not depend on it.

The two are **siblings, not a stack**, and the argument is symmetry: a request
can be executed with no captured context at all, and a context is useful to work
that never opens a file. Whoever owns both pairs them at the submission site.

An earlier draft justified this from an audit observation -- that a surveyed
consumer used no ambient state. That was bad evidence and is recorded here so it
is not repeated: a consumer which has not needed something yet says nothing
about whether the layering is right. The claim rests on the symmetry above.

## <a id="d-2"></a>D-2: A request performs its call, and chooses no delivery model

An entry executes its Win32 call faithfully and reports what happened. It does
not decide how the result reaches anyone, and in particular it does **not**
choose what an opened handle is for.

So an opened handle comes back **plain and unassociated**. Associating a handle
with a completion port -- including via `CreateThreadpoolIo` -- permanently
prevents `IoRing` use of it, and the choice must be made before association by a
layer that knows the destination. An entry that associated on the caller's
behalf would foreclose a fork it has no standing to decide.

That is measured, and the measurement is now executable rather than a claim in
prose: see
[windows-platform-probes](../windows-platform-probes/DESIGN-NOTES.md) ->
`d-completion-port`, and `cargo run -p windows-platform-probes --bin
probe-completion-port`. Worth knowing how close it came to being recorded
backwards -- the first version of that probe concluded the *opposite*, because
it checked where the completion arrived rather than whether the operation
succeeded, and so read a refusal (`ERROR_INVALID_PARAMETER`, zero bytes) as a
success. The corrected probe judges a read on its result code, its byte count,
and the bytes it actually landed, and carries the negative controls that make a
failure attributable.

"Faithfully" also constrains how an entry reports failure -- the raw code
preserved unaltered, and snapshotted before any cleanup can overwrite it. That
is stated once, with its mechanism, in [D-10](#d-10) rather than repeated here.

## <a id="d-3"></a>D-3: One entry per Win32 call

A catalogue entry corresponds to exactly one Win32 call. A consumer needing two
makes two requests and sequences them itself, on the client side, observing the
first result before deciding the second.

A compound entry is added only under a **measured** performance argument, and
would be a fusion of entries that already exist rather than a capability they
lack. The cost of not fusing is a real round trip, and that cost is inherent to
remoting rather than particular to this rule: for a *single* operation the
remoted form is always slower than the synchronous one, because the win is
concurrency and thread-freedom rather than latency.

## <a id="d-4"></a>D-4: A request owns duplicates of any handle it names

**Current, with a replacement parked: [NR-inf.1](CHECKLIST.md) would share a `SharedHandle` instead of duplicating (not decided).**

Five of the round-one entries take a handle rather than a path. A request
**duplicates** such a handle at capture and owns the duplicate for its life.

The distinction a caller reasoning in value semantics will get wrong, stated
plainly: **a path is a value and is copied; a handle is a reference to a kernel
object, and duplicating it shares that object rather than cloning it.** So a
request is self-contained with respect to **lifetime** -- it cannot be left
pointing at a handle its originator has closed -- and is **not** isolated with
respect to **state**.

Where that has teeth is measured, not reasoned. See
[windows-platform-probes](../windows-platform-probes/DESIGN-NOTES.md), which
pins these as tests:

| Question | Measured |
|---|---|
| Does a duplicate share directory-enumeration state? | **Yes** -- it continues where the source stopped |
| Control: do two separate opens share it? | **No** -- a separate open restarts |
| Does closing the duplicate disturb the source? | **No** |
| Do single-shot metadata queries disturb an enumeration in progress? | **No**, on the handle or on a duplicate |

The third row is what makes D-4 safe at all: a request may own a duplicate and
drop it without damaging the handle its caller kept. The first means the
contract must say that a duplicate is **not** an independent enumeration and
that an independent traversal needs a fresh open. The fourth narrows the hazard
usefully -- only the two directory-enumeration classes mutate shared state, and
every other query is a pure read that composes freely.

## <a id="d-5"></a>D-5: The round-one entry list is audited, and its omissions are deliberate

The entries below are the union of what three real consumers call:
[windows-file-watcher](../windows-file-watcher/DESIGN-NOTES.md) and
[windows-file-enumeration-sys](../windows-file-enumeration-sys/DESIGN-NOTES.md)
in this repository, and `MikeGrier/Globazog-rs` at commit `55a0b1ae`.

| # | Entry | Needed by |
|---|---|---|
| 1 | `CreateFileW` | all three |
| 2 | `OpenFileById` | watcher |
| 3 | `FindFirstChangeNotificationW` | watcher |
| 4 | `CloseHandle` and variant close routines | all three |
| 5 | `GetFileInformationByHandleEx` | all three |
| 6 | `GetFileInformationByHandle` (non-`Ex`) | watcher |
| 7 | `GetFinalPathNameByHandleW` | watcher; Globazog via `canonicalize` |
| 8 | `GetVolumeInformationByHandleW` | watcher |
| 9 | `GetFullPathNameW` | enumeration |

**Deliberately out of round one**, so a later reader can tell a considered
omission from an unexamined one: `DeleteFileW`, `MoveFileExW`,
`CreateDirectoryW`, `RemoveDirectoryW`, `SetFileAttributesW`,
`SetFileInformationByHandle`, `GetFileAttributesExW`, `CreateSymbolicLinkW`,
`CreateHardLinkW`, `QueryDosDeviceW`, and `FindFirstFileExW` /
`FindNextFileW`. No audited consumer calls any of them. Every audited open is
`OPEN_EXISTING` against a **directory**; nothing creates a file.

Two entries are distinct calls rather than variants, which is easy to miss:
`OpenFileById` is a second open primitive with no creation disposition, and
`GetFileInformationByHandle` is not a class of `GetFileInformationByHandleEx`.

### The `CreateFileW` entry keeps parameters no consumer uses

No audited consumer passes a `SECURITY_ATTRIBUTES` or an `hTemplateFile`; all
pass null. Both are supported anyway.

This is not speculative scope. An entry that cannot express two of its own
call's parameters is a **narrowed** `CreateFileW`, and narrowing a platform
entry to fit the consumers currently in view is the anti-pattern this
repository's platform-integrity rule names. The completeness of a single entry
is a levelness requirement, not a guess about future demand. Recorded here so
the absence of a consumer is not later mistaken for an oversight.

## <a id="d-6"></a>D-6: Membership is decided by blocking, not by marshaling difficulty

An entry belongs in the catalogue because it is a blocking namespace call a
consumer needs performed off its own thread -- **not** because it is awkward to
marshal.

The distinction matters because the easiest entry to build is also one of the
most important. `GetFileInformationByHandleEx` needs almost no marshaling work:
its inputs are a handle, a scalar class, and a buffer size, with no pointer into
caller memory anywhere. By a difficulty test it would look like a candidate for
omission. By the correct test it is the most-called namespace operation across
all three audited consumers, and the call whose lack of an overlapped form is
why an unassociated handle had to become a first-class destination at all.

A catalogue selected by implementation difficulty would be a catalogue shaped by
our convenience rather than by consumer need.

## <a id="d-7"></a>D-7: A handle that cannot be captured fails at construction

Handle capture validates the source and duplicates it on the **calling** thread,
and every way that can go wrong is a construction error. The alternative --
storing the caller's raw value and discovering the problem when the entry runs
-- would report a caller's mistake on a worker, to code holding neither the
source handle nor any way to correct it. Capture happens where the caller can
still do something about it.

Three inputs are refused before `DuplicateHandle` is reached, and the reason is
not tidiness:

- **Null.** Never a handle.
- **`INVALID_HANDLE_VALUE`.** This is the trap that makes validation load-bearing
  rather than defensive. `INVALID_HANDLE_VALUE` and the current-process
  pseudo-handle are the **same value**, `-1`, so `DuplicateHandle` accepts it and
  returns a perfectly valid handle -- to the current process. An unchecked
  `CreateFileW` failure passed straight into capture would therefore *succeed*,
  and the request would carry a process handle where a file handle was meant.
- **The remaining pseudo-handles** (`-2` through `-6`: current thread, the three
  token forms, and the reserved `-3`). A pseudo-handle is not a reference to a
  kernel object; it is a constant the *using* thread resolves against itself. Its
  meaning would therefore change when the request moved to a worker, which is
  precisely what this crate exists to prevent.

Everything else reaches `DuplicateHandle` and is judged by Windows.
An already-closed handle fails there, with `ERROR_INVALID_HANDLE`.

The duplicate is taken with `DUPLICATE_SAME_ACCESS`, because a request must be
able to perform exactly the call its caller opened the handle for, and
non-inheritable, so capturing a handle never widens what a child process
reaches.

Duplication is fallible, so the type offers `try_clone` rather than `Clone`. The
result refers to the same kernel object, with everything D-4 says about that.

## <a id="d-8"></a>D-8: Security attributes are captured as a self-relative, owned blob

A `SECURITY_ATTRIBUTES` is not a value. It points at a security descriptor, and
an **absolute** descriptor is itself a structure of raw pointers to an owner
SID, a group SID, a DACL and a SACL that are quite possibly on the caller's
stack. Copying the struct would copy those pointers, so a request built on a
submitter and run on a worker would be reading the submitter's dead stack
frame. Capture therefore converts to the **self-relative** form -- every part at
an offset inside one contiguous blob -- and owns that blob.

Validation runs at capture, on the calling thread, for the same reason handle
capture does (see [D-7](#d-7)): a descriptor Windows will reject is the caller's
to fix, and reporting it from a worker gives the report to code that cannot act
on it.

### The three-way distinctions this must not collapse

A single nullable pointer runs together outcomes that are different grants, so
each is given its own representation:

| Caller passed | Meaning |
|---|---|
| a null `lpSecurityAttributes` | default security, non-inheritable handle |
| attributes whose descriptor is null | default security, but the caller's inheritance choice |
| attributes with a descriptor | the caller's security |

and inside a descriptor, a **DACL** has three states that a `bool` or an
`Option` would flatten: **absent** (the object takes its default), **NULL**
(everyone gets everything), and **empty** (nobody gets anything). NULL and empty
are opposites, so collapsing them is not a loss of detail -- it is an inversion.
`AclState` keeps all four cases (with a populated list reporting its entry
count), and applies to the SACL as well.

### Alignment is a property of the buffer, not the first field

A self-relative descriptor must live in DWORD-aligned storage. A `Box<[u8]>`
guarantees an alignment of 1, so this is a requirement the obvious
representation silently fails. It is the second such requirement in the crate
-- the directory-information classes need 8-byte alignment for the same
underlying reason -- so it is met once, by an aligned-buffer primitive whose
alignment is stated at construction, rather than being solved twice by two
different local tricks.

That primitive was this crate's own `AlignedBuffer`, and is now `AlignedBuf` from the [io-buffers](../io-buffers/README.md) crate, which the
crate re-exports because the type appears in its public signatures (the query
result and the captured security descriptor's storage). The move is a breaking
change: the old type's constructors panicked on a bad alignment and aborted on
allocation failure, and `AlignedBuf` returns an error for both. The first
version of the move took that error with `expect` at each call site, which left
a panic where there had been an abort; the engineer rejected both (2026-10-13),
and an allocation that fails is now an error the caller receives, at every site:

- `QueryFileInformation::perform` returns `ERROR_NOT_ENOUGH_MEMORY` when the
  buffer for the caller's capacity cannot be allocated. It is the one code in
  this crate that no Win32 call produced, and the call is not made.
- `SecurityDescriptor::capture` returns
  `SecurityCaptureFailure::Allocate`, whose error carries the same code, from
  both of its allocations, which share one function.
- `SecurityDescriptor` and `SecurityAttributes` are no longer `Clone`: copying
  one allocates, and an infallible `clone` could only panic or abort. Each has a
  `try_clone`, which reports `Allocate`; a request's `try_clone` reports it as
  `HandleCaptureFailure::CopySecurity`.

Every call site passes a constant alignment, so the alignment's own refusal
cannot occur; the constants are asserted to be powers of two at build time, and
every refusal of an allocation is then reported as an allocation failure. What
no test reaches is `SecurityDescriptor::try_clone` failing, because it allocates
exactly the size of a buffer that already exists; [sabotage.json](sabotage.json)
declares it. Why the buffer is fallible and clone-less is
[IB-D-6](../io-buffers/DESIGN-NOTES.md#ib-d-6).

## <a id="d-9"></a>D-9: Path preparation is duplicated from the enumeration crate, deliberately and temporarily

**Superseded in part by [D-19](#d-19).** The mechanism below -- resolving with `GetFullPathNameW` when the request is built, refusing a path past `MAX_PATH`, validating a verbatim path -- is replaced by a graft onto a captured root. The need to fix a relative path's meaning at construction, and the duplication with the enumeration crate and its scheduled merge-or-delete decision, stand; once `NR-3.3` lands the two copies differ, so that decision is no longer a plain de-duplication.

Path preparation resolves a caller's path **on the calling thread, when the
request is built**, because the process current directory is shared mutable
state that any thread can change between submission and execution. That is not
a new contract: `windows-file-enumeration-sys` already ships exactly this
preparation, and writing a second one from scratch is what this repository's
mono-repo policy refuses.

So it was **copied**, byte for byte, and adapted only where this crate's error
taxonomy differs. The alternatives were considered and rejected:

- **Depend on the enumeration crate.** It is released; this crate is not. A
  released crate cannot depend on an unpublished `0.0.0` sibling, and the whole
  point of the branch this landed on is a minimal-impact, fast route to
  publication. Pointing the released crate at this one would have made that
  strictly worse, and would also have inverted the layering: the enumeration
  crate sits *above* the namespace catalogue.
- **Extract to a third shared crate.** A new workspace member and a new
  published crate, purchased before the consumer that would justify it exists.
- **Write a second implementation.** The option the mono-repo policy names as
  the wrong one.

The duplication is therefore the mechanism that keeps a working, released crate
stable while this one is proven -- not debt incurred by accident. `path.rs`
carries a provenance comment naming its source and the commit it was taken at,
and the **merge-or-delete decision is scheduled**, not left to be rediscovered:
see `M26+.3` in
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md). It is gated on
this branch merging with `main`, which is when the de-duplication happens. Until
then, a fix to either copy must be applied to both.

## <a id="d-10"></a>D-10: The faithful-execution contract is a primitive entries bind to, not a rule they restate

Every entry reports what Windows reported: the raw code, unaltered, never
normalised or reclassified. `ERROR_FILE_NOT_FOUND` means a missing directory
from an open, an **empty** directory from a first query, and a genuine failure
from a later one -- only a consumer holding that context can tell them apart,
so any interpretation here destroys information no layer above can rebuild.

The harder half is *when* the code is read. `GetLastError` is volatile thread
state: almost any subsequent Win32 call overwrites it, including cleanup nobody
thinks of as a call -- a `CloseHandle` in a `Drop`, a buffer release, a
restoration guard unwinding. Reading it a few statements after the failure is a
race against the entry's own tidying up.

**The contract forbids reclassifying a code; it does not require inventing one.**
Two entries -- `QueryFinalPath` and `ResolveFullPath` -- retry a growing buffer,
and "the required size kept changing" is a failure Windows never reported at all,
so there is no raw code to pass through faithfully. Each reports it as its own
`Unstable` variant alongside a `Win32` variant that still carries the raw code
unaltered. That is additive rather than a reclassification: no Win32 code is
altered, and a failure Win32 did not produce is not dressed up as one.

Stating this explicitly is not pedantry. `ResolveFullPath` originally read the
rule as "an entry must return a Win32 code" and therefore synthesized
`ERROR_INSUFFICIENT_BUFFER` for the unstable case -- a code Windows also returns
on its own, leaving a caller unable to tell the crate's retry giving up from a
genuine Windows refusal. Inventing a code is the same information loss the
paragraph above forbids, just arriving from the other direction. See
[the seam's error-type rationale](src/request.rs).

So it is **not left to each entry's discipline.** `outcome::perform` takes the
call as a closure and snapshots the code in the statement after it returns,
with the convention-specific forms (`perform_bool`, `perform_handle`,
`perform_nonnull_handle`, `perform_nonzero`) layered on it. An entry binds to
that function; it does not re-implement the rule. This is the
derived-rather-than-restated posture the repository's contract-integrity rule
asks for, applied to the one guarantee every entry in the catalogue shares.

Two consequences worth stating because they are easy to get backwards:

- **Success never consults `GetLastError`.** Many Win32 calls leave a non-zero
  last error behind on success, so an entry that checked the error slot rather
  than the return value would invent failures. The return value decides.
- **The two handle conventions are both real and are not interchangeable.**
  `CreateFileW` fails with `INVALID_HANDLE_VALUE`; other calls fail with null.
  Both forms exist by name because using one where the other belongs turns a
  failure into a plausible-looking handle.

Capture failures are *not* governed by this. `handle`, `security`, and `path`
report a named stage plus a code, because there the useful question is which
part of building the request went wrong, and that is answered on the calling
thread before any entry runs.

## <a id="d-11"></a>D-11: A handle carries its close routine

Closing is a catalogue entry, which surprises people. `CloseHandle` looks like
bookkeeping, but it is a blocking namespace call: it waits for outstanding I/O
and can block hard on a dead network path or an ejected device. A consumer that
carefully moved its opens onto a worker and then closed on its own thread would
have moved the wrong half.

The audit then showed a close entry **cannot assume its routine**. A
`FindFirstChangeNotificationW` handle is closed with
`FindCloseChangeNotification`, and `CloseHandle` is wrong for it *silently* --
no error, just a leak. So the routine travels with the handle rather than being
chosen at the call site. This is the same shape
[windows-threadpool-sys](../windows-threadpool-sys/README.md) already needed for
wait targets, and adopting it here is deliberate reuse of a solved problem
rather than a parallel invention.

Two structural consequences:

- **`ChangeNotification` is a type, not an `OwnedHandle`.** `OwnedHandle`'s drop
  is `CloseHandle`, which is exactly the wrong routine, so returning one from
  the watch entry would hand every caller a silent leak. The type is what
  remembers.
- **Closing happens exactly once, enforced by construction.** `CloseRequest::perform`
  consumes the request through a `ManuallyDrop` so the destructor cannot close
  again; both constructors suppress the source value's own drop so it cannot
  close first; and an unperformed request still closes on drop, because a
  request that quietly did nothing would leak.

## <a id="d-12"></a>D-12: An entry carries flags verbatim and defaults nothing

Two of the audited consumers open without `FILE_FLAG_OVERLAPPED` and one opens
with it, because the watcher's handle is destined for a completion port and the
other two are not. The entry carries that as a **field**. Deciding it would be
the delivery-model choice [D-2](#d-2) refuses to make.

The same reasoning forbids helpful defaults. `FILE_FLAG_BACKUP_SEMANTICS` is
mandatory to open a directory at all and every audited consumer passes it -- and
it is still not implied, because an entry that quietly added a flag would be
deciding what the caller meant, and the same field is what a caller opening a
plain file must be able to leave out. Every parameter starts at "the caller said
nothing"; a plausible-looking default is exactly what a caller cannot see they
were given.

This extends to values the crate does not recognise. `NotifyFilter` is a newtype
over a bitmask rather than an enum, so a bit Windows defines and this crate has
never heard of still reaches it unaltered. Its named constants are bound to the
platform's own `FILE_NOTIFY_CHANGE_*` values rather than restated as literals,
so the two cannot drift.

## <a id="d-13"></a>D-13: A tagged union becomes an enum; a dependency's type does not become our surface

`OpenFileById` takes a `FILE_ID_DESCRIPTOR`: a tag plus a union that a caller
must keep in step by hand. `FileIdentifier` is an enum instead, so the tag is
implied by the variant and the two cannot disagree. All three identifier kinds
are supported although only `FileId` appears in the audited consumers, for the
[D-5](#d-5) reason.

Its object-id variant carries a `u128`, not windows-sys's `GUID`. That type
implements neither equality nor `Debug`, and a public surface should not be
shaped by whichever binding crate we happen to build against -- that is
behavior inherited from a dependency rather than owned. The conversion happens
at the FFI boundary, where it belongs.

## <a id="d-14"></a>D-14: The catalogue offers a seam, in two traits because the distinction is real

Every entry is a value whose `perform` is the single point where Win32 is
touched. `Request` and `ConsumingRequest` are the traits over that, so a
*consumer's* code can be written against "a request that produces `T`" and
exercised in that consumer's own tests without a filesystem, a network path, or
a device that may not be present. Offering the seam is a levelness obligation:
a platform whose consumers cannot test against it is not level, whether or not
a consumer has asked yet.

**Two traits, not one.** An open is a parameter set -- it may be performed
repeatedly, producing an independent handle each time, so it takes `&self`. A
close is one-shot; performing it consumes the request, which is what makes
closing twice impossible ([D-11](#d-11)). One trait would have to pick a side
and both choices lie: a `&self` trait makes a close look repeatable, and a
`self` trait makes every open look single-use.

**A seam, not an abstraction layer.** The traits exist for substitution by a
consumer. Nothing in this crate dispatches through them and the entries keep
their inherent `perform` methods, which is what an ordinary caller uses. A test
asserts the two paths agree, so the trait cannot become a second, divergent
implementation.

The traits are object-safe, because a consumer holding heterogeneous requests
needs that, and a test pins it -- object safety is easy to lose by accident and
nothing else would notice.

## <a id="d-15"></a>D-15: Examples are compiled, and cover what a caller gets wrong

Every public type carries a runnable example, and so does every method whose
correct use is not obvious from its signature. They are doctests, so a contract
change breaks the build rather than leaving the documentation quietly teaching
the old answer -- the repository's contract-integrity rule applied to this
crate's prose.

Coverage is chosen by **where a caller goes wrong**, not by what is easy to
demonstrate:

- the three-way security distinction and the absent/NULL/empty DACL
  distinction, where collapsing a pair inverts a grant ([D-8](#d-8));
- the two handle-failure conventions, where using one for the other turns a
  failure into a plausible-looking handle ([D-10](#d-10));
- what a duplicated handle does and does not share, which a caller reasoning in
  value semantics gets backwards ([D-4](#d-4));
- rearming a notification, without which a waiting loop spins;
- that a close is one-shot while an open is repeatable ([D-14](#d-14)).

An example that merely shows a constructor being called would satisfy a count
and teach nothing; these are written to be read by someone about to make the
mistake.

## <a id="d-16"></a>D-16: Acceptance checks coverage twice, because the audit had two purposes

The audit that produced the entry list had two purposes, and the first draft of
this plan discharged only one -- establishing the *operation set* -- while
silently leaving the question of whether the *scenario* is served. That is how
the coverage gap got missed once already, so acceptance is split in two and the
split is structural rather than a heading:

- **Operation coverage** re-expresses each audited call site against the
  catalogue, one test per call site, named for the consumer and the call. A gap
  shows up as a missing test rather than as a paragraph nobody re-reads.
- **Scenario coverage** confirms the catalogue serves each consumer's actual
  *shape*: a request built on one thread and executed on another under a
  captured context, many requests across concurrent workers from one shared
  capture, and a handle opened by one request carried into a later one -- the
  combination where [D-4](#d-4)'s owned duplicate meets
  [D-10](#d-10)'s shared enumeration cursor, and the one no single-entry test
  reaches.

**An entry may be wider than its call site, and that is the correct direction.**
The watcher passes NULL for two of `GetVolumeInformationByHandleW`'s
out-parameters; this crate's entry fills them anyway. Acceptance records that as
correct rather than as a mismatch, because the failure this milestone guards
against is an entry that is *narrower* than what a consumer needs.

**`windows-thread-ambient-sys` is a dev-dependency, and only that.** A request
carries no ambient context and a context is useful to work that never opens a
file, so the two are paired at a submission site by whoever owns both
([D-1](#d-1)). The scenario tests *are* that submission site -- which is the
only honest way to demonstrate the relationship without creating the coupling
the design refuses. Acceptance also asserts the negative: every entry works with
no captured context at all, because access was checked at the open. A catalogue
that silently required a context would not be a sibling of the ambient crate; it
would be a layer above it.

## <a id="d-17"></a>D-17: A Win32 out-buffer is allocated in content units and sized in terminator-inclusive units, and the two differ by one on purpose

Raised as a review finding on PR #46 and rejected as a defect; recorded here so it
is not re-raised, and so nobody "corrects" it in the unsafe direction.

Two conventions meet at every string-out call in this crate, and they are not the
same:

- **`Wtf16String::with_capacity(n)`** takes *content* units and reserves `n + 1`,
  because the terminator always occupies one slot. Total storage is `n + 1`.
- **Win32's length parameters** (`nBufferLength` for `GetFullPathNameW`,
  `cchFilePath` for `GetFinalPathNameByHandleW`, `nVolumeNameSize` for
  `GetVolumeInformationByHandleW`) count units *including* the terminator. A call
  told `k` writes at most `k` units total, so at most `k - 1` content units.

Every site here passes the same number to both. The consequence is that the
allocation is **one unit larger** than the size Win32 is told:

| site | `with_capacity(n)` | total storage `n+1` | told Win32 `k` | max content Win32 writes (`k-1`) | spare (`n+1-k`) |
|---|---|---|---|---|---|
| [path.rs](src/path.rs) `resolve` | 260 | 261 | 260 | 259 | 1 |
| [volume.rs](src/volume.rs) label | 261 | 262 | 261 | 260 | 1 |
| [volume.rs](src/volume.rs) filesystem name | 261 | 262 | 261 | 260 | 1 |
| [full_path.rs](src/full_path.rs) first attempt | 260 | 261 | 260 | 259 | 1 |

The slack is in the **safe** direction: the buffer is over-sized relative to what
Win32 is permitted to fill, never under-sized. The review that raised this
described it as under-sizing the buffer and as being able to truncate at the
documented limit; both are the wrong way round.

### The documented limit is honoured exactly, not approximately

`path.rs` is the case where a real off-by-one would show, because it enforces a
limit rather than growing. `MAX_PATH` is 260 *counting the terminator* and
`MAX_PATH_CONTENT` is 259. Told `nBufferLength = 260`, `GetFullPathNameW` can
write at most 259 content units -- exactly `MAX_PATH_CONTENT`, which is exactly
what the success path permits before returning `PathTooLong`. So the buffer size,
the limit constant, and the success test agree; there is no length that is
rejected when it should fit, and none accepted that should not.

The growing callers cannot truncate at all: a path that does not fit produces the
"required size including the terminator" form, which is strictly greater than the
size passed, so it is always recognised as a retry rather than mistaken for a
success. `written == capacity` is unreachable -- that would mean `capacity - 1`
content units, which would have fit.

### Why the spare unit is not removed

Tightening each site to `with_capacity(k - 1)` would make the allocation exactly
`k` and is equally correct. It is not done because the gain is one `u16` per call
and the cost is editing four `unsafe` FFI call sites whose current sizing is
proven correct -- a bad trade on a crate whose buffers are handed to the kernel.
If it is ever done, it must be done together with the size passed to Win32, never
to one side alone.

## <a id="d-18"></a>D-18: `GetFullPathNameW` is not lexical, and the genuinely lexical alternative is the wrong call

**Its decision, to keep `GetFullPathNameW` at construction, is superseded by [D-19](#d-19); its measurements stand.** D-18 weighed only the canonicalisers, which leave a relative path relative; the lexical joins `PathAllocCombine` and `PathCchCombineEx` were not weighed, and are measured in D-19.

**The correction.** This crate described `GetFullPathNameW` as **lexical** in
**nine places across five files**, in a sentence that then went on to say it
resolves against the process current directory. Those two claims disagree, and
the call is better described as doing two separable things. It collapses
`.`/`..` and normalizes separators, which *is* lexical -- `C:\a\..\b` becomes
`C:\b` whatever the current directory is, and whether or not `C:\a` exists. It
*also* **roots** most paths that are not fully qualified, and that reads mutable
process state. Three forms use it three different ways -- though not from three
different sources, since the first two both derive from the process current
directory and so does the current-drive case of the third: a relative path
takes the current directory; a root-relative path like `\foo` takes only that
directory's *root*, which is `\\server\share\` when the current directory is a
UNC path and so is not a drive at all; and a drive-relative path such as
`C:foo` takes the entry recorded for that drive -- usually that drive's own
current directory, though the entry is used verbatim and an accepted one may
name a directory on another drive entirely. That rule has two arms:
for a drive other than the current one Windows reads the hidden `=C:` entry
recorded for it, which moves independently of the process current directory;
for the *current* drive the entry makes no difference to the result and the
process current directory wins. Measured -- setting `=Q:` while the process is
on `Q:` changes nothing. ("Makes no difference" rather than "is ignored" for the
same reason the probe no longer says "without consulting a device": an entry
that is read and then discarded is indistinguishable from one never read.) So the call is not lexical *as a whole*, and the claim that
holds unqualified is that it **does not verify what it produces** -- the
documented guarantee, and narrower than the "touches no filesystem" an earlier
draft claimed. A black-box success cannot establish that broader claim, and it
did not need to: the drive-relative form *disproves* it outright, as the
measurement below records.

**"Most" rather than "every", because a legacy device name short-circuits the
rooting.** `CON` resolves to `\\.\CON` and is not rooted, so it is an
unqualified input that is nonetheless invariant -- which is why the rooting
clause cannot be stated unconditionally. The form is looser than exact match:
`CON:`, `CON.`, `con` and `CONIN$` all map, while `CON.txt`, `a\CON` and
`CON:x` root normally. This matters to a crate that prepares paths on a
caller's behalf: `prepare("CON")` returns a device.

**`NUL` is the one member that clause does not describe**, and the clause was
written from `CON` and stated of the whole set. Measured across all eight
accepted names: seven root normally once anything precedes them, and `NUL`
short-circuits as the final component of any path -- `\NUL`, `.\NUL`, `a\NUL`
and `C:\NUL` all reach `\\.\NUL`, where the `CON` spellings root. Only a suffix
(`NUL.txt`, `NUL:x`) takes it out. So a **fully qualified** path can still
resolve to a device, which is the part a caller needs: a rooted result is not by
itself evidence that a name refers to a file on that volume. Pinned by
`nul_is_the_one_device_word_a_path_around_it_does_not_save`.

Keeping the two halves apart matters, because the decision below turns on the
rooting half alone. Saying the call resolves `.`/`..` "against the current
directory" -- as a first draft of this correction did -- attributes process-state
dependence to the one operation that has none, which is the same imprecision
running the other way.

The wrong word had spread well beyond the one file that was reported. The sweep
that found the rest, and its arithmetic, are Tier 2:
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) -> `D-18`.

**The decision: keep `GetFullPathNameW`.** Two canonicalizers that do not root
exist -- `PathCchCanonicalizeEx` and `PathAllocCanonicalize`. They are the wrong
call here for a semantic reason: resolving against the current directory *at
submission* is what this crate is buying. A canonicalizer that does not root
would leave a relative path relative, so its meaning would be settled on the
worker at execution time, against a current directory any thread may have
changed in between -- which is exactly the race preparation exists to close. What
they omit is the part that is wanted.

**No cost comparison is claimed, and that is deliberate.** An earlier draft of
this decision called the alternatives "cheaper". Nothing here benchmarks them,
Microsoft documents behaviour rather than relative cost, and
`PathAllocCanonicalize` allocates its own result -- so the word was a guess
wearing the clothes of a measurement, in a decision whose whole subject is not
doing that. It is also unnecessary: the rooting semantics decide this alone.
Neither is reliably free of process state either, since
`PATHCCH_ALLOW_LONG_PATHS` makes `PathCchCanonicalizeEx` consult the process
long-path setting unless the FORCE variant is used.

Recorded with the alternatives named so the next reader does not re-derive it.
If the reasoning is ever wrong -- a consumer wanting a pure string operation,
having resolved relativity another way -- they are named here.

**It does touch the filesystem, on one form -- measured, after four drafts said
otherwise.** Resolving a drive-relative path for a drive that is *not* the
current one checks that drive's `=X:` entry against the filesystem *and* against
a required shape. An accepted entry is used **verbatim** and need not be on that
drive -- with `=X:` set to `C:\Windows`, `X:foo` is `C:\Windows\foo`. Anything
rejected is replaced by the drive root, and the entry is **written** back there
-- created when absent, so this happens on a pristine host too.

Both halves of the gate were measured, and one of them refuted a draft of this
very decision. Existence matters: a missing directory and an existing *file* are
each rejected. Shape matters too, and independently -- `C:/Windows/System32`,
`C:\Windows\System32\.`, `C:\Windows\System32\..\System32` and
`\\?\C:\Windows\System32` were all rejected while naming the same existing
directory that `C:\Windows\System32` was accepted for. So the draft calling this
"a filesystem query rather than a syntax test" named a mechanism the evidence
contradicts: it is both, and the list is observation rather than specification. The rewrite mutates the process environment
block as a side effect of what reads like a pure query. For the current drive
the entry makes no difference to the result and is not rewritten -- stated as
those two measured effects rather than as "not consulted", because installing an
entry and observing the outcome cannot separate "not read" from "read and
ignored", and a draft of this very paragraph said "not consulted" anyway.

Earlier drafts concluded the opposite by reasoning that the current directory
lives in the PEB and the `=X:` variables in the environment block, so both are
ordinary process memory. The reasoning was sound and the conclusion wrong. That
is the failure this decision exists to name: a mechanism argued from where the
data lives rather than measured. It also means the guarantee this crate relies
on has to be the narrow one -- the call does not *verify* what it produces --
because the broad one is not merely unproven but false.

**The constraint this decision carries, and not just its conclusion:** state
only what the evidence reaches. Nine drafts of this entry each named a
mechanism it did not -- the call's nature, what a number measured, what the
alternatives cost, whether any filesystem was touched. The wordings differ; the
error does not. A reader taking only "it is not lexical" away from D-18 has the
answer without the thing that kept producing wrong ones.

The constraint reaches further than prose, and the sharpest case was not about
this call at all: a test helper collapsed an *empty* `=X:` entry into an absent
one on a comment that called the equivalence measured, when the measurement had
never cleared the last error that distinguishes them. So **"measured" is itself
a claim, and a procedure can be wrong in ways its result never shows.** The
practical consequence was that restoring a borrowed empty entry deleted it. See
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) -> "The measurement that was itself
unmeasured".

The drafts themselves, and why each failed, are Tier 2:
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) -> `D-18`.

## Open, and inherited rather than introduced

- **Path resolution under a captured identity.** A path must be resolved on the
  calling thread, because the process current directory is mutable by any
  thread -- but `GetFullPathNameW` never expands a drive letter,
  and drive-letter resolution follows the *impersonated* token's logon session.
  So a root resolved on a submitter and opened on a worker under a captured
  token can name a different device. The workspace has this as an open decision;
  until it is settled, this crate documents the hazard rather than implying its
  resolution is complete.
- **Reusing the shipped path preparation.** The enumeration crate's
  [path.rs](../windows-file-enumeration-sys/src/path.rs) already does
  submission-time resolution with a typed error, but `prepare` is `pub(crate)`.
  Binding to it rather than writing a second one therefore needs it published or
  extracted; duplicating it is the option this repository's mono-repo policy
  rejects.

## <a id="d-19"></a>D-19: A relative path is grafted onto a root captured at construction, by concatenation, and only when it is clearly safe

**Status: decided by the engineer on 2026-10-15; not yet implemented** -- the work is `NR-3.3` in
[CHECKLIST.md](CHECKLIST.md). It supersedes the mechanism of [D-9](#d-9) and the decision of
[D-18](#d-18); D-18's measurements stand.

**What is decided.** A path reaches Win32 as the caller gave it: a `Wtf16String`, with no length limit
and no check of this crate's own beyond the one that cannot be avoided, an interior NUL, which a
`PCWSTR` cannot carry. `OpenFile::new` takes the `Wtf16String` directly. What is kept from D-9 is the
need to fix the meaning of a relative path when the request is built, because the process current
directory is shared and mutable and a request is performed later, on any thread. That is done by
capturing the current directory when the request is built (`GetCurrentDirectoryW`, which does no
I/O) and resolving a relative path against it **textually** when the request is performed, even after
the current directory has changed.

**The graft is concatenation:** the captured directory, a backslash unless it already ends in one (a
drive root does), then the child. There is no combine call, no flag, and no canonicalisation:
`CreateFileW` normalises `.`, `..` and trailing dots and spaces on the final path itself, and Win32
decides whether a long result is acceptable to the host. Nothing here prefixes `\\?\`, ever.

**Only a clearly safe child is grafted.** All of these must hold:

1. it is not empty;
2. it does not begin with `\` or `/` (that rules out root-relative, UNC, `\\?\` and `\\.\`);
3. it contains no `:` (that rules out drive-relative `C:a` and `C:`, and a stream `name:stream`);
4. if the captured directory is a UNC path, the child has no `..` component;
5. no component of the child is a reserved device name -- `CON`, `PRN`, `AUX`, `NUL`, `COM` and `LPT`
   with a digit, `CONIN$`, `CONOUT$` -- compared without regard to case, after removing everything from
   the first `.` and any trailing spaces. The list is the one in Microsoft's "Naming Files, Paths, and
   Namespaces", and the rule errs toward not grafting.

A child that is not grafted is passed through untouched, and Win32 resolves it when the call is
made, against whatever the current directory is then. For those forms the race D-9 closed stays
open, and is documented as open. A root-relative child (`\a\b`) is among them: grafting it needs the
root of the captured directory, a parse this version does not do, and no work is scheduled for it.

**Why not the alternatives**, measured on 2026-10-15 on Windows 11 with the process current directory
set to `C:\Windows\System32` (the full table is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) ->
`D-19`):

- **`GetFullPathNameW` at construction (D-9, D-18).** It is exact for every form, but for a
  drive-relative path on another drive it reads, validates against the filesystem, and may write the
  hidden `=X:` entry (D-18), on the submitting thread; and its output buffer is what imposed the
  `MAX_PATH` limit.
- **`PathAllocCombine` / `PathCchCombineEx` (a lexical join).** Exact for relative and root-relative
  paths, and also not safe to hand every path: it returns a drive-relative path unresolved and turns
  `C:` into `C:\`; it returns an empty path as the base; it drops the `\\?\` prefix and collapses `.`
  and `..` inside a verbatim path; and it drops the `\\.\` prefix, so `\\.\PhysicalDrive0` becomes
  `\\PhysicalDrive0`. Its long-path flags are Win32's, not this crate's, and
  `PATHCCH_ALLOW_LONG_PATHS` alone **rewrites a result longer than `MAX_PATH` into a `\\?\` path**,
  which is the prefixing this decision refuses. So the call needs a classification of the path first
  anyway, and once the child is known to be a plain relative one, concatenation does the same job
  with none of that.

**Device names are why rule 5 exists.** `NUL` is a device whatever precedes it, so a graft changes
nothing for it. `CON`, `con`, `CONIN$` and `CONOUT$` open as devices when bare and as a lookup of a
file of that name when grafted, so a graft would change what the call means.

**Not verified**, and stated as such:

- Concatenation is not measured against Win32's own resolution of the same relative path; it follows
  from the join's measured equality with `GetFullPathNameW` for the relative forms.
- `GetCurrentDirectoryW` with a current directory longer than `MAX_PATH` was not measured.
- `PRN`, `AUX`, `COM1`, `COM2` and `LPT1` open as nothing in any spelling on the machine measured,
  so whether they behave as `CON` does when a device is present is unknown; they are on the list
  because the documentation names them.
- Behaviour in a process that is long-path aware was not measured; the test binary is not.

**Scheduled work:** `NR-3.3`. `DI-13.2`'s rework in `win-durable-ioring` waits on it.

## <a id="d-20"></a>D-20: Every device control goes through one overlapped-safe helper

**Status: decided and implemented** by `NR-2.1` (the helper is `src/device_control.rs`, its first
entry is [`QueryVolumeNumaInfo`](src/volume_numa.rs)).

**What is decided.** A catalogue entry that is a `DeviceIoControl` call does not call it. It calls the
crate's one private helper, which makes the call the same way on every handle:

- It gives the call an `OVERLAPPED` and a manual-reset event of its own, and waits for completion
  before returning, so the output buffer is not in use by the device when the caller gets it back.
  The crate cannot see whether a handle was opened `FILE_FLAG_OVERLAPPED` (a request owns a
  duplicate of a handle its caller opened, [D-4](#d-4)), and `DeviceIoControl` documents that on an
  overlapped handle a call without a valid `OVERLAPPED` "fails in unpredictable ways".
- It passes the event with its **low-order bit set**, which `GetQueuedCompletionStatus` documents as
  suppressing the completion packet, so a handle that belongs to a consumer's completion port gets
  no packet for a call the consumer never made. The event is still signalled.
- If the wait itself fails it **aborts the process**: the device may still write into memory that
  unwinding would release, and stopping is defined where continuing is not.
- It returns the raw Win32 code unaltered, including `ERROR_MORE_DATA` and
  `ERROR_INSUFFICIENT_BUFFER`, which a control that reports a size first is sized by
  ([D-17](#d-17)).

**What an entry owns** is the interpretation of the bytes and nothing else. It does not invent a value
for a device that has no answer: a failure is the device's own code and a success carries what the
device wrote ([D-12](#d-12)). It also checks that the number of bytes written is the size its structure
is, and reports a mismatch as its own error, because a value read from a buffer the device did not
fill would read as an answer.

**Measured on 2026-10-15**, Windows 11, one NUMA node:

- `FSCTL_QUERY_VOLUME_NUMA_INFO` on a file on the system volume answers node `0`; on the `NUL`
  device it fails with `ERROR_INVALID_FUNCTION`. What a volume that advertises no proximity domain
  answers is not documented and was not observed.
- `FSCTL_PIPE_LISTEN` on an overlapped named-pipe server fails with `ERROR_INVALID_FUNCTION`
  through `DeviceIoControl`. Why is not established, and it is evidence about that one code only: the
  code of `NR-2.2` worked through the helper (below), and the `IOCTL_STORAGE_*` codes of `NR-2.3` and
  `NR-2.4` are ordinary device controls for which `DeviceIoControl` is the documented route, which
  nothing here has yet checked.
- An overlapped call to a file-system control pended in 0 of 20000 isolated runs of two controls,
  and pended on some runs while the full test suite ran in parallel. Of the controls tried,
  `FSCTL_REQUEST_OPLOCK` is the only one that pends on demand (below); with a write-open alone the
  oplock was not broken, and a write through the second handle broke it.

**The pending path is pinned through a seam.** The helper is two functions: `device_control`, which
is the `DeviceIoControl` argument list, and `overlapped_call`, which owns the event, the
`OVERLAPPED`, the wait and the result read, and takes the call as a closure. The tests hand
`overlapped_call` an overlapped `ReadFile` on a named pipe, which pends until the other end writes
and whose completion, byte count and failure are documented behaviour. Four sabotages pin it (not
waited for, waits for nothing, byte count lost, failure after pending reported as success), and
[sabotage.json](sabotage.json) names what stays unpinned: the abort when the wait fails, which the
helper's own event cannot provoke.

**The helper takes an input buffer** (`NR-2.2`). An empty input reaches the device as a null pointer
and a length of zero, so a control that takes none is unchanged. `FSCTL_REQUEST_OPLOCK` takes input and
is the one control found that pends on demand: a read-oplock request stays pending until another
handle writes to the file, and then completes with the oplock's original and new levels. It is the
test that reaches `DeviceIoControl` itself through the pending path, and it shows the input arriving,
because the same request with no input is refused by the device. A sabotage that drops the input
pointer is caught by it, and by nothing that takes no input.

**A duplicate that must be decided.** `win-volume-sys` has its own `control()`
([control.rs](../win-volume-sys/src/control.rs)) with the same overlapped-and-event shape and an
`ERROR_MORE_DATA` convention of its own. Building this one beside it, and not changing the working
crate while this one is proven, is deliberate; the merge-or-delete decision is scheduled as `NR-2.6`,
so the duplicate does not become permanent because nobody came back.

**Scheduled work:** the decision about `win-volume-sys`, `NR-2.6`.

## <a id="d-21"></a>D-21: The disk-extents entry reads the reply and refuses one that is not what it counts

**Status: decided and implemented** by `NR-2.2` (`src/volume_extents.rs`).

**What is decided.** `QueryVolumeDiskExtents` sends `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` through the
helper ([D-20](#d-20)) and returns the runs as a `Vec<DiskExtent>`, in the order the device gave them.

- **It asks again at the count the device names.** The reply is a count then that many extents. A
  buffer with room for the count but not the extents fails with `ERROR_MORE_DATA` and the count is
  written, so the entry reads the count and allocates for it. The retry is bounded and ends in
  `DiskExtentsError::Unstable`, for the reason [`QueryFinalPath`](src/final_path.rs)'s is: an unbounded
  loop on an answer that keeps changing would spin a worker a consumer moved the call onto.
- **It never asks with room for fewer than one extent.** A buffer smaller than the count plus one
  extent is refused by the volume device with `ERROR_INVALID_PARAMETER`, which is not what
  `ERROR_INSUFFICIENT_BUFFER` would lead a caller to expect (measured, below).
- **It reads fields, as `u64`s, and refuses a reply that does not hold what it says.** The disk number,
  the starting offset and the length are read from where the layout puts them. A count the written
  bytes cannot hold, a call that claims more bytes than the buffer has, and a negative offset or length
  (the structure's `LARGE_INTEGER`s) are `DiskExtentsError::Malformed`, not read: extents taken from a
  buffer that does not hold them would read as disks the volume does not have. This is the same
  position as [`QueryVolumeNumaInfo`](src/volume_numa.rs) takes on a reply of the wrong size
  ([D-12](#d-12)), and it is validation of the reply, not interpretation of the device's answer.
- **Any other failure is the device's own code, unaltered, and is not retried.**
- **A count whose buffer cannot be allocated is `ERROR_NOT_ENOUGH_MEMORY`**, this crate's own report
  ([D-8](#d-8)), because the count is the device's and is not trusted to be allocatable.

**Measured on 2026-10-15**, Windows 11, one disk:

- The system volume opened with no access reports one run. A volume handle needs no access, which an
  ordinary account can open.
- The volume device answers a buffer of any size below the count plus one extent with
  `ERROR_INVALID_PARAMETER` (every size from 0 up to one byte short of it was tried) and answers from
  that size up. An ordinary file and the `NUL` device fail with the device's own code.

**What was not observed.** A volume that spans or stripes disks: the `ERROR_MORE_DATA` half, the
count written with it, and the order of several runs are from the documentation and from
[`win-volume-sys`](../win-volume-sys/src/extents.rs)'s reading of it, and are tested here only against
a fake device built to answer that way. The fake's one measured behaviour is the
`ERROR_INVALID_PARAMETER` above.

**A duplicate that must be decided.** `win-volume-sys` has `volume_disk_extents`, the same call with the
same retry, over its own `control()`. It is covered by `NR-2.6` with the helper's.

**Scheduled work:** none beyond `NR-2.6`.

## <a id="d-22"></a>D-22: Storage properties are one call with a typed entry each, and a reply is reported as bytes the device gave

**Status: decided and implemented** by `NR-2.3` (`src/storage_property.rs`, the shared mechanics, and
`src/storage_unique_id.rs`, the first entry). `NR-2.4` is the second.

**What is decided.**

- **One Win32 call, an entry per property.** `IOCTL_STORAGE_QUERY_PROPERTY` returns a different structure
  for each property id, so each property is its own entry with its own typed reply, which is how
  [D-3](#d-3) reads "one entry per Win32 call" for a call whose meaning is chosen by an argument. What the
  entries share is private: the query structure and learning the size of the reply. The one public item
  there is [`StoragePropertyError`](src/storage_property.rs), shared because the failures are the same.
- **The reply starts with its own size.** Every reply begins with a `STORAGE_DESCRIPTOR_HEADER` (a version
  and the size of the whole reply). The entry asks once with a buffer it expects to suffice
  (512 bytes, chosen because the one reply measured was 112) and again at the size the header names, in a
  bounded loop that ends in `Unstable`. The reply returned is exactly the bytes the header says it has,
  not the padding of a larger buffer.
- **A reply that is not what it says is `Malformed`, not read**, as in [D-21](#d-21): a size smaller than
  the header, a call that claims more bytes than the buffer has or fewer than a header, a reply shorter
  than the size it names in a buffer that had room for it, and a request for more room that names a size
  already given.
- **The unique identifier's sections are reported as bytes, not parsed.** The reply is a five-`u32` header
  (`storduid.h`, which `windows-sys` does not carry, so it is laid out here from the documentation) and up
  to three sections its offsets point to: the device ID descriptor, the device descriptor and the drive
  layout signature. Each begins with its own version and size, which is how a section is delimited, and
  each is checked to lie within the reply. An offset of zero means the section is absent.
- **The identifier is not interpreted.** What an identifier inside a descriptor is, and which of them a
  key for a disk is made from, is the consumer's, as the reply's own documentation is.

**Ruled by the engineer on 2026-10-15, and open to revisiting:** typed accessors for the identifiers *inside*
the descriptors (`STORAGE_IDENTIFIER` entries) are not provided -- "at this time, no. We can revisit later if
necessary." Whether they belong here or in the locality helper that derives the key (root `M39`) is for
whoever needs them to raise. No work is scheduled for it, deliberately.

**A generic entry is provided** (the engineer's ruling, same date): `QueryStorageProperty`
([`src/storage_query.rs`](src/storage_query.rs)) is the same call with the property and the query type
chosen by the caller and the reply returned as the bytes the device gave, so a consumer can reach a property
this crate has no typed entry for.

- The property and the query type are newtypes over the raw values, so one cannot be passed for the other.
  Neither is validated: a value the device does not know is the device's own failure, unaltered.
- Additional parameters follow the query structure's fixed fields, are passed as given, and are held shared
  (`Arc<[u8]>`), so copying a request never allocates ([D-8](#d-8)'s rule that an allocation that fails is an
  error the caller receives).
- A standard query's size is learned by the same loop. A query for whether a property exists returns no
  structure, so it is one call with no output and an empty reply, and success means the property exists.
- What is *not* established is what a query type other than standard and exists returns, and so what the
  entry makes of it: a reply that does not begin with the descriptor header is `Malformed`. The mask query
  was refused by both properties tried.

**Measured on 2026-10-15**, Windows 11, one disk, a handle opened with no access:

- The control works through the helper ([D-20](#d-20)), on a plain and an overlapped handle alike, and an
  ordinary file and the `NUL` device fail with the device's own code.
- A buffer of exactly the header's size is answered with the header alone; one between that and the reply is
  `ERROR_MORE_DATA` with the header written; one smaller than the header, and an input smaller than the
  query structure, are `ERROR_BAD_LENGTH`. The `ERROR_MORE_DATA` half is therefore reachable on a machine of
  one disk, unlike [D-21](#d-21)'s, and the loop is tested against the real disk for every first-buffer
  size.
- The disk reports its device ID descriptor and neither of the other two sections (their offsets are zero).
  The descriptor's own size is exactly what remains of the reply.

- An exists query on a real property succeeds with nothing written whatever the output size, and an unknown
  property is `ERROR_INVALID_FUNCTION` for both the exists and the standard question. A mask query, and a
  query type Windows does not define, were `ERROR_NOT_SUPPORTED` for the two properties tried.
- Additional parameters are read by some properties and not others. The device property gave the same reply
  for any bytes. The unique-id property read the first: 0 gave the usual reply, 1 a larger one, and larger
  values `ERROR_INVALID_PARAMETER`. What that byte selects is not documented and is not interpreted; it is
  also how the generic entry's additional parameters are shown to arrive.

**What was not observed.** A disk that reports a device descriptor or a drive layout signature, one on
another bus (NVMe, USB, a virtual disk), and a reply larger than the first buffer. Those shapes are tested
against constructed replies of the documented layout and a fake device that answers as the real one was
measured to. That an offset of zero means "absent" is a reading of the measurement and of the fact that
zero is where the header is; the documentation does not say.

**Documented, with a caveat.** The control code and the property id are documented, and the documentation
marks the property as intended for driver use. The reply structure is documented in the driver kit, not the
Win32 SDK headers; it is the documented route to the structure, which is what this crate's rule asks.

**Scheduled work:** `NR-2.4` reuses `storage_property`; the decision about `win-volume-sys` is `NR-2.6`.

## <a id="d-23"></a>D-23: A disk's NUMA node is its own entry, carried unaltered, and the size protocol is a property's

**Status: decided and implemented** by `NR-2.4` (`src/storage_numa.rs`). It builds on [D-22](#d-22).

**What is decided.**

- **The node is asked per disk.** `QueryStorageDeviceNumaNode` takes a disk handle. A volume that spans disks has
  one answer from [`QueryVolumeNumaInfo`](src/volume_numa.rs) and a node per disk from here, and one node
  reported for a volume whose disks sit on different nodes would be a fiction; the disks of a volume come from
  [`QueryVolumeDiskExtents`](src/volume_extents.rs) ([D-21](#d-21)). The consumer composes them; this crate
  composes nothing.
- **The node is carried unaltered, and no sentinel is invented.** The reply is the three `u32`s of
  `STORAGE_DEVICE_NUMA_PROPERTY` (a version, a size and the node). The checklist item spoke of "the documented
  unknown value"; none was found documented, so it is not claimed, and whether a given value means no node is
  the caller's to decide, as in [D-12](#d-12) and `QueryVolumeNumaInfo`.
- **The size protocol is a property's, not the call's.** What a buffer smaller than the header, or one between
  the header and the reply, gets differs by property (the table is in `storage_property`'s module
  documentation). The loop reads the size from the header of whichever comes back and never asks with less
  than the header, so it needs to know neither, and a test pins the difference so it is not forgotten.

**Not documented, and stated where it is claimed.** The property is documented ("Provides the non-uniform
memory access (NUMA) node of the storage device"), but its description names no reply structure and the
structure's own reference page was not found (a 404 at the address the other storage structures use). The
structure is declared in the public SDK header `winioctl.h`, which `windows-sys` carries, and that is what is
read. This is a weaker footing than [D-22](#d-22)'s unique identifier, whose structure is documented in the
driver kit, and the repository's rule on documented interfaces
([copilot-instructions.md](../../.github/copilot-instructions.md)) was written about `Nt*` and `Rtl*`
exports, not this case. It is recorded so the engineer can overrule it: the entry is one small module, and the
generic entry ([D-22](#d-22)) reaches the property regardless.

**Measured on 2026-10-15**, Windows 11, one disk, a handle opened with no access:

- The reply is twelve bytes and the node is zero, on a machine with one NUMA node.
- Below the header the device answers `ERROR_INSUFFICIENT_BUFFER` (the unique-id property answers
  `ERROR_BAD_LENGTH`). From the header up to one short of the reply it answers with the header alone as a
  *success* (the unique-id property answers `ERROR_MORE_DATA`). The loop is tested against the real disk for
  every first-buffer size.
- The exists query succeeds and a plain and an overlapped handle give the same answer.

**What was not observed.** A node other than zero, a disk on a multi-node machine, and what a disk with no node
reports. Those need hardware this machine is not.

**Scheduled work:** none.
