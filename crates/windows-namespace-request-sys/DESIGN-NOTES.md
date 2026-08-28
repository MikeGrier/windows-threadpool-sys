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
prevents `IoRing` use of it, measured, and the choice must be made before
association by a layer that knows the destination. An entry that associated on
the caller's behalf would foreclose a fork it has no standing to decide.

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
underlying reason -- so it is met once, by an `AlignedBuffer` primitive whose
alignment is stated at construction and preserved by `Clone`, rather than being
solved twice by two different local tricks.

## <a id="d-9"></a>D-9: Path preparation is duplicated from the enumeration crate, deliberately and temporarily

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
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md), gated on this
crate's first release. Until then, a fix to either copy must be applied to both.

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

## Open, and inherited rather than introduced

- **Path resolution under a captured identity.** A path must be resolved on the
  calling thread, because the process current directory is mutable by any
  thread -- but `GetFullPathNameW` is lexical and never expands a drive letter,
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
