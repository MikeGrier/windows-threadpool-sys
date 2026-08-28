# Design session -- asynchronous file enumeration (2026-08-27)

Decisions produced so far: a reusable captured-impersonation crate, flat
one-directory enumeration in a second publishable crate, bounded SQ/CQ session
rings, lossless delivery, native `windows-sys` value types, finite work quanta, and
`GetFileInformationByHandleEx` caller-buffered directory enumeration.

> Tier-3 record. [DESIGN-NOTES.md](../DESIGN-NOTES.md) is authoritative and wins
> on any conflict.

---

## Starting intent

The new abstraction will enumerate Windows directory entries asynchronously without
dedicating a thread while idle. It belongs to the repository's founding theme:
Windows-native asynchronous Rust built from native Windows APIs and the Windows thread
pool, with zero idle thread overhead.

The intended capability is shaped like `FindFirstFileExW`, not Unix-compatible globbing.
A request supplies enumeration parameters and a predicate specification; results arrive
asynchronously on a bounded, crate-owned ring. The consumer side should follow the
windows-file-watcher queue model as closely as the different reliability requirements
allow. Each request enumerates one directory only. Recursive traversal is explicitly out
of scope and can be built by a higher layer submitting further directory-enumeration
requests.

Names and paths must retain Windows fidelity. The repository's `wtf-string` crate stores
native WTF-16 without repeated conversion and provides an already-established fit for
names returned by wide Win32 APIs, including ill-formed surrogate sequences.

The proposed first predicate form is query-by-example: optional attribute requirements
and exclusions, plus comparisons over size and the timestamps exposed by enumeration.
The enclosing predicate specification must admit future variants without replacing the
request API.

The shipped abstraction must also suffice as the Windows one-directory enumeration
backend for Globazog. This is an acceptance criterion, not merely a possible future use:
Globazog must be able to replace its current native enumeration path without losing
metadata fidelity, error reporting, path fidelity, backpressure behavior, or performance.
Globazog remains responsible for recursive traversal and pattern evaluation across path
segments; this lower layer still enumerates exactly one directory per request.

## Initial `FindFirstFileExW` research

**Superseded by the `GetFileInformationByHandleEx` directory-enumeration decision below.**

`FindFirstFileExW` and `FindNextFileW` are synchronous calls. `FindFirstFileExW` returns
the first `WIN32_FIND_DATAW` and a search handle; `FindNextFileW` advances that handle,
and `FindClose` releases it. `ERROR_NO_MORE_FILES` is successful exhaustion, while other
errors terminate or interrupt enumeration according to policy still to be designed.

The native result order is unspecified and file-system-dependent. The API accepts
wildcards in the final name component, not in directory components. The search string
must not end in a backslash. `FindExInfoBasic` omits the alternate 8.3 name;
`FIND_FIRST_EX_LARGE_FETCH` requests a larger directory-query buffer; and
`FIND_FIRST_EX_ON_DISK_ENTRIES_ONLY` matters in the presence of virtualization filters.

`WIN32_FIND_DATAW` supplies the attributes, creation time, last-access time,
last-write time, logical end-of-file size, primary name, alternate name when requested,
and a reparse tag when the reparse-point attribute is set. This data is a directory
enumeration snapshot and may already be stale when consumed. It does not supply every
possible file metadata field, such as allocation size, file identity, or change time.

The existing windows-file-watcher queue establishes several relevant contracts:

- the crate enqueues concrete records and never invokes client code;
- a lazily-created manual-reset event is the receiver doorbell;
- the event is signaled exactly while the receiver has something to observe;
- the queue is bounded;
- reliable control messages reserve capacity before their operation proceeds; and
- lossy watcher batches are converted into an explicit desynchronization report.

Enumeration results differ from watcher observations: silently dropping a matching entry
would make a completed enumeration false. The design must therefore decide how a
synchronous enumerator pauses and resumes when its bounded result ring has no room,
without blocking a Windows thread-pool worker on consumer progress.

## Globazog points to retain or reconsider

Globazog's predicate vocabulary uses explicit comparison operators (`<`, `<=`, `==`, `!=`,
`>=`, `>`), separate all-set and all-clear attribute masks, entry type, reparse status
and tag, size, creation time, access time, modification time, and depth. Its broader
cross-platform metadata model also has change time, which `WIN32_FIND_DATAW` does not
provide and therefore is not automatically part of this design. Depth and a distinct
descent predicate do not apply because this abstraction does not recurse.

The current Globazog Windows backend does not in fact use `FindFirstFileExW` /
`FindNextFileW`. It opens a directory handle and repeatedly calls the documented
`GetFileInformationByHandleEx` API with `FileIdExtdDirectoryRestartInfo` and
`FileIdExtdDirectoryInfo`, parsing a caller-owned 64 KiB buffer. Each returned
`FILE_ID_EXTD_DIR_INFO` provides creation, last-access, last-write, and change times,
logical and allocation sizes, attributes, reparse tag, a 128-bit file ID, and a
variable-length UTF-16 name. Globazog separately queries the directory handle's volume
serial when file identity is needed.

That is a wider contract than `WIN32_FIND_DATAW`, which has no change time, allocation
size, or file ID. A result shape limited to `WIN32_FIND_DATAW` therefore cannot replace
Globazog's current backend without extra per-entry opens, lost metadata, or a second
enumeration path. Conversely, the handle-based API exposes its batch buffer directly:
the abstraction can retain and parse a partially consumed buffer across work callbacks
and knows exactly when the next API call may block. This removes the hidden-buffer
problem that motivated the dual call/time approximation for `FindNextFileW`.

The native primitive is therefore `GetFileInformationByHandleEx` with
`FileIdExtdDirectoryRestartInfo` for the first refill and
`FileIdExtdDirectoryInfo` thereafter. `FindFirstFileExW` / `FindNextFileW` are not used.
This remains entirely on the documented Win32 API. The abstraction owns an opened
directory handle, a fixed caller-owned native result buffer, a cursor into the valid
records in that buffer, and its position in the directory enumeration.

## Captured impersonation transport

The capture/transport/apply primitive is reusable platform functionality, not an
enumeration implementation detail. It will live in the independently published
`windows-impersonation-token-sys` workspace crate as the opaque owned
`ImpersonationToken` type. The independently published
`windows-file-enumeration-sys` crate will depend on it.

An enumeration begin message carries an already captured token. The ordinary SQ
begin helper captures the calling thread's current impersonation context
synchronously before publishing the message, so callers do not hand-write token
plumbing. An explicit-token begin form lets the future traversal crate capture once
at traversal submission and reuse the same context for every descendant-directory
open. It must not recapture from a worker.

Applying a captured token saves the exact prior worker thread-token state and
restores it after the directory open on success, error, and unwind. The application
guard is thread-bound. Restoration failure is fail-fast because silently returning
a shared worker under the wrong identity would contaminate unrelated process work.
A raw native token handle cannot express these invariants, which justifies the
crate-owned type under the repository's native-type interoperability rule.

Legacy `QueueUserWorkItem` already offers `WT_TRANSFER_IMPERSONATION`, but the
modern object-based thread pool used by this repository offers no corresponding
callback-environment setting. WIL has C++ helpers for opening the current access
token and exactly restoring a prior token; `windows`, `windows-sys`, and
`windows-threading` do not package the full Rust abstraction.

## Session and two-ring model

A session owns two bounded rings: an SQ carrying control messages into the
abstraction and a CQ carrying entries and terminal outcomes to one receiver.
Clonable session handles are the multi-producer side of the SQ. Multiple concurrent
enumerations may share a session, and every CQ record carries an `EnumerationId`.
A client obtains per-enumeration isolation by creating one session per enumeration.

The SQ carries at least begin-enumeration, cancel-enumeration, and
abandon-session messages. One logical authority drains it in FIFO order.
`ThreadpoolWork::submit` is a coalesced doorbell: enqueueing while a drain is
already scheduled or running must not schedule an empty second drain. The SQ
servicer mutates the enumeration registry and schedules per-enumeration work; it
does not perform native refills.

Ordinary begin submission is nonblocking and is rejected synchronously if it cannot
enter the bounded SQ. Each accepted enumeration first reserves one CQ terminal slot
and one SQ cancellation slot. Its affine handle owns the cancellation reservation,
so explicit cancellation and handle drop can enqueue cancellation exactly once
without blocking or failing because ordinary traffic filled the ring.

Receiver drop is session abandonment. The session retains one standing SQ
reservation for a single abandon-session message; receiver drop rejects future
begins, enqueues abandonment without blocking, and wakes every stalled enumeration
to release its token, directory handle, native buffer, and work state. No terminal
outcome is owed after receiver abandonment because no observer remains.

CQ terminal reservations must leave at least one unreserved data slot while an
enumeration is active. A CQ capacity of one cannot support this contract. Sharing a
session intentionally shares backpressure; isolation is structural through another
session rather than a bypass around a full shared CQ.

## Settled scope and delivery properties

- One request enumerates one directory. The abstraction does not recursively traverse
  subdirectories.
- Delivery is lossless. Every entry accepted by the predicate must either reach the
  result ring or the request must finish with an explicit failure or cancellation
  outcome; ring pressure may not silently discard an entry.
- A full result ring applies backpressure to the enumeration itself.
- Backpressure must never block a thread-pool worker. Records already returned in the
  caller-owned native buffer are parsed only after establishing that the result ring and
  its backing storage have room for the next accepted entry. When they do not, the
  enumerator retains the native buffer and its current record offset, returns from the
  callback, and resumes after consumer progress makes room.
- `GetFileInformationByHandleEx` is called only when the previous native buffer has been
  fully consumed, so a refill can never overwrite an undelivered entry. A refill may
  return more records than currently fit in the result ring; the fixed native buffer is
  the bounded staging area for that batch. The enumerator does not refill while the
  result ring has no room, because doing potentially blocking I/O before any result can
  be consumed would be pointless.
- The security context used to open the native search is distinct from the security
  context of later execution. The effective token under which the directory handle is
  opened determines its access. Subsequent `GetFileInformationByHandleEx` calls use that
  already-open handle and may execute on different thread-pool workers whose current
  thread tokens are unrelated. Thread-pool dispatch does not itself carry the request
  submitter's impersonation token.
- Request submission captures the submitter's effective security token without opening
  the directory synchronously. The worker impersonates that captured token only while
  opening the directory handle, reverts immediately afterward, and then uses the opened
  handle for later enumeration calls on any worker. A worker must restore its prior
  thread security context before returning to the shared Windows thread pool, including
  every failure path.
- Native Windows values retain the corresponding `windows-sys` types when those types
  already express the contract. In particular, enumeration timestamps use the
  `windows_sys::Win32::Foundation::FILETIME` type rather than a crate-owned timestamp
  wrapper. The crate may re-export that type for API convenience and converts its two
  fields internally to a `u64` tick count when evaluating comparisons. This is
  intentional public coupling: these crates complement the Microsoft `windows` and
  `windows-sys` crates rather than defining a competing Windows type system.
- Enumeration advances in bounded work quanta. Each quantum has both a parsed-record
  budget and an elapsed-time budget. Every examined record consumes the record budget,
  including records rejected by the predicate; otherwise a reject-all predicate could
  monopolize a worker indefinitely. Cancellation and result capacity are checked between
  records. A refill may synchronously block and no budget can preempt or bound that one
  Win32 call, but the refill boundary is known and parsing the returned batch is entirely
  under this abstraction's control.
- The elapsed-time budget uses a cheap monotonic Windows scheduling counter, such as a
  tick count or interrupt-time count, rather than wall-clock time, `SystemTime`, or a
  `FILETIME`. It measures only whether the callback should yield. Its exact source,
  resolution, call-check cadence, and both budget values are internal implementation
  tuning informed by measurement; they are not request parameters or API guarantees.
- Enumeration work is created with the callback environment marked as potentially
  long-running because any native-buffer refill may perform synchronous I/O. This lets the
  Windows thread pool account for a blocked callback when deciding whether another
  worker is needed; it does not weaken the finite-quantum or no-ring-blocking rules.
- A request reserves capacity for its terminal outcome before enumeration is allowed to
  begin, so lossless entry delivery cannot consume the only space in which completion,
  failure, or cancellation must be reported.
- Each request owns one reusable native enumeration buffer. Its default capacity is
  64 KiB, matching the existing Globazog Windows backend. The request may tune this
  capacity; requested values below 1 KiB are clamped to 1 KiB. Buffer allocation and
  alignment must satisfy `FILE_ID_EXTD_DIR_INFO`, and the effective capacity is fixed
  for the request rather than growing as a hidden response to directory contents.
- A callback performs at most one `GetFileInformationByHandleEx` buffer refill. It may
  parse the newly returned records until ring pressure or its record/time budget causes
  it to yield, but after draining that buffer it resubmits rather than issuing a second
  refill in the same callback. This creates a scheduling point at every possible
  synchronous-I/O boundary.
- `GetFileInformationByHandleEx` has no documented `OVERLAPPED`, event, APC, completion
  routine, or IOCP form. It therefore cannot be associated with `ThreadpoolIo`; the
  abstraction uses `ThreadpoolWork` to execute each synchronous refill. Calling
  `NtQueryDirectoryFile` directly could expose asynchronous completion machinery, but
  that would abandon the documented Win32 surface and is explicitly not part of this
  design.
- Yielding resubmits the retained enumeration state as single-flight work. Returning from
  the callback creates a scheduling point: the pool may run other work, or may resume the
  enumeration immediately when there is no competition. Resubmission must not permit two
  callbacks to refill or parse the same directory-enumeration state concurrently.

This token distinction exposes a known defect in the Globazog prior art: deferring the
open to unrelated worker execution can unintentionally enumerate under the process or
worker security context instead of the submitter's effective token. This session records
the defect as a design warning only; it does not schedule work in the Globazog
repository.

## Open pressure points

- How consumer progress wakes a capacity-stalled enumeration and resubmits its work
  without polling or a lost-wakeup race.
- The exact native mechanics by which `ImpersonationToken` represents a submitter
  with no thread token, preserves identification/delegation levels, and reports
  anonymous or otherwise uncapturable contexts before SQ acceptance. This belongs
  to the helper crate and is scheduled before enumeration implementation.
- The measured parsed-record and elapsed-time budgets, the cheap monotonic counter used for
  the latter, and how often that counter is sampled inside a quantum.
- Which events are guaranteed: entries, per-directory errors, cancellation, and one
  terminal completion.
- Whether the Win32 name wildcard is part of the request in addition to the structured
  predicate, and whether matching semantics are owned by this crate or delegated to the
  file system.
- What fallback or capability contract is required where
  `FileIdExtdDirectoryInfo` is unavailable.
- Whether change time, allocation size, 128-bit file ID, extended-attribute size, and
  volume identity are always returned, optional result fields, or a requested result
  shape. Globazog replacement requires that they remain obtainable without per-entry
  opens.
- Which path forms and long-path behavior the request accepts.
- Whether native order is merely exposed as unspecified or whether any stable ordering
  mode is offered.
