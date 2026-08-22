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

## Skip-on-success completion notification modes (M10)

- **`Issued` answers "will a completion packet arrive", not "did the call finish synchronously".** Those come
	apart precisely because an IOCP-associated overlapped handle receives a packet for *every* request it
	completes, including one that returns success immediately without `ERROR_IO_PENDING`. The proof is in
	`FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`'s own definition, which says the I/O Manager "does not queue a
	completion entry to the port, *when it would ordinarily do so*" -- ordinarily, it does. So an immediate
	`TRUE` is `Issued::Pending` on a default endpoint and `Issued::Completed` on a skip-mode one, and the same
	native return value means different things depending on the endpoint. This is the single most
	misread part of the seam, so both `Issued` variants document it at length.
- **The notification mode is tracked on the endpoint, not passed per call.** This is what
	"opt-in endpoint provenance attribute" has to mean in practice. `set_notification_modes` records what it
	established on `UnassociatedEndpoint`, `CompletionPort::associate` carries it into `AssociatedEndpoint`, and
	the adapters read it to classify. A fire-and-forget setter that only called Win32 would leave every adapter
	unable to answer the one question the seam requires, which is exactly the defect that made an ioctl on a
	skip-mode endpoint hang rundown before M10.5. The mode is accumulated rather than replaced, because Win32
	cannot clear one once set; `assume_overlapped` therefore requires a caller who set a mode on the raw handle
	to re-declare it, so the endpoint's record agrees with the handle's reality.
- **The mode is set before association, deliberately.** The flag is inert until the handle reaches a port, so
	establishing it first means there is never a window in which an operation could be issued against a handle
	whose notification behaviour is still undecided.
- **The synchronous byte count lives in the operation header, not on the submitting stack frame.**
	`Operation` carries a `sync_bytes` cell (before `payload`, so the reclaim thunk's offset stays identical for
	every `P`) that adapters pass as `lpNumberOfBytesTransferred` / `lpBytesReturned`. A stack local would be a
	dangling write: `DeviceIoControl` documents that with a non-null `lpOverlapped` the count "is meaningless
	until the overlapped operation has completed", so the kernel may write it after the submitting call has
	returned. Scatter/gather has no such out-parameter at all -- that argument slot is `lpReserved` -- so it
	recovers the count from `GetOverlappedResult` with `bWait: FALSE`, which is the sanctioned reader and cannot
	block because the operation is already complete.
- **The buffer-owning adapters report a two-state outcome (`Started`) rather than hiding the synchronous case.**
	An adapter owns the operation's buffers, so the two paths differ in *who owns the payload*, not merely in
	timing: a caller that ignored the distinction would either wait forever for a packet that is not coming or
	drop a result already delivered. `Started::Completed` reduces the operation to exactly the payload the
	token's `claim` would have yielded, so both arms report the same shape, and `expect_pending` serves the
	common case of a caller that never enables the mode.
- **Sockets declare their modes on the *associated* socket, unlike handles.** Superseded M10's exclusion of
	sockets (M12.2). The handle side sets modes before association because there the mode is part of an
	endpoint's provenance; a socket has no unassociated stage to hang that on, and inventing an
	`UnassociatedSocket` purely for symmetry would be churn for its own sake. Setting after association is
	still sound: the flag is inert until I/O time, so `AssociatedSocket::set_notification_modes` takes
	`&mut self` while `recv`/`send` keep taking `&self` -- a caller declares once and then submits freely.
	The asymmetry is real and deliberate, not an inconsistency to be tidied away.
- **The socket setter probes the provider rather than trusting the caller.** Win32 restricts socket
	skip-on-success to Layered Service Providers that return IFS handles, and a socket wrongly put in that
	mode reports `Pending` for an operation whose packet was suppressed -- rediscovering, on the socket side,
	exactly the rundown wedge M10.5 fixed for handles. Trusting the caller would re-open a bug already paid
	for once. The probe reads *this* socket's own `WSAPROTOCOL_INFOW` via
	`getsockopt(SOL_SOCKET, SO_PROTOCOL_INFOW)` and requires `XP1_IFS_HANDLES` in `dwServiceFlags1`, which is
	narrower and more accurate than the `WSAEnumProtocols` sweep the flag's own documentation suggests: it
	asks about the provider that actually created this socket, not about every LSP installed on the machine.
	Refusal is `io::ErrorKind::Unsupported`, deliberately not a Win32 error, because nothing failed -- the
	question was asked and answered. Only `skip_completion_port_on_success` is probed; `FILE_SKIP_SET_EVENT_ON_HANDLE`
	carries no such restriction.
- **The IFS decision is a separate function from the `getsockopt` that feeds it**, purely for testability.
	Every base Winsock provider on a stock Windows returns IFS handles, so the refusal arm is otherwise
	unreachable without installing an LSP -- `require_ifs_handles(flags)` takes the word and returns the
	answer, so both arms are covered by ordinary unit tests.
- **`classify_socket` became mode-aware in the same change that made the mode reachable**, exactly as
	`fs::classify_issued` and `device::classify_issued` did in M10.5. This was not a separable follow-up: a
	setter without it would ship the wedge, and the count it needs comes from `WSARecv`/`WSASend`'s
	`lpNumberOfBytesTransferred`, which the adapters had previously been passing as null.
- **`FILE_SKIP_SET_EVENT_ON_HANDLE` is unsafe to combine with the blocking backend**, which waits on precisely
	the handle event that flag suppresses. Recorded on the flag's own documentation rather than prevented,
	because the two are independently useful and the endpoint does not know its future backend.

## Caller-supplied owned buffers (M11)

- **Completion-based I/O forces owned buffers, not slices.** The kernel touches the caller's memory
	*after* the submitting call returns, so a `&[u8]` cannot describe an async operation: its borrow would
	have to span the whole operation, and nothing in the API can make it. The submission tokens have no
	`Drop` that cancels, and even one would be defeated by `mem::forget`, so a caller could always end the
	borrow with the kernel still reading. A cancel-on-drop that *blocked* would be sound and would also
	defeat the point of submitting asynchronously. The buffer is therefore handed over -- a protracted
	borrow made out of ownership rather than a lifetime -- and returned on completion through `claim` or
	`Started::Completed`.
- **The blocking adapters may still take slices, and do.** `BlockingEndpoint::read`/`write`/
	`read_scatter`/`write_gather`, `BlockingSocket::recv`/`send`, and the blocking `ioctl` all take plain
	borrows and allocate nothing, because they do not return until the operation is over: an ordinary
	borrow provably covers it. This is strictly cheaper than owning, so the two backends differ on purpose
	rather than for want of a shared shape.
- **`IoBuf`/`IoBufMut` are `unsafe` because the contract is a stable address.** A type whose accessor
	returns a fresh address on each call, or that reallocates while an operation is in flight, is what makes
	the kernel write into freed memory long after submission returned. That is unprovable by the compiler,
	so implementing the traits is an assertion. `Send + 'static` come along because the leaked operation
	storage is reclaimed on another thread through a thunk carrying no lifetime.
- **The traits are split so a shared buffer can be a source but never a destination.** An `Arc<[u8]>` is a
	fine thing to send *from* and can never be read *into*, since handing the kernel a writable pointer to
	bytes other clones are reading would alias; `&'static [u8]` is read-only for the same reason. One
	combined trait would have forced either excluding shared buffers from writes or admitting them as read
	targets.
- **Read buffers are fully initialized rather than init-tracked.** No `MaybeUninit`, no `set_init`-style
	obligation. A caller-supplied pooled buffer is initialized once and reused for the life of the pool, so
	the cost is per-pool rather than per-operation, and the API carries nothing for a caller to forget. The
	trade is that a fresh one-shot read buffer is zeroed before use.
- **No adapter allocates on the caller's behalf.** `read` takes the buffer to fill rather than a length,
	and `ioctl` takes an output buffer rather than an output length, so an allocation is always visible at
	the call site. A naive caller writing `vec![0; n]` there pays for it knowingly; a caller reusing a pool
	pays nothing. Hiding it would put a cost on the exact path the crate exists to keep cheap.
- **`scatter_gather_len` was removed with M11.** Both scatter paths now validate a `PageBuffers` that
	already exists, so there is no caller-supplied page count left to overflow-check; `PageBuffers::new` is
	where a degenerate count is rejected, at the caller's own call site.
- **`&'static mut [u8]` implements *both* traits, and is the one reference type that does (M12.1).** The
	split above is about aliasing, not about references: `Arc<[u8]>` and `&'static [u8]` are excluded from
	`IoBufMut` because they are *shared*, so handing the kernel a writable pointer would alias bytes another
	holder is reading. A `&'static mut` is exclusive by construction -- no other live reference to those bytes
	can exist -- so it is a legitimate read destination, and excluding it would have been the arbitrary half
	of the split rather than a safety measure. It is the natural handoff for a leaked or statically-allocated
	pool, and satisfies the stable-address promise trivially: the referent is `'static` and never moves, so
	moving the reference does not move the bytes.

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

### Registration ends at dequeue, not at reclamation

The registry answers one question: **is a completion packet still coming for this operation?** It stops being
true the moment the packet is dequeued, so that is where an operation is deregistered.

It did not start that way. Registration originally ended when the `Completion`'s storage was reclaimed --
dropped or claimed -- which conflated two different lifetimes: the port's obligation to deliver a packet, and
the completion's ownership of the operation's storage. A `Completion` holds neither a borrow of the port nor
any tie to it, so entirely safe code could dequeue one, hold it, and drop the port:

```text
let completion = port.get(INFINITE)?.unwrap();  // packet delivered; still registered
drop(endpoint);
drop(port);                                     // run_down() sees 1 outstanding
```

`Drop` runs `run_down`, which blocks in `get(INFINITE)` waiting for a packet that has **already been
delivered**. Reproduced: an unconditional hang, no timeout, no diagnostic beyond the outstanding-at-drop
warning. Dropping the completion from another thread would not help either -- it updates the registry but
nothing wakes `GetQueuedCompletionStatus`.

Moving deregistration to dequeue separates the two lifetimes cleanly:

- **Rundown** counts only undelivered packets, so a held completion cannot make the port wait for itself.
- **Cancellation** (`cancel_if_live`) now reports `NotFound` for an operation whose packet has been dequeued.
	That is more correct than before, not a regression: the operation is finished, and cancelling it was always
	meaningless.
- **Storage ownership** stays with the `Completion`, which still frees the box on drop and may now legitimately
	outlive the port it came from. `reclaim_from_overlapped` reads a thunk inside the operation's own
	allocation, so it needs nothing from the port -- which is why the completion no longer holds an
	`Arc<PortState>` at all.

Address reuse is not a hazard in the window this opens. The box is not freed until the completion is dropped,
so no later operation can be issued at that address while a stale identity might still be presented; and once
it is freed, a new operation there registers a fresh generation, which an `OperationId` compares.

This does change what `outstanding()` reports: a dequeued-but-held completion no longer counts. That is the
intended meaning -- the count measures what the port still owes the caller.

### Rundown waits are bounded and rechecked, not unbounded

`run_down` is `while outstanding() > 0 { get(...) }`. The wait inside the loop is **bounded**
(`RUN_DOWN_POLL_MS`) and the count is rechecked after it, rather than an unbounded `get`.

A `CompletionPort` is shareable and `get` takes `&self`, so another thread may consume completions concurrently
with a rundown. That opens a race an unbounded wait cannot survive: this loop can observe `outstanding() == 1`,
a concurrent consumer can then dequeue that last packet -- and clear its registry entry -- and only afterwards
does this loop call `get`. There is no packet left to deliver, so an unbounded `get` blocks forever. Clearing a
registry entry does not wake a `GetQueuedCompletionStatus` already in progress, so nothing rescues the wait; the
recheck must. A bounded wait wakes on its own after the interval, the loop re-reads `outstanding()`, sees zero,
and returns.

This is distinct from the [registration-ends-at-dequeue](#registration-ends-at-dequeue-not-at-reclamation) hang:
that one was a single thread waiting for a packet it had itself already dequeued; this one is one thread waiting
for a packet a *different* thread dequeued. Registration timing does not close it, because the count is correct
throughout -- the defect is purely that an unbounded wait cannot notice the count reaching zero by another path.
A wakeable count-to-wait transition would also work; a bounded recheck is chosen as the smaller mechanism, and
the interval only bounds recheck latency -- a packet genuinely destined for the rundown thread is still returned
the instant it arrives, so this is not a busy-poll of a live operation.

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

Rundown correctness rests on the live-operation registry: its length is the outstanding count that governs
draining, and each operation's `repr(C)` header carries a type-erased reclaim thunk that frees it from the
`OVERLAPPED` pointer alone. The registry was originally a lock-free `AtomicUsize`; it became a mutex-guarded map
when identities gained generations, because liveness has to be checked per identity and not merely counted (see
[the identity decision](#decision--identities-are-generation-stamped-and-validated-not-bare-addresses)).

Source tracking is a separate, opt-in diagnostic layer: when enabled it records each operation's
`#[track_caller]` submit `Location` in a per-port map, so the drop-time message can name the sources. It is a
one-time in-process setting (`set_source_tracking`) that defaults from the `WINDOWS_OVERLAPPED_IO_SYS_TRACK`
environment variable and is off by default. Its cost is now a second map insertion rather than the only lock on
the path. With tracking off, the drop message reports only the count and how to enable sources. The optional
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
	- `socket` → `Win32_Networking_WinSock` **and `Win32_Storage_FileSystem`** — overlapped socket operations,
		plus `SetFileCompletionNotificationModes` for `AssociatedSocket::set_notification_modes` (M12.2). The
		second of those is not a stray dependency: the notification-mode call lives in the file-system module
		even though its `FileHandle` parameter accepts a socket, because a socket handle *is* a kernel handle.
		This is the rule below applied, not an exception to it -- a family turns on what that family needs, and
		the socket family genuinely needs this. It does **not** make the `socket` feature imply the `fs`
		feature; the two select different sets of this crate's own adapters.
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
	not a compile-time guarantee. The Rust baseline is 1.98 (the MSRV) on edition 2024.

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

### Decision — identities are generation-stamped and validated, not bare addresses

An operation's storage address alone is not a durable name for it. Reclaiming an operation returns that address
to the allocator, which may hand it to a later operation, so an address retained past its operation's completion
can name a different, live operation. This was originally treated as a documentation matter -- "an identity is
unique only among simultaneously outstanding operations" -- but that was the wrong call, because it left a
**safe** function able to do something silently wrong: `cancel` acts purely on the address, so a stale identity
could cancel an unrelated operation. It was reproduced directly, recycling an address within 64 cancel-and-
resubmit cycles.

The severity comes from the triggering pattern being the *primary* use of cancellation -- a timeout firing while
a completion is in flight. A caller cannot atomically know whether the operation is still outstanding; that is
precisely what it needs the API to establish.

`OperationId` therefore carries a process-wide monotonic generation taken at submission, and each backend keeps
an `OperationRegistry` of live identities. Cancellation consults the registry and rejects an identity that is
not live under its own generation, with `ErrorKind::NotFound` and **without** calling `CancelIoEx` -- a recycled
address is never handed to the kernel on the caller's behalf. Retaining an identity indefinitely is now
harmless.

Consequences accepted:

- **The IOCP backend's lock-free submission path is gone.** `PortState` previously kept `outstanding` as an
	`AtomicUsize` specifically so the mutex was untouched on the hot path by default. The registry must be
	consulted on every submission and completion, so that property is deliberately reversed: correctness of a
	safe API outranks an uncontended-atomic-versus-mutex difference on the submission path. The registry replaces
	the counter rather than joining it (`outstanding` is now `live.len()`), so there is one lock rather than a
	lock plus an atomic, and the count and the liveness set cannot disagree.
- **The generation sequence is process-wide, not per-backend.** Per-object counters would let two objects mint
	the same (address, generation) pair, so an identity from one object could alias an operation in another. A
	single `AtomicU64` costs one relaxed increment and makes every identity unique for the life of the process;
	at one submission per nanosecond it still takes centuries to wrap.
- **`OperationId` is `Send + Sync`.** The raw pointer would otherwise make it neither, which would put
	cancelling from a different thread -- the entire point of holding an identity -- out of reach. The identity
	is inert data that no backend dereferences, so this is sound.

Alternatives rejected: storing the generation inside the operation header and validating by reading it, which
would require dereferencing storage that may already be freed; and exposing an `is_outstanding(id)` predicate,
which invites a time-of-check/time-of-use race when `cancel`'s return value already answers the same question
atomically.

#### Duplicate registration panics deliberately

`OperationRegistry::insert` panics when an address is registered while an earlier operation is still registered
at it, rather than overwriting or ignoring the duplicate. The registry's whole purpose is to answer "does this
identity still name a live operation?", and two live operations sharing one entry makes that answer wrong for
one of them -- reintroducing precisely the mis-cancellation the generations were added to prevent. A silent
duplicate would therefore convert a loud, immediate failure into a rare wrong-operation cancellation, which is
the harder bug by a wide margin.

The panic carries the colliding address and *both* generations, and states that the fault is in the completion
backend rather than its caller, because that is the only audience able to act on it: the condition is
unreachable through ordinary use of either backend, since a submitted operation owns freshly boxed storage. The
invariant it guards -- **an address is never registered while it is available for reuse** -- is stated on the
type, and the callback-ordering trap that violates it is called out there and in
[the workspace design notes](../../DESIGN-NOTES.md). This is not hypothetical: the assertion caught a real race
in the `TP_IO` backend, where deregistering after the callback left a window because `IoCompletion::claim` frees
the storage inside the callback.

`OperationId`, `Issued`, and `Submitted` are part of that seam rather than IOCP-private types: an operation
identity, a submission classification, and a submission outcome mean the same thing to any backend. Building
`TP_IO` outside this crate showed that only `OperationId` was not actually reachable -- it had no public
constructor, so a backend in another crate could read an identity but never produce one. `OperationId::mint`
closes that gap: it takes the operation's storage address and pairs it with the next value of a process-wide
monotonic counter, which is what makes the identity unique rather than merely descriptive.

`mint` is safe because an `OperationId` is only an address plus a counter -- the address is what
`OperationId::as_ptr` already exposes, and the type is documented as never to be dereferenced or freed. The
generation is what an address alone could not provide: storage is reclaimed and reissued, so an identity minted
from an address alone could silently name a later operation. That failure was reproduced before the redesign, at
cycle 21 of a reclaim loop.

**Minting is the only way safe code can obtain an identity.** There is deliberately no safe constructor that
pairs an address with a generation of the caller's choosing. An earlier `from_parts` did exactly that, and its
documentation claimed that "reconstructing an identity confers no access that observing it did not already
confer" -- which was false. A caller holding `(p, g)` could construct `(p, g + 1)`, and if the next submission
reusing `p` were stamped with that generation, `cancel` would accept the forged identity and abort an operation
the caller never submitted. The generation defeats a *retained* identity; nothing stopped a *guessed* one. That
is an isolation break rather than undefined behaviour -- cancelling a live operation is well-defined, tokens are
not forgeable, and no storage can be reclaimed twice by it -- but it is precisely the property generations exist
to provide.

The fix removed the pairing step from the normal path rather than guarding it. Every caller of `from_parts` was
reassembling what the registry had just returned, so `OperationRegistry::remove` and `OperationRegistry::identify`
return a whole `OperationId`, built from the pair the registry itself recorded. A backend recovering the identity
of a completion it knows only by address asks the registry for it and never supplies a generation, so it cannot
supply a wrong one.

`unsafe fn forge` remains, for the tests that prove a stale or ahead identity is *rejected*: that coverage has to
be reachable from the sibling crate, where a `pub(crate)` seam would not be. Marking it `unsafe` for an invariant
that is not memory safety is a deliberate choice -- the obligation is the kind `unsafe` exists to record, and the
alternative was losing those regression tests or leaving safe code able to forge. The `compile_fail` doctest
proving safe code cannot forge is paired with a positive control differing only in the `unsafe` block, so the
rejection is demonstrably the missing obligation rather than any other compile error.

The alternative -- letting the thread-pool crate define its own identity type -- was rejected: it would fork the
shared vocabulary the two crates exist to hold in common, and downstream code composing the backends would have
to translate between two identical types.

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

### Decision — per-family safe adapters, file family first

The Open boundary is resolved for the **file family** by choosing the operation-specific safe-adapter path, in
this crate behind the `fs` feature, rather than a fully generic safe submission API. The generic problem stays
open, and the narrow unsafe `submit` / `run` seam stays available for families that do not yet have an adapter.
The shape below is the template every later family (socket, scatter/gather, `DeviceIoControl`) follows.

- **Blocking backend** -- `BlockingEndpoint::read` / `write` are fully safe and synchronous. The adapter owns the
	buffer, issues the single `ReadFile` / `WriteFile` internally, waits with `GetOverlappedResult`, and returns
	`io::Result<(Vec<u8>, usize)>` (read) or `io::Result<usize>` (write). No `unsafe`, no `OVERLAPPED`, and no
	completion ceremony reaches the caller, because the whole operation finishes within the one call.
- **IOCP backend** -- `AssociatedEndpoint::read` / `write` are safe on the submission side. The adapter owns the
	buffer in the operation payload, recovers the buffer pointer from the pinned `OVERLAPPED` through a
	`pub(crate)` payload-offset primitive (the same offset trick as the reclaim thunk), issues the native call,
	and returns a typed `FileIo` token instead of a bare `OperationId`. The token carries the payload type as a
	witness, so its `claim(&Completion)` is safe: it verifies the completion's `OVERLAPPED` identity matches the
	token and only then performs the typed reclamation the witness justifies, handing back the buffer and byte
	count. The async claim-time type erasure that keeps the *generic* boundary open is discharged here, for this
	one family, by the token witness plus the identity check -- not by a general solution.

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
