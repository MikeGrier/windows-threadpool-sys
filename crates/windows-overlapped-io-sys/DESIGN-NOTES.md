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

### Voluntary rundown versus `Drop`

Rundown has two distinct paths with different obligations, and the difference is deliberate.

- Voluntary rundown is an explicit method that **blocks** with the full semantics: it prevents new submissions,
	cancels every outstanding operation, drains and observes all of their completions, frees the associated
	storage, and returns only once the kernel is guaranteed to be done with every operation's storage. Blocking
	belongs here and only here, because only an explicit call may take unbounded time.
- `Drop` must be **memory-safe** and must **never block for correctness**. Those two requirements can only be
	met together by refusing to free storage the kernel might still own: leaking that storage is memory-safe,
	whereas freeing it while a completion is still pending is a use-after-free. `Drop` therefore closes the native
	handle and port -- which is sound, because leaked storage stays valid for any late kernel writes -- and
	abandons any operation storage that voluntary rundown did not already reclaim. It does not wait for
	outstanding completions.

This rests on the invariant that already governs the crate: per-operation storage is freed only after that
operation's completion has been observed. Voluntary rundown observes every completion before returning; `Drop`
observes none and leaks. Both uphold the invariant, so no path can produce a post-`Drop` kernel write into freed
memory.

`Drop` must not **panic**: a leak is not a memory-safety violation, and unwinding from `Drop` is harmful. When
`Drop` runs with operations still outstanding it should instead emit a best-effort diagnostic carrying enough
context to make the leak diagnosable -- at least the count of abandoned operations. A zero-dependency `-sys`
crate may only be able to do this on a best-effort basis (for example behind an optional logging feature or as a
last-resort write to standard error), and full context may not always be recoverable; the diagnostic is
advisory, not a correctness mechanism.

Draining generically is the implementation crux: because the port delivers untyped completions, freeing an
abandoned operation's storage without its `P` requires either type-erased reclamation recorded in the operation
header or a caller-typed drain that supplies `P`. This choice is bound to the same generic-submission boundary
that the rest of the crate is prototyping and must be resolved as part of it.

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
- Operation-family bindings are gated behind Cargo features so the core stays minimal: file read / write,
	scatter / gather, `DeviceIoControl`, and `SetFileCompletionNotificationModes` under `Win32_Storage_FileSystem`;
	socket operations under `Win32_Networking_WinSock`. Enabling a family turns on only the `windows-sys` features
	that family needs.
- Minimum supported Windows version is the shared workspace baseline: the current public releases validated by
	GitHub CI, namely Windows Server 2025 (`windows-latest`) and Windows 11. `CancelIoEx` and
	`GetQueuedCompletionStatusEx` are available there without down-level gating; per-handle notification-mode
	support (`SetFileCompletionNotificationModes`) still varies by device and is treated as a runtime capability,
	not a compile-time guarantee. The Rust baseline is 1.97 (the MSRV) on edition 2024.

## Crate boundary summary

`windows-overlapped-io-sys` exports the endpoint owners, provenance and sealed types, pinned operation storage
with identity and state, the result model, the cancellation primitives, the raw IOCP and event backends, and
the backend trait a thread-pool implementation consumes. `windows-threadpool-sys` depends on this crate to
implement the `TP_IO` backend and its `StartThreadpoolIo` accounting alongside its callback environment, work,
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
