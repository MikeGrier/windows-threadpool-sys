# Design session -- remoting synchronous Win32 namespace operations (2026-08-27)

> Tier-3 record. [DESIGN-NOTES.md](../DESIGN-NOTES.md) is authoritative and wins
> on any conflict. This file records how the decisions were reached, what was
> measured, what was rejected, and what is still open.

This session designed a facility that makes synchronous-only Win32 operations --
open, delete, rename, attribute queries -- available asynchronously to a calling
thread, by remoting them to thread-pool workers with the caller's context
captured explicitly. It produced nine measurements of undocumented or
folklore-level platform behaviour, several of which overturned assumptions the
design had already been built on.

Nothing here is implemented yet. The work items this session implies are in
[CHECKLIST.md](../CHECKLIST.md).

**Landed during this session, after it began:** PR #44 merged
[windows-impersonation-token-sys](../crates/windows-impersonation-token-sys/DESIGN-NOTES.md) and
[windows-file-enumeration-sys](../crates/windows-file-enumeration-sys/DESIGN-NOTES.md).
The first owns the impersonation component of the captured context, so the
facility consumes it rather than reimplementing capture. The second is the first
shipped inhabitant of the namespace plane and independently reached much of the
same shape -- bounded submission and completion rings, a reporting worker,
finite quanta, a captured-token directory open.

**Replacing that directory open was the intent all along.** This was recorded
first as an open merge-or-delete question and corrected by the engineer: the
direction is committed, and only the timing is conditional in the ordinary
duplicate-then-decide way. Having a committed consumer before the facility exists
already paid for itself by correcting the design -- it established that an
**unassociated** handle is a first-class destination, where this session had
described the open as forking two ways (completion port or ring) when it forks
three. `GetFileInformationByHandleEx` is synchronous and has no overlapped form,
so the consumer needs a plain handle, and a two-destination design could not have
served its own first consumer. The replacement is M21.3 in
[CHECKLIST.md](../CHECKLIST.md).

---

## Starting intent

The thesis as originally stated: "Win32 does not provide asynchronous
operations, so we make up for it by marshaling the context for select
synchronous-only operations and exposing them as asynchronous operations on the
calling thread, remoting the work over to workers."

Four concerns were raised at the outset:

1. The context includes thread-local state -- impersonation, and possibly more.
   Marshaling is non-trivial in general: the current directory is process-wide,
   so either the path is canonicalized in place or every drive's working
   directory has to be captured and remoted.
2. How many rings, with how many reserved buffer pools? Should the ring for
   Win32 APIs be the same as the `IoRing`?
3. How many workers may process offloaded synchronous operations, and can that
   be bounded?
4. Should there be two layers -- a trivial one over the thread pool, with the
   ring layered on top?

## Reframing: the namespace plane, not "Win32"

"Win32 is not asynchronous" is broader than what is meant, and the broad form
makes scoping impossible. Win32 *is* asynchronous on the **data plane**:
overlapped `ReadFile`/`WriteFile`/`DeviceIoControl`, and `IoRing`. Both are
already covered by crates in this workspace.

What is synchronous-only is the **namespace and metadata plane**: open, close,
delete, rename, attributes, times, security, links, volume and path queries,
directory enumeration. That is not an accident of API vintage; it follows from
the object manager's design.

Adopting that division gives a principled inclusion test rather than a
taste-based one, and it places the new facility precisely against its siblings:
`windows-overlapped-io-sys` and `windows-ioring-sys` own the data plane, this
facility owns the namespace plane, and `windows-file-enumeration-sys` is a
specialization of the namespace plane that needed streaming delivery.

One consequence worth stating because it is easy to miss: **`CloseHandle`
belongs in the catalogue.** It blocks on outstanding I/O and can block hard on a
dead network path.

## Measurements

### Platform and method

All measurements were taken on:

- Windows 11 Enterprise, build 10.0.28000
- `aarch64-pc-windows-msvc`, Snapdragon X2 Elite (X2E80100), 12 logical
  processors
- rustc 1.98.0
- `IoRing`: `max_version=400`, `max_sq=65536`, `max_cq=131072`,
  `features=0x2` (`IORING_FEATURE_SET_COMPLETION_EVENT`)

The thread-limit and timer measurements already recorded elsewhere in this
repository may have been taken on x64; where a number below looks like a
platform constant rather than a semantic fact, it is flagged as needing an x64
re-check.

Every probe was written to measure the platform through `windows-sys` directly,
except the thread-pool growth probe, which was built on the shipping
`windows-threadpool-sys` API so that it measured the real thing rather than a
reimplementation of the SDK's inline environment helpers.

### M-1: `BuildIoRingRegisterFileHandles` replaces the table

Three files were filled with distinguishable bytes (`0xAA`, `0xBB`, `0xCC`),
registered in two batches, and read back by index:

| Step | Result |
|---|---|
| register `[A]`, read index 0 | `S_OK`, first byte `0xAA` |
| register `[B,C]` (second call on the same ring) | build `S_OK`, completion `S_OK` -- **not refused** |
| read index 0 | `0xBB` (file B) |
| read index 1 | `0xCC` (file C) |
| read index 2 | `ERROR_INVALID_INDEX` |

The table after the second call is exactly `[B, C]`: a new array, indices from
zero, previous entries gone. Confirmed both by content and by the out-of-range
boundary.

Two follow-ups decided whether replacement is *usable*:

- **Capacity is not scarce.** 256, 4096, and 65536 handles all registered
  successfully.
- **Replacement does not disturb in-flight operations.** With a 512 MiB read
  outstanding against old index 0, the table was replaced with a different,
  4 KiB file. The replace completed first; the big read still returned `S_OK`
  with all 536,870,912 bytes. Had index resolution been late, it would have read
  4096 bytes from the other file. **The index is resolved at submission time.**

### M-2: `IoRing` I/O is thread-agnostic

A 512 MiB read was submitted from a thread that then exited. Across five
trials, the probe first verified the operation was still outstanding at thread
exit (`PopIoRingCompletion` returned `S_FALSE`), then observed completion
`S_OK` with the full byte count, 92--102 ms after the issuing thread died.

### M-3 (control): non-port-associated overlapped I/O *is* thread-bound

M-2 means nothing without evidence that this harness could detect a thread-bound
cancellation at all. An overlapped `ReadFile` was issued on a **named pipe with
no writer** -- a read that can never complete on its own -- from a thread that
then exited. `GetOverlappedResult` returned `ok=0`,
`last_error=995` (`ERROR_OPERATION_ABORTED`).

So thread-bound cancellation is live on current Windows, this harness detects
it, and `IoRing` is immune to it. Thread-agnostic I/O comes from **completion
port association**, not from `FILE_FLAG_OVERLAPPED` alone.

### M-4: completion-port association and `IoRing` are mutually exclusive

A PASS is `S_OK` with 4096 bytes and the expected fill byte.

| Case | Result |
|---|---|
| 1 -- CONTROL: no association | **PASS** |
| 2 -- associate with IOCP, then `IoRing` read | **FAIL** `ERROR_INVALID_PARAMETER`, 0 bytes |
| 3a -- same handle, `IoRing` read *before* association | **PASS** |
| 3b -- same handle, `IoRing` read *after* association | **FAIL** |
| 4 -- CONTROL: overlapped `ReadFile` via the port | **PASS**, 4096 bytes, expected key |
| 5a -- `IoRing` read *before* `CreateThreadpoolIo` | **PASS** |
| 5b -- `IoRing` read *after* `CreateThreadpoolIo` | **FAIL** |

Association permanently poisons a handle for `IoRing`, and `CreateThreadpoolIo`
does the same. Case 4 shows the handle is still perfectly healthy through the
port, so this is a fork, not a broken handle.

### M-5: a pool worker inherits no impersonation token, and runs with critical errors enabled

The submitting thread genuinely held a thread token (`ImpersonateSelf`
succeeded, `OpenThreadToken` returned 1). In the callback:

- `OpenThreadToken` returned **0**, `last_error=1008` (`ERROR_NO_TOKEN`).
- Thread error mode was **`0x0000`** -- `SEM_FAILCRITICALERRORS` **clear**.

So context capture is necessary rather than merely prudent, and the
critical-error handler is enabled on shared pool threads, meaning a hard error
(the classic "no disk in drive" case) can raise a modal dialog on process-shared
infrastructure.

### M-6: `CancelSynchronousIo` blocks until the target leaves synchronous I/O

This began as a question about whether a cancel could be applied to a *later*
operation than the one it targeted. It is not:

- Fired against a thread with nothing outstanding, it returned `0` /
  `ERROR_NOT_FOUND` immediately, and an operation that thread started 100 ms
  later ran untouched for 3 s. **The cancel is point-in-time; it does not
  linger.**

The real hazard is the opposite one:

| Case | Target behaviour | `CancelSynchronousIo` |
|---|---|---|
| 1 | one operation, then leaves I/O | returns in **342 us** |
| 2 | re-enters, same handle, tight loop | **never returns** |
| 3 | re-enters, same handle, 20 ms apart | **never returns** |
| 4 | re-enters, **different** handle | **never returns** |
| 5 | re-enters an operation completing after 3 s | returns after **3.0003802 s** |

Diagnosed with a noninvasive `cdb -pvr -pn` attach. Four identical samples over
twelve seconds at **zero CPU**:

```
main    ntdll!NtCancelSynchronousIoFile+0x4    <- the CANCELLER, blocked
        KERNELBASE!CancelSynchronousIo+0x20
worker  ntdll!NtReadFile+0x4                   <- the target, in its NEXT read
```

Cases 2--4 also show that the cancel *took effect* -- the target recorded its
abort -- and the call still never returned. **Effect and return are decoupled.**
Case 4 rules out the narrow escape: it is not about the same file object, so no
choice of handles avoids it.

Case 5 was written as a falsifiable prediction of the model and landed within
400 us of it:

> **`CancelSynchronousIo` blocks until the target thread is no longer performing
> synchronous I/O.**

### M-7: `SetThreadpoolCallbackRunsLong` is the entire growth mechanism

Reaching 16 concurrent blocked callbacks with `max = 16`:

| | without runs-long | with runs-long |
|---|---|---|
| all 16 running | **1.94 s** | **1 ms** |
| median inter-arrival | **166 ms** | 31--80 us |

Per-arrival shape, which shows what is actually happening:

```
without:  #1-#4 = 0ms   then #5=166  #6=324  #7=482 ... #16=1940ms
with:     #1-#16 all within 1ms
```

**Four threads are free, and growth beyond that is throttled to roughly one
thread per 166 ms.** Stable to within 1 ms of that figure across five
independent runs (medians 165.5, 165.6, 165.8, 166.1, 166.1 ms). Four is very
likely the pool's initial concurrency on this 12-processor machine, so both the
free count and the interval need an x64 re-check before being written down as
platform constants.

### M-9: path resolution follows the impersonation token's logon session

Drive letters are symbolic links in the object manager namespace. Real local
volumes live in the machine-wide `\GLOBAL??` directory; `subst` drives and
mapped network drives live in a **per-logon-session** directory keyed by the
token's authentication id (LUID).

This was first recorded as needing a second logon session and credentials. It
does not. `LogonUserW` with `LOGON32_LOGON_NEW_CREDENTIALS` mints a **new logon
session while keeping the caller's local identity and access**, and does not
validate the credentials it is given, because they are only ever used for
outbound network authentication. That is exactly the token this question needs:
a different LUID with unchanged local rights.

A `subst` drive was created in the process's own session, then resolved with
`QueryDosDeviceW` -- which asks the object manager directly what a letter means
to the caller, with no file ACL to confound the answer -- under four contexts:

| Context | LUID | `C:` (global) | subst letter (session) |
|---|---|---|---|
| no impersonation | process token's | resolves | **resolves** |
| `ImpersonateSelf` (same session) | `...00040D06` | resolves | **resolves** |
| anonymous (other session) | `...000003E6` | **`ERROR_ACCESS_DENIED`** | not found |
| **new credentials** (other session) | `...343FA9CC` | **resolves** | **NOT FOUND** |

The anonymous row is why the `C:` control exists: anonymous cannot read the
object directory at all, so its missing subst letter is unattributable. The
new-credentials row has a passing control and is therefore decisive.

**Path resolution consults the impersonated token's logon session.** A worker
impersonating a captured token can resolve the same string to a different
device, or to nothing at all. The same result was reproduced on a thread-pool
worker, which is the shape the design actually uses.

Consequence: lexical canonicalisation at submission does **not** close the hole,
because `GetFullPathNameW` resolves relative components and `.`/`..` but never
expands a drive letter. Session-relative drive letters must be expanded to a
session-independent form at submission, or rejected at admission. Note also that
`\?\` does not help -- `\?\Z:\dir` still resolves `Z:` through the device map;
only UNC, `\?\Volume{GUID}\`, and `\?\GLOBALROOT\Device\...` bypass it.

### M-8: raising the maximum while saturated works, and the default maximum is 512

- **Raise while saturated.** With every thread parked and work queued behind
  them, `set_max_threads(4 -> 12)` released the queue in **1.1--1.6 ms** across
  five trials.
- **Default maximum.** With no maximum ever set, concurrent blocked callbacks
  plateaued at exactly **512**, with 512 distinct thread ids, from 600
  submissions. The repository's notes had deliberately declined to guess this
  number.
- **The ceiling holds.** `max = 16` with 32 submissions ran exactly 16
  concurrently with 16 distinct thread ids and no overshoot.

## Decisions converged

Recorded authoritatively in
[DESIGN-NOTES.md](../DESIGN-NOTES.md#remoting-synchronous-namespace-operations).
In brief:

1. **Scope is the namespace/metadata plane.** The data plane belongs to the
   existing crates.
2. **No threads are owned.** Pooled execution on a private pool, never a thread
   minimum, `runs_long` mandatory, an elastic maximum, and a hard quarantine
   ceiling above which admission fails with a typed error.
3. **A quarantined worker has affinity to the operation, not the client**, which
   forces `Arc`-shared domain internals so a wedged worker can outlive its
   owner.
4. **The context is a named, exhaustively enumerated composite that *contains*
   an `ImpersonationToken`** rather than an impersonation token grown to carry
   everything else. The aspects have different application windows (impersonation
   is applied only around the open and reverted immediately; the error mode must
   hold for the whole callback), different failure semantics (impersonation
   restore failure is fail-fast, an error-mode restore failure is not), and
   different capturability. Application composes per-aspect guards outermost-first
   and releases in exact reverse. Capture fails synchronously at admission; apply
   and restore are fail-fast on every path including unwind.
5. **The aspects relate to the caller in three different ways**, so "captured
   context" names only part of it: impersonation and WOW64 redirection are
   *transplanted* from the submitter; the thread error mode is *overridden* with
   the facility's own policy, because a hard error on a shared pool thread can
   raise a modal dialog; and I/O and memory priority is *declared* in the request,
   never captured, because it is only partially queryable and remoting would
   otherwise silently promote a background caller's I/O.
6. **The path is resolved at submission**, because the process CWD is mutable by
   any thread and even perfect remoting would be racy. Long paths are not
   silently prefixed with the extended-length marker.
7. **The Win32 ring is not the `IoRing`.** Share the ring type, not the storage;
   unify at the *wait*, never by draining a kernel-owned producer into a bounded
   stage.
8. **The thing being built is an execution domain**, and the ergonomics come
   from a type-level traversal in which each step offers only the legal next
   steps -- not from ambient lookup.
9. **Ambient state is derived from an explicit binding, never the origin of
   one.** The callback context is the substrate; a trampoline-installed scoped
   TLS projection is the convenience, saved and restored rather than set and
   cleared.
10. **The request carries every handle-shaping decision**, because the opening
    thread is gone by the time the caller sees the handle.
11. **Two layers**, cut as catalogue-plus-faithful-execution (synchronous,
    testable with no ring, pool, or async) and delivery model -- not as
    "trivial implementation" and "ring implementation".
12. **v1 cancellation is pre-execution only.**

## Rejected alternatives

- **Sharing storage with the `IoRing` completion queue.** The `IoRing` API has
  no post/user-completion entry point, so a namespace completion cannot be
  placed in its CQ. Rejected on mechanism, not taste.
- **Draining the `IoRing` CQ into a unified ring via a `ThreadpoolWait`.**
  Technically possible with zero idle threads, but it introduces a copy and a
  bounded stage in front of a kernel-owned producer that cannot be
  backpressured. Rejected.
- **A dedicated thread per cancellable operation.** Considered as the way to get
  quarantine and mid-flight cancellation. Dropped once M-7 showed the pool
  performs quarantine-and-replace itself, and M-6 showed mid-flight cancellation
  is unsafe on a shared worker regardless. It remains the *only* shape in which
  mid-flight cancellation could work, so it is recorded rather than discarded.
- **Growing `ImpersonationToken` to carry the whole thread context.** Rejected on
  mechanism before layering: the aspects have different application windows, so a
  single type implies a single window that is wrong for at least one of them.
  Also rejected because it would impose impersonation's fail-fast restore
  semantics on aspects that do not warrant them, and would tax the crate's
  existing standalone consumer, which wants impersonation alone.
- **Thread-lifetime `thread_local!` ownership of a domain.** Rejected on three
  mechanical grounds: callers run on shared pool threads, so a domain would be
  stranded on process-shared infrastructure; TLS destructors are unreliable on
  Windows, so a ring's `Drop` might never run; and submission and completion are
  on different threads by construction, so the ambient binding is unavailable
  exactly where completion happens.
- **Steering concurrency by moving the pool maximum at runtime.** Viable --
  M-8 measured it working in ~1.2 ms -- but it steers a quantity that cannot be
  read back, through a void-returning setter, against the measured "last call
  wins" semantics. Preferred alternative: set the maximum once to the ceiling
  and bound concurrency with an admission counter, which is exact and owned.
- **A periodic supervisor timer.** Superseded during the session by the
  observation that the decision is only needed when there is work that cannot be
  dispatched, so it can be evaluated lazily at admission. That also makes the
  decision memoryless -- no "how long has the pool been full" state -- and keeps
  an idle domain at zero threads.
- **Classifying "stuck" versus "slow".** No finite observation distinguishes a
  90-second SMB timeout from a permanently wedged driver, and the response to
  both is identical: add a slot. The classifier is deleted rather than tuned.
- **Rejecting a per-call byte ceiling on scatter/gather.** Not from this
  session, but the same discipline: a review finding is a hypothesis, not an
  instruction.

## Open questions

- **Which session-independent form to substitute.** M-9 settled *that* the
  device map follows the impersonation token; it does not settle what the
  facility should do about it. Expansion is not uniform: a network mapping
  becomes a UNC path, a real local volume becomes `\Device\HarddiskVolumeN`
  (needing a `\?\GLOBALROOT\` prefix), and a `subst` becomes some other path
  entirely. `QueryDosDeviceW` distinguishes the three cleanly, so detection is
  cheap; substitution is the part that needs a decision.
- **Ordering within a session.** Submitting `DeleteFile(X)` then `CreateFile(X)`
  on a pool with N workers does not execute them in order. The contract must
  state this explicitly -- probably "unordered, with an optional serialized
  mode" -- rather than let it fall out of the implementation. Composed flows
  (open then read) are ordered for free by their data dependency; two
  independent namespace operations are not.
- **Disposal of undrained completions that own resources.** An async open's
  completion carries an owned handle. If the receiver is dropped with that
  completion undrained, the ring must destroy the record rather than discard it
  -- and the destructor can block, since closing a handle to a dead network path
  is exactly the operation this facility exists to keep off the caller's thread.
  This may argue that handle-returning operations need a disposal path back
  through the session.
- **The owned-association seam.** `AssociatedEndpoint<'port>` is lifetime-bound
  to a borrowed port, but here the port is `Arc`-shared. An owned-association
  form will be needed, in the same shape as the `OperationId::mint` seam that
  only became visible when a backend was built outside
  `windows-overlapped-io-sys`. Given M-4, this is needed only on the port branch
  of the fork.
- **The threshold `T` for presumed-stuck.** Must exceed the pool's injection
  interval, or the supervisor reads the pool's own throttle as evidence of stuck
  workers. With `runs_long` set that interval is microseconds, so any sane `T`
  clears it -- which is a further argument for making `runs_long` mandatory.
- **Whether `WOW64` filesystem redirection and thread I/O priority can be
  captured.** Redirection is thread-scoped and would silently change which files
  a 32-bit process sees. I/O priority is only partially queryable through
  documented API, which is why the design makes priority an explicit request
  field rather than sniffing ambient state.

## Corrections made during the session

Recorded because each was caught by a control or a debugger rather than by
review, and because the pattern is the one this repository has already named:
a guard you have only seen pass is untested.

1. **"An armed wait costs a fraction of a shared thread."** Asserted from the
   pre-Windows-8 implementation. Wait multiplexing moved into the kernel, so an
   armed `TP_WAIT` is genuinely zero threads. This *strengthens* the founding
   thesis and is load-bearing for it.
2. **"IOCP association and `IoRing` coexist."** The first version of the
   coexistence probe checked only *where* the completion arrived and never
   looked at its result code, which was `ERROR_INVALID_PARAMETER` with zero
   bytes. Rewritten with a positive control, a before/after pair on one handle,
   and a health check through the port -- reversing the verdict entirely.
3. **"`CancelSynchronousIo` is viable with a lock-guarded in-flight record."**
   The lock closes the user-mode window, but the argument assumed the call was
   point-in-time in a stronger sense than measured. It is point-in-time in
   *effect* and unbounded in *duration*.
4. **A vacuous test that read as a pass.** The first boundary-race test reported
   `op2 aborted: 0/200` while `cancel reported success: 0/200` -- it never hit
   anything, so it measured nothing. Its replacement reports valid-trial counts
   explicitly so a vacuous run cannot read as a pass.
5. **Two strawmen.** The engineer was told the supervisor must not run on the
   pool it supervises (never proposed otherwise) and that OS-level thread
   introspection was the wrong approach (the word had meant the facility's own
   bookkeeping, which was the recommendation). Both were stated as corrections
   when they were not.

6. **"The device map question needs a second logon session and credentials."**
   It needed neither. `LOGON32_LOGON_NEW_CREDENTIALS` mints a new logon session
   without validating credentials, and the question was answered in one run.
   An open question was left standing on an unexamined assumption about the
   cost of answering it -- which is the same failure as deferring work for a
   perceived lack of need, applied to measurement.

Two further notes on the device-map probe itself, kept because they are the same
class of hazard:

- **Its first token candidate could not answer the question, and the control is
  the only reason that was visible.** Anonymous impersonation returned "not
  found" for the subst letter -- the result the hypothesis predicted -- while
  also failing to resolve `C:`, which every context must resolve. Without the
  `C:` control that would have been read as a positive finding.
- **A third candidate was removed rather than explained.** Impersonating the
  UAC-linked token killed the process: execution reached a marker immediately
  before `ImpersonateLoggedOnUser` and never the one immediately after, with no
  panic message. It was localized, not root-caused, and removed only because the
  new-credentials token had already answered the question with a passing
  control. It should not be reintroduced without explaining the crash first.
