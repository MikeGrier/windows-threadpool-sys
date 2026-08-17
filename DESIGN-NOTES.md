# Design notes

This is the Windows Threadpool System crate. (windows-threadpool-sys).

It provides a Rust memory safe API over the Windows operating system's threadpool APIs. The Windows threadpool
is uniquely valuable in that it gives the ability to interact with the Windows operating system's
blocking facilities (in general) without the application dedicating any threads to the waits.

Many mechanisms have been added to Windows over the decades to allow for aggregation of waits
(WaitForMultipleObjectsEx, IO Completion Ports), but these all have limits and all end up requiring the
application to multiplex threads to deal with those limits.

The Windows thread pool works with the Windows kernel to, when no work is scheduled, have no extra threads
allocated towards the work at all. This is a unique value of it, and allows services on Windows to use the
thread pool and quiesce down to extremely low cost, low power states.

Current (August 2026) Rust work dispatch systems are uniform with Linux with typically means having one thread
created permanently per available processor per "reactor", and on Windows, with component boundaries being
DLLs not processes, this can lead to processes having (n * P) idle threads' stacks consuming memory for no
good reason, where 'n' is the number of reactor instances in the process and 'P' is the number of processors
on the machine.

This crate does not attempt to solve this problem, but it does provide a useful building block for code that
wants to avoid contributing to it. The Windows threadpool types are inherently memory unsafe and leave many
choices up to the developer. The `windows-sys` crate published by Microsoft helps with the basics of the FFI
to the APIs, but does little to help turn the alphabet and phrasebook into a useful programming model.

## Windows SDK model and constraints

This crate targets the object-based thread pool API (introduced in Windows Vista) rather than the legacy
`QueueUserWorkItem`, timer queue, or registered-wait APIs. The modern API has separate pool, cleanup group,
work, timer, wait, and I/O objects. A callback environment selects the pool, cleanup group, callback priority,
long-running behavior, and related callback settings when a callback-generating object is created.

The SDK contracts impose several requirements that the safe API must represent:

- Closing cleanup group members blocks until executing callbacks finish and either waits for or cancels
	callbacks that have not started. It also releases the member objects, which must not be used or closed
	individually afterward. Creating new group members must be synchronized with group cleanup.
- Canceling queued callbacks does not stop callbacks that have started. For thread pool I/O, it also does not
	cancel the underlying I/O request or make its `OVERLAPPED` storage safe to free.
- A waitable handle must remain valid while its wait is pending. A wait must be explicitly rearmed for each
	activation, and passing a mutex handle is unsupported.
- Disarming a timer or wait prevents new callbacks from being queued but does not retract callbacks already
	queued. Relative times exclude sleep and hibernation; absolute times include them.
- `StartThreadpoolIo` must precede every overlapped operation. A failed operation, or an immediate success on
	a handle using `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS`, must be balanced with `CancelThreadpoolIo`.
- Callback code runs on shared, process-managed threads. It must restore thread-local state before returning,
	must not terminate the thread, and must not unwind across the FFI boundary.
- Teardown must prevent new submissions, disarm callback sources, account for outstanding I/O, wait for
	executing callbacks, and only then release callback context and dependent native resources.

The minimum supported Windows version is the current publicly-supported Windows baseline that the project's
GitHub CI validates: Windows Server 2025 (the `windows-latest` hosted runner) and, equivalently, Windows 11 on
the client. The crate does not pursue down-level support below that baseline. Every capability this crate uses
-- the object-based thread pool, callback priorities, `SetThreadpoolTimerEx`, `SetThreadpoolWaitEx`,
`CancelIoEx`, and `GetQueuedCompletionStatusEx` -- is available there, so their historical Vista / 7 / 8
introduction points do not require version gating in the public API. The toolchain baseline is Rust 1.97 (the
MSRV) on edition 2024.

## Shared invariants (both crates)

Both crates uphold one contract for ownership, cancellation, and callback lifetime, so a resource created in one
and driven through the other never has ambiguous rules. The per-crate mechanics live in
[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) (overlapped
storage, IOCP and blocking backends) and in the sections of this document (thread-pool objects); the invariants
below are the common surface both must satisfy.

### Ownership

- Every native resource -- handle, socket, operation storage, and thread-pool object -- has exactly one typed
	owner whose destructor is the documented native destructor. Reuse `std::os::windows::io::OwnedHandle` /
	`OwnedSocket` where that destructor is `CloseHandle` / `closesocket`; add a typed owner only for a specialized
	destructor. There is no untyped universal handle owner.
- An operation's pinned storage is transferred out of Rust's control on submission -- to the kernel, or to the
	thread pool -- and is reclaimed only after that operation's completion has been observed. Its stable
	`OVERLAPPED` address is its identity; the storage is never moved, reused, freed, or aliased while the
	operation is outstanding.
- Associating an endpoint with a completion backend is a one-time, consuming transition; an associated endpoint
	exposes neither cloning nor an unrestricted raw-handle escape hatch.

### Cancellation

- Cancellation is a request, not a completion. `CancelIoEx` (overlapped I/O) and `CancelThreadpoolIo`
	(thread-pool accounting) neither stop an operation that has started nor make its storage safe to free.
- The matching completion -- delivered even for a cancelled operation, typically as `ERROR_OPERATION_ABORTED`
	-- remains the sole trigger that reclaims the operation's storage. Targeted and whole-endpoint cancellation
	both leave that completion path intact.
- Targeted cancellation names an operation by an `OperationId`, which pairs the storage address with a
	process-wide generation taken at submission. Both crates validate the identity against their live-operation
	registry before issuing a native cancel, so an identity retained past its operation's completion is rejected
	rather than applied to whatever operation has since been given that address. Cancelling therefore races
	safely against completion in both crates: a late cancel fails, it never hits an unrelated operation.

### Callback lifetime and teardown

- Callbacks run on shared, process-managed threads. A callback must not unwind across the FFI boundary, must not
	terminate its thread, must restore any thread-local or thread state it changed before returning, and must not
	block on downstream consumer progress.
- Teardown, in both crates, follows one order: prevent new submissions, disarm or cancel outstanding work,
	account for and drain outstanding completions, wait for executing callbacks to finish, and only then release
	the callback context and the dependent native resources.
- Voluntary rundown blocks with these semantics, and `Drop` blocks the same way to stay memory-safe: it never
	frees storage the kernel or pool may still touch, and it never panics. See
	[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) for the
	overlapped-side realization.

## Downstream directory notification evaluation scenario

A separate future directory notification facility is an evaluation scenario for deciding how fully this crate
should build out its lower layers; it is not part of this crate's API or implementation plan. That facility is
expected to add and remove watched paths dynamically, watch subtrees recursively, use
`ReadDirectoryChangesExW` where available, fall back to `FindFirstChangeNotification` when necessary, retain
multiple notification buffers, and deliver submissions and completions through SQ/CQ-style rings.

The capabilities exert different kinds of design pressure:

- Dynamic addition and removal tests whether individual I/O and wait registrations have independent ownership,
	rundown, and failure isolation. It does not imply that this crate should own a path registry.
- Recursive watching is primarily directory-domain policy. It does not by itself require another thread pool
	abstraction, although multiple recursive roots reinforce the need for independently removable registrations.
- The `ReadDirectoryChangesExW` path tests stable `OVERLAPPED` and payload ownership, exact operation identity,
	balanced `StartThreadpoolIo` accounting, prompt rearming, and cancellation races. These concerns argue for
	reusable thread pool I/O operation machinery, but do not decide whether that machinery can be both fully
	generic and entirely safe.
- The `FindFirstChangeNotification` fallback tests composition with `TP_WAIT`, including safe disarm, rearm,
	callback drain, and notification-handle close ordering. Its specialized open, rearm, and close operations
	remain concerns of the directory notification facility rather than general file I/O in this crate.
- SQ/CQ delivery tests whether callbacks can feed a caller-selected bounded ring without allocation, copying,
	blocking a process thread pool worker, invoking arbitrary application code, or imposing this crate's choice of
	channel or executor. Ring layout, capacity, overflow policy, and buffer recycling remain downstream decisions.

Taken together, the scenario is evidence for building registration-level `TP_IO` and `TP_WAIT` ownership,
operation identity, callback dispatch, and object-local rundown more fully than a single private directory
adapter would require. It is not evidence for a general safe file I/O layer covering file creation, reads,
writes, paths, parsing, or filesystem policy.

The directory scenario establishes that an owned overlapped-I/O foundation must exist before this crate exposes
thread pool I/O. That foundation now lives in the sibling crate `windows-overlapped-io-sys`, which is versioned
and published independently. Its requirements -- typed endpoint ownership, provenance-controlled association,
pinned per-request `OVERLAPPED` storage, completion identity, the uniform result model, cancellation, and
rundown across raw IOCP and event backends -- are specified in
`crates/windows-overlapped-io-sys/DESIGN-NOTES.md` and are deliberately rounded out to cover the full range of
Windows overlapped operations, not just directory notification. Operation payloads stay opaque there:
directory-notification names may be arbitrary 16-bit units that a higher layer validates without the foundation
decoding or copying them.

This crate consumes that foundation to implement thread pool I/O. `TP_IO` reuses the shared endpoint and
operation storage but is a distinct completion backend: it owns an opaque system-managed IOCP association and
additionally requires every `StartThreadpoolIo` to be balanced by a callback or `CancelThreadpoolIo`. The
sibling crate defines the backend seam; this crate implements that seam and its accounting, and never exposes
the thread pool's internal completion port. The dependency is one-directional: `windows-overlapped-io-sys` must
not depend on `windows-threadpool-sys` or reference any `TP_*` type.

Generic overlapped submission remains the hardest public boundary for the pair. A safe API cannot accept an
arbitrary raw handle, `OVERLAPPED` pointer, payload, and caller-reported submission result while proving that
exactly one operation was issued and that a completion will or will not arrive. A constrained owned-operation
prototype must demonstrate balanced immediate failure, immediate success, pending completion, cancellation, and
destruction without trusting caller-maintained state before any public thread pool I/O API is committed.

## `windows-sys` binding boundary

The initial dependency is `windows-sys` 0.61.2 with default features disabled and
`Win32_System_Threading` plus `Win32_System_IO` enabled. They transitively enable `Win32_System`, `Win32`, and
`Win32_Foundation`. The threading feature is sufficient for the core pool, cleanup group, work, timer, wait,
and thread pool I/O exports. The `Win32_System_IO` feature covers the `OVERLAPPED` interplay at the thread pool
I/O seam; the reusable overlapped-I/O foundation itself now lives in the sibling crate
`windows-overlapped-io-sys`, which owns its own `windows-sys` feature layout.

The bindings expose the kernel32 functions, opaque `PTP_*` values, `unsafe extern "system"` callback types,
`TP_CALLBACK_ENVIRON_V3`, callback priorities, and `TP_POOL_STACK_INFORMATION`. They remain raw bindings:
the `PTP_*` values are represented as `isize`, callback contexts are untyped pointers, and the thread pool I/O
callback exposes its `OVERLAPPED` argument as `*mut c_void`.

The reusable overlapped machinery -- `OVERLAPPED`, `OVERLAPPED_ENTRY`, the cancellation and result functions,
and the raw IOCP create, post, and dequeue functions, all under `Win32_System_IO` -- is specified and owned by
`windows-overlapped-io-sys`; see `crates/windows-overlapped-io-sys/DESIGN-NOTES.md` for its feature layout.
`SetFileCompletionNotificationModes` is under `Win32_Storage_FileSystem` and remains an operation- or
endpoint-specific dependency there rather than part of this crate's core feature set.

The SDK's callback-environment functions are header-only inline helpers and are therefore not emitted by
`windows-sys`. This includes `InitializeThreadpoolEnvironment`, `DestroyThreadpoolEnvironment`,
`SetThreadpoolCallbackPool`, `SetThreadpoolCallbackCleanupGroup`, `SetThreadpoolCallbackPriority`, and
`SetThreadpoolCallbackRunsLong`, as well as `SetThreadpoolCallbackLibrary`. The crate will need narrow
equivalents over `TP_CALLBACK_ENVIRON_V3`.
`TP_CALLBACK_ENVIRON_V3::default()` is not a substitute for SDK initialization: the SDK sets version 3,
normal callback priority, and the structure size in addition to clearing the remaining fields. The current SDK
destroy helper is a no-op, but the wrapper should still model that lifecycle boundary.

`LeaveCriticalSectionWhenCallbackReturns` additionally requires the `Win32_System_Kernel` feature because its
`CRITICAL_SECTION` parameter is feature-gated. That feature is intentionally deferred until the safe API has a
use for this specialized callback-return operation.

## Cleanup groups own their members

`CloseThreadpoolCleanupGroupMembers` releases every member at once, and afterwards a member must not be used or
closed again. Two things follow that an "individually owned object, flagged as group-owned" design cannot
satisfy. First, each object also owns a heap callback context, and that context is only safe to free once the
group has finished releasing members -- which is precisely the moment an individual object cannot observe.
Second, nothing would stop a caller using a member after the release.

So `CleanupGroup` creates its members (`create_work`, `create_timer`, `create_periodic_timer`, `create_wait`)
and owns both the member objects and their contexts, plus the watched handle of a wait member. Members are
handle wrappers with no `Drop`; the group frees everything after the bulk release. Use-after-release is a
compile error rather than a documented rule: members borrow the group and `close_members` takes `&mut self`, so
the borrow checker rejects touching a member afterwards. A `compile_fail` doc test pins that.

Two consequences worth recording:

- **Thread-pool I/O is excluded on purpose.** A `TP_IO` object must not be closed while an overlapped operation
	is outstanding, because the kernel still owns that operation's storage, and a bulk release has no way to
	satisfy that precondition. `ThreadpoolIo` therefore stays individually owned, where its `Drop` cancels,
	drains, and only then closes. Adding a `create_io` would trade a guarantee for a convenience.
- **The caller's environment is copied, not mutated.** Layering the group onto a caller-supplied
	`CallbackEnviron` in place would be visible to them afterwards, and would break reusing one environment
	across two groups. Copying costs one struct move per member creation.

Per-object `Drop` was already correct teardown for every type, so a cleanup group buys bulk convenience rather
than a safety property the crate lacked. It was still worth building safely rather than leaving
`set_cleanup_group` as the only route, because that route is `unsafe` and easy to get wrong in exactly the way
described above.

## `TP_IO` backend realization

`ThreadpoolIo` is the third completion backend for the shared overlapped model, alongside the raw IOCP and
blocking backends owned by `windows-overlapped-io-sys`. It reuses that crate's endpoint ownership and pinned
operation storage unchanged and adds only the two concerns the thread pool imposes.

### Balanced accounting is the type's central invariant

One counter -- the number of `StartThreadpoolIo` calls not yet balanced -- serves as both the pool's accounting
and the crate's rundown state, because they are the same quantity: an unbalanced start is exactly an operation
whose storage the kernel or pool still owns. `submit` increments before issuing `StartThreadpoolIo`, so a
completion delivered on a pool thread can never race ahead of the count. It is then balanced on exactly one of
three paths: the I/O callback (pending), or `CancelThreadpoolIo` inline in `submit` for an immediate failure or
for a synchronous completion on a handle in `FILE_SKIP_COMPLETION_PORT_ON_SUCCESS` mode. The unsafe contract on
`submit` exists to make the caller's `Issued` classification the single point where this can go wrong.

The accounting is the shared `OperationRegistry` from `windows-overlapped-io-sys`: its length is the number of
unbalanced starts, and its condition variable is what rundown blocks on. Using the shared type rather than a
private counter means the two backends cannot drift apart on what "outstanding" means, and it gives `TP_IO` the
identity validation described under [Cancellation](#cancellation) for free. Rundown must block on a condition
variable rather than pump a dequeue loop as the IOCP backend does, because these completions arrive on pool
threads the owner does not drive.

### Callback ordering: deregister on entry, and what that costs `run_down`

The governing invariant is that **an address is never registered while it is available for reuse**. Violating it
is a real race, not a theoretical one: the registry's duplicate-address assertion caught it during
multi-threaded testing, where one thread's completion freed storage that another thread's submission was
immediately handed while the freed operation's entry was still present.

Satisfying it requires deregistering the operation **before any user code runs**, at the top of the callback.
Deregistering afterwards is not sufficient no matter how the guards are ordered, because `IoCompletion::claim`
hands the storage to the callback, which may drop it -- freeing the address -- part-way through its own body.
The kernel is finished with the operation by the time its callback is entered, so there is nothing to lose by
deregistering there, and it makes `cancel` correctly reject an operation whose completion is already being
delivered. Doing it unconditionally before `catch_unwind` also keeps the accounting exact when a callback
panics, which is why no drop guard is needed for it.

The cost is that an empty registry no longer implies the callbacks have finished, so `run_down` waits for two
things: first that no operation is outstanding, then -- via `WaitForThreadpoolIoCallbacks` -- that no callback
is still executing. Without the second wait, `cancel_all(); run_down(); read results` is a race, which is
exactly how the weaker version was caught. Keeping both inside `run_down` means callers get the contract they
would assume ("when this returns, my callbacks have run") rather than the one that happens to fall out of the
implementation.

An earlier version of this backend deregistered at the *end* of the callback, reasoning that this made
"outstanding reached zero" mean "no storage is still pool-owned". That was both unsafe (the race above) and
unnecessary: the property that matters at teardown -- no callback still touching the context -- comes from
`WaitForThreadpoolIoCallbacks`, not from the counter. The raw IOCP backend already deregistered before releasing
storage in both its claim and drop paths, so this also brings the two backends into agreement.

There is deliberately no cancelling variant of `wait`: cancelling a pending I/O callback neither cancels the
underlying operation nor makes its `OVERLAPPED` safe to free, so the only sound way to stop outstanding I/O is
`cancel_all` followed by `run_down`.

### `Drop` cancels rather than only blocking

The raw IOCP `CompletionPort::drop` blocks on a drain that the caller must already have made terminating. A
`ThreadpoolIo` owns its endpoint handle, so it can do better: `Drop` reports the skipped rundown once, then
issues `cancel_all` itself, which guarantees every outstanding operation delivers its callback and therefore
that the block terminates. Cancelling a handle that is about to be closed anyway costs nothing and converts a
potential permanent hang into a bounded wait.

### Seam change this required

`OperationId` had no public constructor, so only backends inside `windows-overlapped-io-sys` could name their
in-flight operations. Implementing the second backend outside that crate revealed the gap, and it was fixed at
the source rather than by defining a competing identity type here -- a duplicate would have split the shared
vocabulary the two crates are supposed to hold in common.

Building the second backend then exposed a deeper problem in the same seam: an identity was only an address, so
one retained past its operation's completion could name a later operation that had been given the same storage.
`OperationId::mint` / `from_parts` and the shared `OperationRegistry` replaced the original address-only
constructor; see
[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) for that
decision, which this crate consumes rather than duplicates.

Primary references:

- [Thread Pools](https://learn.microsoft.com/windows/win32/procthread/thread-pools)
- [Thread Pool API](https://learn.microsoft.com/windows/win32/procthread/thread-pool-api)
- [Using the Thread Pool Functions](https://learn.microsoft.com/windows/win32/procthread/using-the-thread-pool-functions)
- [`threadpoolapiset.h` API index](https://learn.microsoft.com/windows/win32/api/threadpoolapiset/)
- [`ReadDirectoryChangesExW`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-readdirectorychangesexw)
- [`CancelIoEx`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)
- [`windows-sys::Win32::System::Threading`](https://docs.rs/windows-sys/0.61.2/windows_sys/Win32/System/Threading/)
