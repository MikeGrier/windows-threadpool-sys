# Design notes: windows-overlapped-io-sys

This crate owns the reusable ownership, association, completion, cancellation, and rundown model for Windows
overlapped I/O over `windows-sys`. It is the lower half of a pair: `windows-threadpool-sys` builds its
thread-pool I/O backend on the endpoint and operation storage defined here. The two crates are versioned and
published independently, and this crate must never depend on `windows-threadpool-sys` or reference any `TP_*`
type.

The goal of this document is to specify the crate's requirements broadly enough to be functional across the
full range of Windows overlapped operations, not only the directory-notification and thread-pool scenarios that
motivated it. The public surface is designed so that every overlapped operation family below can be expressed
with the same storage, identity, result, and rundown machinery, even where a given operation ships later or in
a downstream crate.

## Purpose and scope

In scope:

- Typed ownership of overlapped-capable endpoints and their documented destruction.
- Provenance-controlled association of an endpoint with exactly one completion backend.
- Pinned, per-request `OVERLAPPED` storage coupled to its payload, offsets, and explicit operation state.
- Completion identity, a uniform result model, and cancellation primitives.
- A raw I/O completion port (IOCP) backend and an event / `GetOverlappedResult` backend.
- A backend seam that `windows-threadpool-sys` implements for `TP_IO` without this crate knowing about it.

Out of scope:

- Interpreting operation payloads. Buffers are opaque byte or 16-bit-unit sequences; the crate never decodes,
	copies for the caller, validates, or imposes a record format.
- Filesystem, socket, path, or protocol policy. The crate does not open, name, or configure endpoints beyond
	what association requires.
- Executor, channel, or reactor choice. The crate never blocks a caller's dequeue thread on downstream
	progress and never runs arbitrary application code from inside a completion.
- Creating or managing worker threads. The owner of a raw IOCP chooses where and how packets are dequeued.
- APC / alertable completion routines (`ReadFileEx`, `WriteFileEx`, and the Winsock completion-routine forms).
	These use a per-thread, non-port, non-`OVERLAPPED`-identity delivery model and are deferred as a separate
	backend if a use ever appears; they are not part of the core model.

## Range of operations the model must accommodate

The storage, identity, and lifetime machinery must be sufficient to express all of the following without a
redesign. Specific safe wrappers may be added incrementally or live downstream, but the core must not foreclose
any of them.

Endpoint categories:

- Regular files and volumes opened with `FILE_FLAG_OVERLAPPED`.
- Named and anonymous pipes.
- Communications / serial resources (`WaitCommEvent` and comm reads and writes).
- Arbitrary devices driven through `DeviceIoControl`.
- Directory handles used for change notification (`ReadDirectoryChangesW` / `ReadDirectoryChangesExW`).
- Sockets (`WSARecv`, `WSASend`, `WSARecvFrom`, `WSASendTo`, `AcceptEx`, `ConnectEx`, `DisconnectEx`,
	`WSAIoctl`, `WSARecvMsg`, `WSASendMsg`, `TransmitFile`).

Console handles are excluded because they do not participate in overlapped completion.

Operation shapes the payload model must cover:

- Single contiguous buffer read and write (`ReadFile`, `WriteFile`).
- Vectored buffers: `WSABUF` arrays for sockets and `FILE_SEGMENT_ELEMENT` arrays for scatter / gather
	(`ReadFileScatter`, `WriteFileGather`). The descriptor array is itself part of the pinned operation storage.
- Operations with an extra output block whose layout the kernel fills, such as the address buffer of
	`AcceptEx` or the control buffer of `WSARecvMsg`.
- Zero-buffer control operations (`ConnectNamedPipe`, `DeviceIoControl` with no data, `WaitCommEvent`).
- Seek-carrying operations that use `OVERLAPPED.Offset` / `OffsetHigh`, versus non-seekable endpoints that must
	leave those fields zero.
- Caller-injected user packets via `PostQueuedCompletionStatus` on a raw IOCP, used for wakeups and shutdown
	signalling. These carry a completion key and transfer count but no real `OVERLAPPED` operation.

## Completion backends

The crate defines backends as distinct types that share endpoint and operation storage but never emulate one
another.

Raw IOCP backend (implemented here):

- Owns the completion-port handle, the per-handle completion key, the concurrency value chosen at port
	creation, and packet dequeue via `GetQueuedCompletionStatus` and batched `GetQueuedCompletionStatusEx`.
- Supports many handles per port, distinguished by completion key, and supports `PostQueuedCompletionStatus`
	for user packets.
- Does not create dequeue threads. The owner decides whether dequeue is single-threaded, multi-threaded, or
	batched, and where it runs.

Event / `GetOverlappedResult` backend (implemented here):

- For overlapped endpoints that are not associated with any port. Uses `OVERLAPPED.hEvent` plus
	`WaitForSingleObject` and `GetOverlappedResult` / `GetOverlappedResultEx`.
- Naturally serializes to the operations an event can distinguish; it is the fallback when port association is
	unavailable or undesired.

Thread-pool I/O backend (implemented in `windows-threadpool-sys`):

- Consumes the endpoint and operation storage exported here through a backend trait defined here, then adds the
	`StartThreadpoolIo` / `CancelThreadpoolIo` accounting and callback dispatch that only the thread-pool crate
	understands.
- The system-managed internal port is never exposed, and this crate never posts to it or dequeues from it.

## Endpoint ownership and provenance

- Reuse `std::os::windows::io::OwnedHandle`, `OwnedSocket`, and their borrowing traits where the documented
	destructor is `CloseHandle` or `closesocket`. Add typed owners only for resources with specialized
	destruction (for example change-notification handles). Do not introduce an untyped universal handle owner or
	treat integer-like resources as interchangeable.
- An unassociated overlapped endpoint transitions, by a consuming typestate move, to an endpoint bound to
	exactly one backend. Association is one-time and, for IOCP, not removable; operations issued through a
	duplicated handle also complete against that association.
- Ownership alone does not prove an endpoint is safe to associate. A safe associated-endpoint constructor needs
	controlled provenance: it must either create the endpoint itself or consume a sealed type whose creator
	established overlapped mode and exclusive completion routing. An associated endpoint exposes neither cloning
	nor an unrestricted raw-handle escape hatch.
- `SetFileCompletionNotificationModes` (`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`,
	`FILE_SKIP_SET_EVENT_ON_HANDLE`) changes whether a completion packet arrives on synchronous success. It is
	modeled as an opt-in endpoint provenance attribute, because it directly alters the "a packet will or will not
	arrive" invariant that reclamation depends on. It lives behind a feature gate because it pulls in
	`Win32_Storage_FileSystem`.

## Operation storage and identity

- Each in-flight request owns pinned, stable storage holding its `OVERLAPPED`, its typed payload, any descriptor
	array or extra output block, its offsets, and an explicit state machine (idle, submitted, pending, completed,
	cancelled).
- The address of the `OVERLAPPED` is the completion identity. It must be stable for the operation's entire life
	and must not be moved, reused, or freed until a completion for that exact address has been observed (or the
	no-packet path has been proven). Identity mapping from a dequeued packet back to its owning operation goes
	through that address.
- Payload bytes remain opaque. Names and records that happen to be UTF-16 are not decoded; a higher layer may
	publish validated offsets into retained buffers without this crate copying or interpreting them.

## Result and status model

- A completion yields a uniform result: bytes transferred plus a normalized status. The status is reconciled
	from the backend-specific source: the failure code surfaced when `GetQueuedCompletionStatus` reports a
	non-null `OVERLAPPED`, the `IoResult` argument of the thread-pool callback, or `GetOverlappedResult` for the
	event backend. `Internal` and `InternalHigh` are read only through these sanctioned paths.
- A completed operation's result and payload survive native endpoint shutdown. Tearing down the port or handle
	must not invalidate a result the owner has not yet consumed.
- Synchronous completion is distinguished from a pending submission at the submit seam, because only the caller's
	native call knows which occurred. The raw IOCP backend's `submit` closure classifies the outcome as an
	`Issued`: `Pending` when a completion packet will be delivered (native success that queues a packet, or
	`ERROR_IO_PENDING`), or `Completed { bytes_transferred }` when the call finished synchronously with no packet
	to arrive -- the state a handle in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode reports on synchronous success.
	A `Completed` outcome is reclaimed inline and returned through `Submitted::Completed`; only a `Pending` outcome
	leaves an operation counted as outstanding. Misclassifying a synchronous success as `Pending` would leave the
	outstanding count permanently high and hang rundown, so the distinction is part of the seam's safety contract,
	not an optimization.

## Cancellation and rundown

- Targeted cancellation uses `CancelIoEx(handle, &overlapped)`; whole-endpoint cancellation uses
	`CancelIoEx(handle, null)`. Cancellation is only a request: it does not establish completion, and the matching
	packet (or the fact that one already arrived) remains the reclamation trigger. Storage is never freed on the
	strength of a cancellation call alone.
- Rundown ordering: prevent new submissions, request cancellation or otherwise account for every outstanding
	operation, drain completions, and only then close the port and endpoint. Per-operation storage is reclaimed
	solely after that operation's completion has been observed.
- The backend seam expresses the accounting difference: the thread-pool backend must balance every
	`StartThreadpoolIo` with a callback or a `CancelThreadpoolIo`, whereas the raw IOCP and event backends have no
	such counter. The core exposes the hooks these require without encoding thread-pool accounting itself.

### Voluntary rundown and `Drop`

Rundown has two entry points that share the same blocking semantics; the difference is only who initiates it.

- Voluntary rundown is an explicit method that **blocks**: it prevents new submissions, cancels every
	outstanding operation, drains and observes all of their completions, frees the associated storage, and
	returns only once the kernel is guaranteed to be done with every operation's storage. It is the preferred
	path because the caller chooses when the wait happens.
- `Drop` is the safety net, and it **must also block** to stay memory-safe. `Drop` reclaims the operation
	storage, so before freeing it must guarantee the kernel has finished writing to every outstanding
	`OVERLAPPED`; that guarantee can only be obtained by cancelling the outstanding operations and waiting for
	their completions. A non-blocking `Drop` that freed the storage could permit a post-`Drop` kernel write into
	freed memory -- exactly the use-after-free the crate must prevent. `Drop` therefore performs the same
	cancel-drain-free rundown synchronously before closing the native handle and port.

This rests on the invariant that governs the crate: per-operation storage is freed only after that operation's
completion has been observed. Both paths satisfy it by observing every completion before freeing; the only
difference is that voluntary rundown lets the caller control the timing, while `Drop` blocks whenever the caller
did not run down first.

The blocking drain belongs to whichever object owns the completion stream -- the completion port for the raw
IOCP backend. An associated endpoint's own teardown cancels its outstanding operations (closing its handle does
this) and defers reclamation to that drain, so the port's rundown is what finally frees the storage.

`Drop` must not **panic**: unwinding out of `Drop` is harmful, and blocking-then-freeing is the correct
behavior, not an error. Because a blocking `Drop` signals that the caller omitted an explicit rundown, `Drop`
should emit a best-effort diagnostic when it runs with operations still outstanding, carrying enough context to
locate the missing rundown -- at least the count of operations it had to drain. A zero-dependency `-sys` crate
may only manage this on a best-effort basis (behind an optional logging feature or as a last-resort write to
standard error), and full context may not always be recoverable; the diagnostic is advisory, not a correctness
mechanism.

The raw IOCP backend keeps rundown correctness lock-free: an atomic outstanding count governs draining, and
each operation's `repr(C)` header carries a type-erased reclaim thunk that frees it from the `OVERLAPPED` pointer
alone. Source tracking is a separate, opt-in diagnostic layer: when enabled it records each operation's
`#[track_caller]` submit `Location` in a per-port map, so the drop-time message can name the sources. It is a
one-time in-process setting (`set_source_tracking`) that defaults from the `WINDOWS_OVERLAPPED_IO_SYS_TRACK`
environment variable and is off by default, because when on it takes a mutex on the submission hot path. With
tracking off, the drop message reports only the count and how to enable sources. The optional
`operation-backtrace` cargo feature additionally captures a full backtrace per submission -- itself gated at run
time by `RUST_BACKTRACE` -- giving both build-time and run-time control over how much context is retained.

Because `Drop` must free outstanding storage without knowing each operation's `P`, generic drain is mandatory
rather than optional: the operation header must record a type-erased reclamation function that frees the storage
from the `OVERLAPPED` pointer alone. That mechanism is what lets both voluntary rundown and `Drop` reclaim
heterogeneous operations on one endpoint, and it advances the generic-submission boundary the rest of the crate
is prototyping.

### Multi-endpoint and multi-threaded drain

One `CompletionPort` may serve many endpoints, and its completion stream may be drained by several threads at
once. The decisions that make that safe:

- The port -- not the endpoint -- owns the completion stream and is the single drain authority. Outstanding
	operations are counted once per port (a shared atomic), not per endpoint, so `run_down` drains *every*
	endpoint's outstanding operations in one pass; there is deliberately no per-endpoint rundown. An endpoint's
	own teardown only cancels its in-flight operations -- closing its handle issues an implicit
	`CancelIoEx(handle, null)` -- and defers reclamation to the port's drain. The lifetime binding (each
	`AssociatedEndpoint` borrows its port) enforces the order: all endpoints drop first, cancelling their
	operations, and the port's `run_down` or blocking `Drop` then observes and frees every resulting completion.
- Completions are attributed on two independent axes. The completion *key* identifies which endpoint a packet
	came from (each association fixes its key); the `OVERLAPPED` address identifies which operation, and `claim`
	recovers its typed storage. Neither axis depends on which thread dequeued the packet, so a shared port with a
	distinct key per endpoint keeps endpoints' completions distinguishable even when drained concurrently.
- Any number of threads may call `get` on the same `&CompletionPort` concurrently; the kernel hands each queued
	packet to exactly one caller, and the shared atomic count plus per-completion reclamation stay correct under
	that concurrency (`CompletionPort` is `Sync`). The one restriction is that a `Completion` is neither `Send`
	nor `Sync` -- it borrows the port's shared state and carries a raw `OVERLAPPED` -- so each thread must claim
	or drop the completions it dequeues on that same thread rather than handing them to another. Multi-threaded
	draining is therefore "many workers each dequeue-and-process", never "one thread dequeues and forwards".

Behavioral matrix every backend must be exercised against:

- immediate submission failure with no packet to arrive;
- immediate success, including the skip-on-success mode where no packet arrives;
- pending completion delivered later;
- targeted cancellation racing an in-flight completion;
- whole-endpoint cancellation racing multiple in-flight completions;
- completion identity under many simultaneous operations;
- endpoint shutdown with operations still outstanding; and
- results and payloads retained after native endpoint shutdown.

## `windows-sys` feature layout

- Core (always on): `Win32_Foundation` and `Win32_System_IO`, which supply `OVERLAPPED`, `OVERLAPPED_ENTRY`,
	`CreateIoCompletionPort`, `GetQueuedCompletionStatus`, `GetQueuedCompletionStatusEx`,
	`PostQueuedCompletionStatus`, `CancelIoEx`, and `GetOverlappedResult` / `GetOverlappedResultEx`.
- The event backend additionally needs `WaitForSingleObject`, which lives under `Win32_System_Threading`.
- The published default feature set is empty (`default = []`): the safe endpoint creator
	([`UnassociatedEndpoint::open`](src/endpoint.rs)) opens overlapped handles through `std::fs::OpenOptions`
	(`custom_flags(FILE_FLAG_OVERLAPPED | …)`), so the core completion machinery needs no operation-family
	`windows-sys` bindings at all.
- Operation-family bindings are gated behind three additive Cargo features so the core stays minimal, one per the
	families the checklist enumerates:
	- `fs` → `Win32_Storage_FileSystem` — file read / write, scatter / gather, and
		`SetFileCompletionNotificationModes` (`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`).
	- `socket` → `Win32_Networking_WinSock` — overlapped socket operations.
	- `device` → `Win32_System_Ioctl` — device control-code (IOCTL / FSCTL) definitions. `DeviceIoControl`
		itself is already in the always-on core (`Win32_System_IO`); the feature only adds the control-code
		constants a device family needs.
	Enabling a family turns on only the `windows-sys` features that family needs and never changes the completion
	machinery. Tests that issue real overlapped I/O pull the same bindings in through dev-dependencies, so the
	families are not required to exercise a backend.
- Minimum supported Windows version is the shared workspace baseline: the current public releases validated by
	GitHub CI, namely Windows Server 2025 (`windows-latest`) and Windows 11. `CancelIoEx` and
	`GetQueuedCompletionStatusEx` are available there without down-level gating; per-handle notification-mode
	support (`SetFileCompletionNotificationModes`) still varies by device and is treated as a runtime capability,
	not a compile-time guarantee. The Rust baseline is 1.97 (the MSRV) on edition 2024.

## Submission seam

Both backends -- the raw IOCP backend here and the `TP_IO` backend in `windows-threadpool-sys` -- perform the
same ownership transfer, so it is exposed as a small set of primitives rather than duplicated. An operation is
handed to the kernel with `Operation::into_overlapped`, which arms a type-erased reclaim thunk in the header,
leaks the boxed storage, and returns the stable `OVERLAPPED` identity to pass to exactly one native call. The
operation is recovered with `Operation::from_overlapped` when the payload type is known (an immediate failure or
a typed completion) or with the free `reclaim_overlapped` when it is not (rundown, where a backend frees
operations of mixed payload types). Endpoints reach a backend by consuming an `UnassociatedEndpoint`:
`CompletionPort::associate` for IOCP, `BlockingEndpoint::new` for the blocking backend, and, in the thread-pool
crate, `CreateThreadpoolIo` over the handle taken with `UnassociatedEndpoint::into_handle`.

The raw IOCP backend is implemented on top of these primitives, which validates that the seam is sufficient for
a real backend. `TP_IO` reuses them unchanged and adds only its own concerns: `StartThreadpoolIo` before each
submission, `CancelThreadpoolIo` to balance an immediate failure, and reclamation from its callback (typed when
the operation family is known, or `reclaim_overlapped` during object rundown). This crate never links the
thread-pool functions or references `TP_*`.

## Crate boundary summary

`windows-overlapped-io-sys` exports the endpoint owners, provenance and sealed types, pinned operation storage
with identity and state, the submission seam (`into_overlapped` / `from_overlapped` / `reclaim_overlapped`), the
cancellation primitives, and the raw IOCP and blocking backends. `windows-threadpool-sys` depends on this crate
to implement the `TP_IO` backend and its `StartThreadpoolIo` accounting alongside its callback environment, work,
timer, and wait objects. The dependency is one-directional.

## Open boundary

Fully generic, fully safe overlapped submission remains the decisive unresolved question, exactly as recorded
for the pair: a safe API cannot accept an arbitrary handle, `OVERLAPPED` pointer, payload, and caller-reported
submission result while proving that exactly one operation was issued, that it used the supplied storage, that a
packet will or will not arrive, and that storage is reclaimed only after real completion. A constrained
owned-operation prototype must resolve this before any public submission API is committed. The outcome will
decide whether safe submission is generic, requires operation-specific safe adapters (possibly downstream), or
retains a deliberately narrow unsafe extensibility seam.

## Primary references

- [I/O Completion Ports](https://learn.microsoft.com/windows/win32/fileio/i-o-completion-ports)
- [`CreateIoCompletionPort`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-createiocompletionport)
- [`GetQueuedCompletionStatusEx`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-getqueuedcompletionstatusex)
- [`PostQueuedCompletionStatus`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-postqueuedcompletionstatus)
- [`OVERLAPPED`](https://learn.microsoft.com/windows/win32/api/minwinbase/ns-minwinbase-overlapped)
- [`CancelIoEx`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)
- [`GetOverlappedResult`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-getoverlappedresult)
- [`SetFileCompletionNotificationModes`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-setfilecompletionnotificationmodes)
- [I/O Concepts (overlapped I/O)](https://learn.microsoft.com/windows/win32/fileio/synchronous-and-asynchronous-i-o)
