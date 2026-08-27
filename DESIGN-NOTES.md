# Design notes

## Founding theme

This repository exists to enable asynchronous Windows code in Rust that uses native
Windows APIs and the Windows thread pool while imposing zero thread overhead when idle.
It favors Windows-native fidelity and composition over a lowest-common-denominator
reactor model. The repository is called `windows-threadpool-sys` because it began with
the memory-safe Windows thread-pool facade documented here; the other crates extend the
same premise to adjacent Windows asynchronous facilities.

These crates complement the Microsoft `windows` and `windows-sys` crates rather than
compete with them. Public APIs use or re-export their corresponding native Windows types
when those types faithfully express the contract. A crate-owned parallel type requires
an additional invariant or semantic distinction that the native type cannot express;
surface-level convenience alone is not sufficient.

This is the Windows Threadpool System crate (`windows-threadpool-sys`).

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
	must not terminate the thread, and must not unwind across the FFI boundary -- an escaping unwind aborts the
	process, and nothing contains it.
- Teardown must prevent new submissions, disarm callback sources, account for outstanding I/O, wait for
	executing callbacks, and only then release callback context and dependent native resources.

The minimum supported Windows version is the current publicly-supported Windows baseline that the project's
GitHub CI validates: Windows Server 2025 (the `windows-latest` hosted runner) and, equivalently, Windows 11 on
the client. The crate does not pursue down-level support below that baseline. Every capability this crate uses
-- the object-based thread pool, callback priorities, `SetThreadpoolTimerEx`, `SetThreadpoolWaitEx`,
`CancelIoEx`, and `GetQueuedCompletionStatusEx` -- is available there, so their historical Vista / 7 / 8
introduction points do not require version gating in the public API. The toolchain baseline is Rust 1.98 (the
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
	block on downstream consumer progress. The trampolines do **not** contain a panic: an unwind reaching an
	`extern "system"` frame aborts the process. This is deliberate -- see
	[the panic-containment decision](#a-panicking-callback-aborts-rather-than-being-contained).
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
delivered.

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
`OperationId::mint` and the shared `OperationRegistry` replaced the original address-only constructor, and the
registry hands a backend the whole identity for an address rather than a generation to re-pair, so safe code
cannot assemble one at all. See
[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) for that
decision, which this crate consumes rather than duplicates.

## A safe wait constructor takes proven wait provenance, not any handle

The pool's wait objects support only some kinds of handle. A mutex handle in particular is documented by the
SDK as unsupported, and passing one makes the native behaviour undefined -- there is no error return.

`ThreadpoolWait::new` is a safe function, so it cannot delegate that precondition to its caller by documenting
it: safe code that can invoke undefined behaviour is unsound however clearly the requirement is written down.
Both wait constructors therefore take a `WaitableHandle` rather than an `OwnedHandle`. The type mirrors the
shape `UnassociatedEndpoint` already uses in the sibling crate: safe constructors for the handle kinds this
crate can create itself, plus one narrow `unsafe fn assume_waitable` for handles obtained elsewhere, where the
caller takes on the obligation explicitly.

The cleanup group's `create_wait` takes the same type. It is a second safe path to the identical hazard, and
changing only the individually-owned constructor would have left it unsound -- the reason the constraint lives
in a type rather than in each function's documentation.

## A wait target owns its close routine, because not every waitable handle closes with `CloseHandle`

`WaitableHandle` originally wrapped a std `OwnedHandle`, which encodes one specific destructor:
`CloseHandle`. That is right for an event, and wrong for a whole class of waitable handles that Win32
hands out with a bespoke release function. The motivating case is
`FindFirstChangeNotification`, whose result is waitable but must be released with
`FindCloseChangeNotification`; `CloseHandle` on one is not a leak we can shrug at but a documented
misuse.

So the handle a wait watches is now a `WaitTarget`: either the default `Owned(OwnedHandle)` path, or
`Custom`, which carries the raw handle alongside the caller-supplied
`unsafe extern "system" fn(HANDLE) -> BOOL` that releases it. `WaitableHandle::assume_waitable_with`
is the narrow `unsafe` seam that builds one, next to the existing `assume_waitable`, and for the same
reason: the caller is asserting both that the handle is a supported wait target and that the routine
they passed is the correct release for it.

Three properties fall out of putting the destructor in the target rather than in the wait object.

**The close cannot be reordered before the drain.** Both teardown paths already drain first -- an
individually-owned `ThreadpoolWait::drop` cancels and waits for callbacks before its fields drop, and
`CleanupGroup::release_members` runs `CloseThreadpoolCleanupGroupMembers` before it frees adopted
resources. Because the close is a *destructor* on a value those paths already own, it inherits that
ordering instead of restating it. Nothing had to be added to either path to keep the handle alive
until the pool stopped watching it.

**Both paths get it for free.** The group adopts the target as a boxed `WaitTarget` freed by
`free_boxed::<WaitTarget>`, not as a boxed `OwnedHandle` freed with `CloseHandle`. That is one
substitution at the adoption site, and it is what stops the group being a second path to the wrong
destructor -- the same argument that put wait provenance in a type above.

**`Drop` lives on an inner struct, not on the enum.** `WaitTarget` deliberately has no `Drop` impl of
its own; the custom arm's field is a small `CustomClose` that carries the destructor. An enum with a
destructor cannot be destructured by an ordinary `match`, which would have forced `into_handle` to
resort to `ptr::read` and an `unreachable!`. Pushing the destructor one level down keeps the enum
freely movable while the handle is still released exactly once.

### `into_handle` now declines rather than lying

Handing an `OwnedHandle` back out of a custom-close target would hand out the wrong destructor: the
receiver would eventually close a `FindFirstChangeNotification` handle with `CloseHandle`. There is
no correct `OwnedHandle` to give, so `WaitableHandle::into_handle` returns
`Result<OwnedHandle, Self>` -- `Ok` for the default path, and for a custom target `Err(self)`, giving
the wrapper back intact so the caller still owns a handle that closes correctly.

This is a breaking change to a published signature, taken rather than the alternatives of panicking
(a safe function that aborts on a legal input) or closing with the wrong routine (silent misuse). The
crate's own internal caller wanted the whole target rather than a bare handle anyway, and now uses
`into_target`.

### Testing it needs per-test statics, not one global counter

A close routine is a bare `extern "system"` function pointer, so it cannot capture: everything it
records has to live in a `static`. The tests therefore declare their observation state *inside* each
test function rather than at module scope. Under `cargo nextest` that would not matter -- each test
gets its own process -- but CI also runs `cargo test`, which runs tests as threads in one process,
where a shared counter would be corrupted by any other test closing a handle concurrently.

The integration tests also check the drain ordering *per handle* rather than globally. With hundreds
of waits tearing down together, "some handle is closed" says nothing while a callback runs; the bug
is a callback finding *its own* handle already closed, so each callback captures its raw handle value
and looks for that. Each ordering test additionally asserts that teardown actually blocked, which is
what stops it passing vacuously when a callback happens to finish before teardown begins.

## Re-arming is gated against teardown, under the same lock that arms

Both the timer and the wait let a callback re-arm the object from inside itself, which is what the SDK requires
for repeated activation. Neither was synchronized with `Drop`.

`Drop` disarms, drains callbacks, then closes the object and frees the context. A callback already running when
the disarm happens can arm the object again afterwards; the drain then returns -- it only waits for callbacks,
not for the object to be idle -- and the close and context free race a freshly queued callback.

Each context now holds a `shutting_down` flag. Arming takes that lock and does nothing when the flag is set;
`Drop` sets the flag and disarms under one acquisition of it. A re-arm request is therefore either applied
before teardown begins or suppressed, never interleaved. The lock is only ever held across the native setter,
never across the drain: a callback blocked on it would otherwise never finish, and the draining thread would
wait on it forever.

The timer half of this was a window the previous review round *introduced*. Deferring the re-arm to after the
callback returns -- which is what makes the requested delay run from the end of the firing, keeping successive
firings sequential -- moved the arming past `Drop`'s disarm, where it had previously been inside the callback.

Suppression has no user-visible effect by construction: it only ever happens while the object is being
destroyed, so no caller can observe the difference, and the absence of undefined behaviour is not testable
directly. Both objects therefore expose the outcome to their own tests -- `rearm_reporting` on the wait, and a
test-only observer on the timer, whose `Arc` the test keeps so the record outlives the freed context. Without
these, a test could only assert that teardown terminated, which it does with the gating removed as well.

## Quiescing without dropping is `stop_and_drain`, and it covers the callback only

`ThreadpoolTimer` and `ThreadpoolWait` both let a callback re-arm from inside itself, so "stop this and wait
until it is idle" is not expressible as disarm-plus-drain. `wait()` demonstrably does not do it: after
`disarm(); wait();` a self-re-arming timer was measured still set and firing, because the deferred re-arm is
applied after the callback returns, which is after the external disarm.

`cancel_pending()` *was* measured to leave the object quiescent -- but only because the pool drops a callback
armed by the trampoline during an in-flight cancel. No SDK contract promises that, so relying on it would make
our quiescence guarantee a property of the dependency rather than of this crate. Both types therefore have a
`stop_and_drain` that suppresses re-arming under the same lock `Drop` uses, disarms, drains, and lifts the
suppression so the object stays usable. `ThreadpoolPeriodicTimer` already had one, so this also removes an
asymmetry between the three timer-shaped types.

The suppression is a **depth count**, not a flag. It has two users with different lifetimes: `stop_and_drain`
raises and lowers it, while `Drop` raises it permanently. With a flag, one `stop_and_drain` finishing would
clear a suppression a concurrent one still needed.

Suppression is not observable from outside -- dropping queued callbacks leaves the object idle either way -- so
the regression test asserts the *mechanism*, checking that the in-flight callback's re-arm request was actually
refused. A first version asserted quiescence instead and passed with the suppression removed.

### The suppression covers the callback's re-arm, not an external one

A later review round pointed out the limit of all this: the external arming methods -- `set_after`, `set_at`,
`set_after_with_window`, `arm`, `start*` -- take `&self` on `Sync` types and do **not** pass through the
suppression lock. A concurrent arm from another thread landing inside the stop window is therefore not excluded
by anything in this crate, and the original documentation claimed flatly that the object was idle on return.

No observable failure could be produced, and the reason is instructive: `WaitForThreadpoolTimerCallbacks` with
cancellation clears a due time *even when no callback is queued* (measured: `is_set` true then false), whereas
`wait()` leaves it set. The drain therefore cancels a racing arm incidentally -- which is the same undocumented
behaviour this decision opens by refusing to depend on. The guarantee is real today and is not ours.

Two ways to make it ours were considered: route external arming through the same lock, or require exclusive
access for `stop_and_drain` as `BlockingEndpoint` does. **Neither was taken.** The gate costs a mutex on every
arm and, for the cleanup-group members, plumbing a context pointer into types that currently hold only a raw
handle; exclusive access would remove the ability to stop an `Arc`-shared timer from another thread, which is a
legitimate pattern the type otherwise supports. Against a race with no demonstrated failure, both were judged to
buy less than they cost.

What was fixed is the claim. Each `stop_and_drain` now separates what it enforces -- no callback queued or
executing, and a callback's own re-arm discarded -- from what it assumes: that nothing else arms the object for
the duration, which a caller obtains by owning it exclusively or serializing access. The measurement is recorded
alongside so the incidental cancellation is not mistaken for a contract and quietly relied upon later.

This is the honest shape of the decision: a documented precondition is a weaker thing than an enforced one, and
saying so is better than either overclaiming or paying for machinery nobody needs.

## The encoding check rejects stray control characters

A form feed reached two committed source comments. The cause was a PowerShell replacement containing
`` `forge` `` in a double-quoted string: PowerShell reads the backtick-`f` as its **form-feed escape**, so the
text became `<FF>orge`. Invisible in every editor and diff, and it survived review twice.

[tools/check-encoding.ps1](tools/check-encoding.ps1) did not catch it, because it tested only for invalid UTF-8
and for mojibake digraphs -- and a form feed is valid UTF-8 and is not mojibake. It now also rejects any C0
control or DEL other than tab, line feed and carriage return, reporting the byte value and line. The check was
verified against a planted form feed, not just against the repaired files.

The wider point is about tooling rather than encoding: **a shell that rewrites the text it is passed is the
wrong tool for editing source.** This repository's instructions already say to use the file tools instead of
PowerShell for file content, precisely because of escape hazards like this one; the damage happened in a
`String.Replace` chain that looked innocuous. Where a shell must be used, prefer single-quoted strings, and
verify the result by reading the file back rather than trusting the command's exit status.

## The encoding check also rejects a doc-comment marker glued to code

A stray `///` was found appended to a line inside a doc example: `/// }, None)?;///`. It was introduced by an
edit splice of mine four rounds earlier and survived every gate since.

The reviewer reported it as a compile failure. It is not: tested against a scratch file, a `///` in that
position is only an `unused_doc_comment` **warning**, and the doctest still compiles and passes. Running
rustdoc with `RUSTDOCFLAGS=-D warnings` does **not** catch it either, which was checked rather than assumed. So
the damage was real and the stated consequence was not -- worth separating, because the wrong consequence is
what would have driven the wrong fix (a lint setting that does nothing).

[tools/check-encoding.ps1](tools/check-encoding.ps1) now rejects `///` immediately following a non-space
character at end of line, in `.rs` files. The pattern was checked for false positives across the whole
repository before being adopted: there are none, because a legitimate doc comment is always preceded by
whitespace or begins its line. The script's own description was widened from "encoding" to **text hygiene**,
since neither this rule nor the control-character rule is an encoding fault; they live here because this is the
check CI already runs over every tracked file.

The first version of this guard was a **no-op**: it gated on a `$ext` variable that was not in scope inside the
file loop, so it silently matched nothing and the planted probe passed. This is the second time in this project
that a guard has been written, observed to pass, and believed. The rule that caught it is worth restating: **a
guard you have only seen pass is untested.** Plant the defect it is meant to catch and watch it fail, then
remove the defect and watch it pass. Both directions, every time.

## Restoring a file with an old timestamp silently disables the rebuild

Verifying a fix by planting the defect back requires *building* both states. Restoring the fixed file with
PowerShell's `Copy-Item` breaks that, because it **preserves the source file's `LastWriteTime`**. The restored
file is then older than the artifact built from the defective version, Cargo judges it up to date, and every
subsequent run silently executes the **defective binary**.

Observed exactly that: source stamped 02:08:34, test executable 02:09:51, `Finished in 0.01s` with no
`Compiling` line, and a test failing against source that was provably correct (the restored file hashed
identically to the known-good backup). Roughly twenty minutes went into hunting a defect in the fix before the
timestamps were compared. The near-miss is worse than the delay: had the *fixed* state been the one built from
a stale artifact, a broken fix would have appeared to pass.

Rules adopted:

- After restoring a file by copy, **stamp it**: `(Get-Item $p).LastWriteTime = Get-Date`. Editing through the
	file tools does this implicitly, which is why the problem only appears with shell copies.
- **Read the build output, not just the test summary.** A revert-verification run that does not print
	`Compiling` for the crate under test proves nothing at all.
- Prefer plant-and-restore through the file tools over shell copies, for the same reason the tools are already
	preferred for content: the shell's convenience operations have side effects on the metadata builds depend on.

## An ABI expectation in a test is written independently of the constant it checks

`environ_flags::LONG_FUNCTION` and `ENVIRON_VERSION` are ABI identities -- values the operating system reads --
so the repository's no-inline-numeric-constants rule applies and they are named at the point of definition.

Their *tests* are the exception that needed thinking about. `assert_eq!(LONG_FUNCTION, LONG_FUNCTION)` passes
however the constant is changed, so a test that imports the implementation's constant to check the
implementation's constant pins nothing. The first response to that was to keep bare `1` and `3` literals in the
assertions, with a comment explaining why -- which traded one rule away for the other.

Both hold at once with a test-local `expected_abi` module restating the values independently. The assertion is
still against a number written separately from the implementation, so changing the implementation constant
fails the test; and the number is still named, so what it *is* stays legible. This is the general shape for
pinning any ABI or wire value: name it twice, deliberately, on either side of the test boundary.

## Summary prose keeps overclaiming what the reference docs get right


Four consecutive review rounds found an absolute statement in overview or description prose contradicting a
precise one in the reference documentation next to the code:

- `stop_and_drain` "leaves the object idle on return" -- true only absent concurrent external arming.
- The crate overview and README: a `ThreadpoolTimer` "never overlaps" -- true only for re-arming through
	`TimerFiring`, and the type's own docs said so in a section titled *When firings can overlap*.
- The pull request: `set_pool` "takes an owned `ThreadpoolPool`" -- it takes a borrow, as the signature shows.
- The pull request: `CallbackEnviron<'pool>` "makes the compiler enforce that the pool outlives the objects
	created from it" -- it enforces that against the *environment*; `ThreadpoolPool`'s own *Ordering
	requirement* section explains at length why it cannot reach the objects.

The pattern is consistent enough to be worth naming. Reference documentation gets written while looking at the
code, with the awkward cases in view. Summary prose gets written while looking at the *intent*, and the
qualifiers are exactly what summarising drops. Two of these were introduced while correcting a different
inaccuracy in the same paragraph.

The rule adopted: an unqualified "never", "always", or "the compiler enforces" in overview prose is a claim that
must be checked against the reference documentation for the same item before it is written, and re-checked when
the surrounding text is edited. Where the precise statement is too long to summarise, name the guarantee and
link to it rather than compressing it into an absolute.

## A thread maximum of zero is refused, and the maximum beats the minimum

**Partly superseded: the "maximum beats the minimum" bullet is wrong. See
[Conflicting thread limits are refused, because Win32 resolves them silently](#conflicting-thread-limits-are-refused-because-win32-resolves-them-silently).**
The zero-maximum bullet stands.

Two facts about `SetThreadpoolThreadMaximum`, both established by measurement rather than from the SDK page,
which states neither:

- **A maximum of zero leaves a pool that runs nothing.** A submitted work item was measured never executing;
	the call returns void, so the mistake is unreportable and undiscoverable except by the callbacks never
	arriving. `set_max_threads` therefore returns `io::Result<()>` and rejects zero, matching
	`set_min_threads`. This is a slightly different case from the period and length rejections elsewhere: there
	the platform did something *other* than what was asked, whereas here it does exactly what was asked -- the
	request is simply never useful, and a safe API should not hand back a pool that cannot run anything.
- **The maximum takes precedence over the minimum.** *(Superseded -- this holds only for the min-then-max
	ordering, and even then only in steady state.)* Setting a minimum of 4 and then a maximum of 2 was
	measured peaking at 2 concurrent callbacks. The method previously documented the opposite ("the pool clamps
	the value to at least the current minimum"), and a unit test carried that claim in its *name*. Both are
	corrected, and the measured behaviour is now pinned by a test rather than asserted in prose.

## <a id="conflicting-thread-limits-are-refused-because-win32-resolves-them-silently"></a>Conflicting thread limits are refused, because Win32 resolves them silently

The previous decision recorded that "the maximum takes precedence over the minimum", from a single measurement
of one ordering. That generalisation was wrong, and the test pinning it was intermittently failing at roughly 1
run in 30 of the full suite. Measuring both orderings and both regimes:

| Sequence | Peak concurrent callbacks |
|---|---|
| `set_min_threads(4)` then `set_max_threads(2)` | 2 in steady state (40/40 isolated, 60/60 under CPU load) |
| `set_min_threads(4)` then `set_max_threads(2)`, many pools created concurrently | 3 observed in 1 trial of 240 |
| `set_max_threads(2)` then `set_min_threads(4)` | **4**, in every one of 60 trials, and it does not settle back after 250ms |

So the real rule is **last call wins**, not "the maximum wins". A minimum set after a lower maximum annuls that
maximum outright: the pool runs `minimum` callbacks concurrently and stays there. Both setters return void or a
bare success, so nothing reports the conflict, and Win32 offers no getter with which a caller could notice.

Two consequences were adopted.

**The wrapper tracks the limits it has set and refuses a pair that cannot both hold.** `set_max_threads` rejects
a value below a minimum we previously set, and `set_min_threads` rejects a value above a maximum we previously
set; both with `io::ErrorKind::InvalidInput` and no `raw_os_error`, matching the zero-maximum rejection above.
This is the same principle as that rejection -- the platform will do something the caller did not intend and
cannot observe -- applied to the pair rather than to a single argument. The alternative of silently clamping was
rejected: clamping would also annul one of the two limits, differing only in which one, and would still be
invisible.

Each tracked limit is an `Option<u32>` that stays `None` until the corresponding setter succeeds, because there
is no way to read a pool's current limits back from Win32. A limit we were never told cannot be used to reject
its counterpart, so a fresh pool accepts either setter at any value. This is a deliberate limit on the check
rather than an oversight: guessing at the documented default maximum in order to reject more would mean
enforcing a number we had not verified.

**The maximum is documented as a steady-state target, not an instantaneous ceiling.** Raising the minimum
creates threads eagerly, and those surplus threads are not retired the moment a lower maximum is applied -- the
peak of 3 above. Refusing the conflicting pair makes that window unreachable through the safe API, which is what
removed the flaky test, but the underlying property is worth stating: the maximum bounds resource consumption,
and is not a mutual-exclusion mechanism.

This is the fifth round in which summary prose overclaimed a precise result -- see [Summary prose keeps
overclaiming what the reference docs get right](#summary-prose-keeps-overclaiming-what-the-reference-docs-get-right).
The difference is instructive: the earlier four were summaries contradicting correct reference documentation,
whereas here the reference documentation itself generalised one measurement into a rule. **A measurement
establishes what happens in the case measured.** Where the statement is about which of two settings wins, the
other ordering is a different case and has to be measured too.


## A periodic timer rejects any period it cannot actually repeat at

`SetThreadpoolTimer` takes the period as whole milliseconds. A period under 1ms therefore rounds down to zero,
and a zero period tells the pool not to repeat -- so a sub-millisecond `ThreadpoolPeriodicTimer` fired exactly
once and stopped. Measured before the fix: 999us fired once in 300ms where 1000us fired 31 times, with no error
at any point.

`ThreadpoolPeriodicTimer::new` now rejects anything below `MIN_PERIOD` (one millisecond), a public constant so
callers can see the limit rather than discover it. Rejecting was chosen over silently rounding up because the
constructor already rejects zero for the same underlying reason, and because substituting a different period
than the caller asked for is the kind of hidden behaviour this crate avoids elsewhere.

`MIN_PERIOD` is a floor on what can be *asked for*, not a delivery guarantee: ticks arrive on the system timer
tick, ~15.6ms by default. The two limits are unrelated -- one is the width of a field, the other is the
resolution of the clock -- and conflating them would suggest a 1ms period ticks at 1ms, which it does not.

The lower bound alone turned out to be only a third of the problem, because the same `as_millis()` conversion
alters two other classes of period. A **fractional** one such as 1.5ms is truncated, so the timer ticks at 1ms
while `period()` still reports 1.5ms; a period beyond **`u32::MAX` milliseconds** is capped, ticking far more
often than asked. Both are now rejected too, alongside a `MAX_PERIOD` constant, so `period()` can never disagree
with what was scheduled. Accepting a value and then quietly altering it is the recurring defect in this crate's
history -- see the ioctl length limits below, which are the same mistake in a different module.

## Lengths that do not fit the Win32 field are rejected, not capped

`DeviceIoControl`, `ReadFile`/`WriteFile` and the scatter/gather calls all take their sizes as `u32`. The
helpers that produced them capped with `unwrap_or(u32::MAX)`, so a buffer larger than 4GiB submitted only a
prefix -- or described the output buffer as smaller than it was -- and then reported success for an operation
that did something other than what was asked.

Every entry point now validates before allocating, so an unusable request costs nothing. The submitting paths
measure the lengths up front rather than inside their submission closures, which run at the FFI boundary and
have no way to report an error; the closures derive only pointers. That ordering is also what makes the tests
affordable: they ask for 4GiB and never allocate it.

The scatter/gather adapters reach the limit through a page *count*, which is the more plausible route to an
oversized total, and checking before allocating additionally converts what would have been `PageBuffers::new`'s
overflow panic into an ordinary `InvalidInput` error.

### There is no 64 MiB ceiling on scatter/gather, and we checked

A review round asserted that `ReadFileScatter` and `WriteFileGather` have a documented per-call limit of 2^26
bytes (64 MiB), and that all four scatter/gather adapters should reject anything larger. **They do not, and we
did not.** This is recorded so the claim is not re-raised, and so nobody later "fixes" the absence.

Two independent checks, both negative:

- The Microsoft Learn pages for both functions were read in full -- parameters, return value, remarks. Neither
	states any per-call byte ceiling. The only size constraints documented are that the segment array must have
	enough elements for the byte count, that each buffer is one page and page-aligned, and that the total must be
	a multiple of the volume sector size because the handle is opened `FILE_FLAG_NO_BUFFERING`.
- Measured directly: scatter reads of 16383, 16384, 16385 and **32768** pages all succeeded, the last returning
	134,217,728 bytes -- 128 MiB, twice the claimed ceiling, straddling it in both directions.

Adding the suggested check would have rejected requests the platform accepts, introducing a defect while
appearing to remove one. Where a real environment-specific ceiling exists -- a filesystem, a redirector, a
driver -- it surfaces as an ordinary native error, which these adapters already return unaltered. That is the
right division: reject what the *API contract* cannot express (a length that does not fit the `u32` field),
and report what the *platform* refuses.

The general point is worth keeping: a review finding is a hypothesis, not an instruction. This one was specific,
plausible and confidently worded, and still wrong.

### One saturation is deliberate

A coalescing *window* is a permission -- "you may delay this firing by up to
this much to batch it" -- and the pool is always free to fire earlier, so a saturated window asks for less
coalescing rather than producing a wrong result. Periods pass through the same helper but cannot reach the
saturation, being validated at construction. The distinction that matters is whether capping loses data or only
loses an optimisation.

Worth recording as process rather than design, and worth reading twice: **`device.rs`, `fs.rs` and `socket.rs`
each carried their own copy of the capping helper.** The first review named only `device.rs`; the second round
found `fs.rs` and closed with "when a defect is found in a helper, check whether the helper has siblings"; the
third round then found `socket.rs`, which that very advice would have caught had it been acted on rather than
merely written down. Writing a lesson in a design note is not the same as applying it -- when a defect class is
identified, grep the workspace for the whole class before declaring it fixed.

## A wait's re-arm is immediate, so its callback can overlap itself

`TimerFiring::rearm_after` is deferred until the callback returns, precisely so a one-shot timer's firings stay
sequential. `WaitActivation::rearm` cannot work that way: the SDK requires the wait to be armed for the handle's
*current* signal state to be observed, so the arming takes effect immediately.

The consequence is easy to miss and expensive to meet. On a manual-reset event the handle stays signalled, so
re-arming from inside the callback queues the next activation at once -- before the current one returns.
Measured: re-arming at the top of a 20ms callback entered it 7529 times in 400ms, 5110 of those overlapping an
earlier entry. An auto-reset event does not do this, because the wait consumes the signal.

Nothing about that is wrong, but it is the *opposite* of the guarantee the timer next door gives, and a caller
who carries the assumption across gets what looks like a runaway pool. It is now documented on both the method
and the type, contrasted explicitly with the timer, and pinned by tests covering the overlap, the auto-reset
case, and the mitigation -- so the documentation cannot quietly stop being true.

Writing the mitigation exposed that it was unreachable: the advice is to reset the event before re-arming, but
`WaitActivation` exposed no way to reach the handle, and a callback cannot capture it because the wait owns it.
`WaitActivation::handle` closes that gap. Documenting a way out is worth nothing if the API does not provide one.

## The blocking backend's "one operation at a time" is enforced by `&mut self`

`BlockingEndpoint` completes one operation at a time by waiting on the *handle* with `GetOverlappedResult`. With
two operations outstanding the handle is signalled by whichever finishes, so a call can return the other's
result and hand back buffers the kernel is still writing into.

That constraint used to live only in `run`'s safety comment, while every safe adapter took `&self` on a type
that is automatically `Send + Sync` -- so safe code could break it by sharing an endpoint across threads. The
safe adapters now take `&mut self`, which turns it into a borrow-check error, the same protection cleanup-group
members get and at the same cost: none. A caller who genuinely wants to share an endpoint wraps it in a `Mutex`,
which is explicit about the serialization it is buying.

`run` keeps `&self` and stays `unsafe`. A caller driving the raw seam may legitimately hold other borrows, and
an `unsafe` function's contract is the right home for an obligation the type system is not being asked to check.

The guarantee is pinned by a `compile_fail` doctest paired with a positive control that differs *only* in single
ownership versus an `Arc`. That pairing matters: a `compile_fail` test passes for any compile error, including a
typo, so on its own it proves nothing about the reason.

## Exhausting the generation sequence fails rather than wraps

`OperationId::mint` took generations with `fetch_add`, which wraps at `u64::MAX` and then reissues generations
from zero -- reintroducing exactly the stale-identity aliasing that generations were added to prevent, against a
type that states an (address, generation) pair names one submission *for the life of the process*.

Minting now refuses to pass `u64::MAX`, in a **single** atomic update. The first attempt at this used `fetch_add`
followed by a `store` to pin the counter, which only narrowed the window: `fetch_add` leaves the counter wrapped
to zero until the `store` lands, and a thread arriving in between takes 0, then 1, 2, ... and mints
successfully. A `fetch_update` that saturates means the counter never transiently holds a wrapped value, so
there is no window to arrive in. (Use `then`, not `then_some`, inside it -- the latter is eager and overflows at
the boundary.)

Exhaustion remains unreachable in practice -- centuries at one submission per nanosecond -- so this is about the
invariant being enforced rather than merely asserted. The counter is a parameter of a small helper purely so the
boundary is testable; production code always passes the static.

The regression test watches the *counter*, not the mint. An earlier version tried to catch a thread minting a
recycled generation and passed against the broken implementation, because the window is a few instructions wide
and hitting it is luck. An observer sampling the counter sees the wrapped value long before any thread happens
to consume one, which is the difference between a test that detects the defect and one that merely might.

Two further corrections to that test are worth recording, because both are hazards any concurrent test can hit:

- **A barrier, not a hope.** Its observers could be scheduled *after* the minters had finished, in which case
	they sampled nothing and the test passed against the very implementation it existed to catch. A barrier
	across observers and minters makes the observers provably running before the boundary is crossed. Detection
	is still probabilistic -- but across 80,000 wrap events rather than one, and no longer vacuous.
- **Never swap the panic hook from worker threads.** The test had four threads each calling `take_hook` /
	`set_hook` / restore around `catch_unwind`. The hook is *process-global* and Cargo runs tests in parallel
	threads of one process, so an unlucky interleaving leaves the no-op hook installed and silently strips
	diagnostics from unrelated tests. The fix is not more careful hook juggling: `try_next_generation` gives the
	test a non-panicking form, so it raises no panics and touches no hook at all. Where the hook genuinely must
	be swapped -- the stress suite's deliberate-panic scenario -- it is done on the scenario's own thread while
	holding the lane that serializes the binary, and the comment there now says why both conditions are needed.

The regression test asserts the behaviour rather than the guard: the shortest accepted period must actually
repeat. Lowering `MIN_PERIOD` makes it fail by timing out, which is how the original defect presented.

## A cleanup group's release does not latch

`release_members` used to set a `released` flag and return early on every later call. Because the `create_*`
methods take `&self`, a group could gain new members after a release returned -- and those members were then
skipped by both a later `close_members` and by `Drop`. Measured before the fix: `owned_resources` remained at 1
after a second `close_members`, so the context leaked and `CloseThreadpoolCleanupGroup` ran with a live member.

The flag was removed rather than reset per batch. `CloseThreadpoolCleanupGroupMembers` is idempotent -- with no
members it does nothing -- so releasing unconditionally costs nothing and makes the group genuinely reusable.
That is a better answer than the alternative of rejecting member creation after a release: it turns a latent
leak into a supported lifecycle rather than into a new error path.

The general shape is worth remembering, because it is the same mistake as the period check above: both guards
tested the condition that had been *written down* (has this run before; is the period zero) rather than the one
that actually mattered (are there members to release; will the period round to zero).

## <a id="a-panicking-callback-aborts-rather-than-being-contained"></a>A panicking callback aborts, rather than being contained

Every trampoline used to wrap its callback in `catch_unwind` and discard the payload. That is now removed, and
a callback that panics aborts the process.

The containment contradicted the crate's own contract. The callback contract already said a callback **must
not unwind across the FFI boundary** -- so unwinding was a documented violation, and the `catch_unwind` then
quietly forgave it. A guarantee that silently rescues callers from breaking a stated rule makes the rule
unenforceable and teaches callers that panicking is supported.

Removing it costs nothing in diagnosability, which is the point most likely to be misjudged. `catch_unwind`
discards the panic *payload*, not the message: the panic hook runs before unwinding begins, so the default
hook has already printed the message and location to stderr. Containment was buying process survival only,
never visibility. What changes is that a violation now stops the program instead of being absorbed.

The mechanism is Rust's, not this crate's: since Rust 1.81 an unwind escaping an `extern "C"`-family function
aborts. Removing the catch does not *add* an abort path, it stops intercepting the one the language already
defines.

Consequences worth knowing:

- The behaviour cannot be tested in-process, because the abort takes the test runner with it. It is covered by
	[crates/windows-threadpool-sys/tests/callback_panic_aborts.rs](crates/windows-threadpool-sys/tests/callback_panic_aborts.rs),
	which re-executes the test binary as a child selected by an environment variable and asserts from the parent
	that the child died abnormally. The parent asserts *not a clean exit* rather than a specific status, because
	the exact code Windows reports for a Rust abort is a toolchain detail that has changed across releases;
	pinning it would produce failures that are not regressions in this crate.
- Two of those children cannot join their callback deterministically. `WaitForThreadpoolTimerCallbacks` and
	`WaitForThreadpoolWaitCallbacks` wait for callbacks that are *executing*, not ones merely queued, so a child
	that armed a 1 ms timer and immediately waited would return before the callback started and exit cleanly.
	Those children outlive the firing by sleeping instead. The work and I/O children need no such treatment,
	because `WaitForThreadpoolWorkCallbacks` covers pending submissions and `run_down` waits for the completion.
- The stress suite's deliberate-panic scenario is gone, and with it the process-global panic-hook swapping it
	required -- a hazard already recorded further down this file.

## The timer stress suite is opt-in, and asserts only what load cannot perturb

The timer types carry the crate's subtlest concurrency contracts, and the unit tests establish each one once,
deterministically. [crates/windows-threadpool-sys/tests/timer_stress.rs](crates/windows-threadpool-sys/tests/timer_stress.rs)
applies pressure to them instead.

It is gated on the `WINDOWS_THREADPOOL_STRESS` environment variable, with `WINDOWS_THREADPOOL_STRESS_SCALE`
multiplying every load count. An environment variable rather than `#[ignore]` because one knob turns the whole
suite on and the tests still compile and lint in CI, so the suite cannot rot while nobody runs it. The gate is
applied by a macro rather than a line inside each test: a gate that has to be remembered per test is one that
gets forgotten in exactly one of them, and that test is then a load test running in CI. Scenarios also take a
process-wide lane, because Cargo runs them on parallel threads against one shared pool, where they would
otherwise measure each other.

Assertions are confined to what load cannot perturb: non-overlap where the type guarantees it, quiescence after
a drain, `owned_resources` reaching zero, and the absence of a hang or a crash. Rates, latencies, and exact
firing counts are reported instead, because under load they describe the machine.

Two measurements shaped every scenario, and are recorded here because they are not obvious and they will
otherwise be rediscovered the hard way:

- **Pool timers fire on the system timer tick, ~15.6ms.** A zero-delay re-arm does not fire immediately; it
	fires on the next tick. A self-re-arming chain therefore advances at roughly 64 links a second however
	trivial its callback, so chain lengths are sized for wall-clock time -- raising them buys duration, not
	coverage.
- **A loop that arms without pausing outruns the pool completely.** Early versions of the churn, rapid-drop, and
	start/stop scenarios recorded *zero* callbacks: the loop never left the timer armed when a tick arrived, so
	each was silently testing the arming calls alone. Scenarios that need firings now pause past a tick and
	assert a floor, so the same degeneration cannot recur unnoticed. The rapid create/arm/drop scenario keeps its
	zero -- there it is the point, and it is documented rather than asserted away.

## Captured impersonation is a separate platform layer

The workspace will add the independently published
`windows-impersonation-token-sys` crate. It owns an opaque
`ImpersonationToken` that captures the calling thread's effective impersonation
state into owned transportable state, applies that state temporarily on another
thread, and restores the exact prior thread-token state afterward. Restoration
failure panics from the application guard's `Drop`: returning a shared worker to
the pool under an unknown identity is a process security failure, not an error
that can be safely reported and ignored.

The type is crate-owned even though this workspace normally prefers Microsoft
native types. A raw `HANDLE` cannot express ownership, required rights,
cross-thread transportability, immutability, scoped application, exact nested
restoration, or the fail-fast restoration invariant. The helper remains built on
the corresponding `windows-sys` token APIs and does not define a competing token
model.

The ordinary file-enumeration SQ begin helper captures this token synchronously
before publishing its begin message. A second explicit-token form lets a future
traversal layer capture once at traversal submission and reuse that same context
for all descendant directory opens. Recapturing on a thread-pool worker would
capture the worker or process identity and repeat the Globazog defect this layer
exists to prevent.

The legacy `QueueUserWorkItem` API exposes this behavior through
`WT_TRANSFER_IMPERSONATION`; the modern object-based thread pool has no
equivalent callback-environment setting. Microsoft WIL provides the constituent
open-current-token and exact-restore helpers in C++, but the Microsoft Rust
libraries expose raw APIs rather than a complete capture/transport/apply type.
The reasoning and rejected Rust alternatives are recorded in
[DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

## Flat asynchronous enumeration is a separate publishable crate

The workspace will add the independently published
`windows-file-enumeration-sys` crate. One request enumerates one directory; a
future traversal crate composes those flat requests rather than moving recursion
into this layer. The crate must replace Globazog's current Windows
one-directory backend without losing metadata, native path/name fidelity, error
detail, backpressure correctness, or throughput.

Each session owns a bounded multi-producer submission ring and bounded
single-receiver completion ring. Begin, cancellation, abandonment, and future
control operations enter through the SQ. Entries and exactly one terminal
outcome per accepted enumeration leave through the CQ, tagged with an
`EnumerationId`. Ordinary SQ saturation rejects a begin synchronously.
Per-enumeration reservations make later cancellation infallible; a standing
session reservation makes receiver-drop abandonment infallible. CQ terminal
reservations may not consume every data slot.

The native engine uses an opened directory handle and
`GetFileInformationByHandleEx` with `FileIdExtdDirectoryRestartInfo` followed by
`FileIdExtdDirectoryInfo`. It owns one fixed, reusable, correctly aligned native
buffer per request, defaulting to 64 KiB and clamping configured values below
1 KiB. It retains partially parsed batches across callbacks, performs at most
one potentially blocking refill per callback, and uses finite record and cheap
monotonic elapsed-time budgets. Ring pressure suspends enumeration without
blocking a worker or dropping an accepted entry. `FindFirstFileExW`,
`FindNextFileW`, direct `Nt*` APIs, and IOCP integration are not part of this
design.

The remaining v1 surface decisions -- predicates, metadata selection, native
timestamp representation, errors, paths, ordering, unsupported filesystems, and
oversize records -- must be settled before their implementation. That work is
scheduled as FE-2 in [CHECKLIST.md](CHECKLIST.md), not left only in this design
record.

Primary references:
- [Thread Pools](https://learn.microsoft.com/windows/win32/procthread/thread-pools)
- [Thread Pool API](https://learn.microsoft.com/windows/win32/procthread/thread-pool-api)
- [Using the Thread Pool Functions](https://learn.microsoft.com/windows/win32/procthread/using-the-thread-pool-functions)
- [`threadpoolapiset.h` API index](https://learn.microsoft.com/windows/win32/api/threadpoolapiset/)
- [`ReadDirectoryChangesExW`](https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-readdirectorychangesexw)
- [`CancelIoEx`](https://learn.microsoft.com/windows/win32/api/ioapiset/nf-ioapiset-cancelioex)
- [`windows-sys::Win32::System::Threading`](https://docs.rs/windows-sys/0.61.2/windows_sys/Win32/System/Threading/)
