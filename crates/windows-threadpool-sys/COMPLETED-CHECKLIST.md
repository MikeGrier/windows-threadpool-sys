# Completed checklist: windows-threadpool-sys

Append-only archive of completed work items. See [CHECKLIST.md](CHECKLIST.md) for pending work and
[PLANS.md](PLANS.md) for plan status. Design decisions are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md).

## Moved 2026-08-17 — M1 callback environment, M2 work submission, M3 TP_IO backend

### M1 — Callback environment

- [x] **M1-1** -- Implement SDK-equivalent `TP_CALLBACK_ENVIRON_V3` initialization and mutation -- the
	header-only helpers that `windows-sys` does not emit. See [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [x] **M1-2** -- Unit-test callback-environment init and mutation against the documented default field values
	(version, priority, size).

### M2 — Work submission and callback ownership

- [x] **M2-1** -- Implement and test one end-to-end work submission abstraction over `CreateThreadpoolWork`,
	`SubmitThreadpoolWork`, `WaitForThreadpoolWorkCallbacks`, and `CloseThreadpoolWork`.

- [x] **M2-2** -- Validate the callback ownership model with the work abstraction before extending it to timers
	and waits.

### M3 — TP_IO backend over the shared seam

- [x] **M3-1** -- Wire the `windows-threadpool-sys` -> `windows-overlapped-io-sys` dependency (path plus
	version), and update the publish workflow so the overlapped crate releases before this one.
	*(completed 2026-08-17 15:37:00 -04:00)*

	The publish workflow blocks the `windows-threadpool-sys` job until the required
	`windows-overlapped-io-sys` version is live on crates.io, because `cargo publish`'s verification build
	resolves the versioned dependency from the registry rather than the workspace path. That makes the
	release order independent of the order the two tags are pushed.

- [x] **M3-2** -- Implement the `TP_IO` backend over the shared seam with balanced `StartThreadpoolIo` /
	`CancelThreadpoolIo` accounting and callback-driven reclamation.
	*(completed 2026-08-17 15:56:00 -04:00)*

	Added `ThreadpoolIo` and `IoCompletion`. One counter serves as both the pool's start/cancel accounting
	and the crate's rundown state, since an unbalanced start is exactly an operation whose storage the
	kernel or pool still owns. Required widening the shared seam with `OperationId::from_ptr`, which was
	fixed in `windows-overlapped-io-sys` rather than worked around here.

- [x] **M3-3** -- Test `TP_IO` across the behavioral matrix: immediate failure, immediate success, pending
	completion, cancellation, and object rundown with operations outstanding.
	*(completed 2026-08-17 16:06:00 -04:00)*

	21 integration tests in `tests/tp_io_behavioral_matrix.rs` covering the five matrix states plus the
	accounting and reclamation invariants at scale (512 file reads, 256 simultaneously outstanding pipe
	reads, 8-thread concurrent submission) and the edge cases: a panicking callback, reads past EOF,
	zero-length reads, mixed payload types, unclaimed completions, and repeated idempotent rundown.

	The matrix surfaced that an operation identity is unique only among *simultaneously outstanding*
	operations -- reclaiming an operation returns its storage address to the allocator, which may reissue it.
	That contract was undocumented and is now recorded on `OperationId` and in the overlapped crate's
	design notes.

## Moved 2026-08-17 — M4 safe abstractions and documentation

M4 was restructured during execution. Its original first item bundled work, timer, wait, and I/O, but work had
landed in M2 and I/O in M3, so only timer and wait remained; those became separate implementation and test
items. Two items were added from defects found while building: M4-1 (a `CallbackEnviron` soundness hole) and
M4-8 (the one-shot/periodic timer split).

- [x] **M4-1** -- Close the `CallbackEnviron` soundness hole: add an owned `ThreadpoolPool` and change
	`set_pool` to accept it, and make `set_cleanup_group` `unsafe` pending a full cleanup-group design.
	*(completed 2026-08-17 17:00:00 -04:00)*

	`set_pool` and `set_cleanup_group` were **safe** functions accepting a raw `PTP_POOL` / `PTP_CLEANUP_GROUP`
	(bare `isize`) that the thread pool later dereferences, so safe code could cause undefined behavior.
	`set_library` next door was already `unsafe` for exactly this reason.

- [x] **M4-2** -- Implement a safe timer over `CreateThreadpoolTimer`, `SetThreadpoolTimer`,
	`IsThreadpoolTimerSet`, `WaitForThreadpoolTimerCallbacks`, and `CloseThreadpoolTimer`.
	*(completed 2026-08-17 17:05:00 -04:00)*

- [x] **M4-3** -- Test the timer across one-shot, periodic, absolute, disarming, cancellation, and destruction.
	*(completed 2026-08-17 17:05:00 -04:00)*

	The tests corrected the implementation's specification: `IsThreadpoolTimerSet` reports whether a due time is
	set, not whether the timer will fire again, so a one-shot timer stays set after firing. The `is_set`
	documentation had said the opposite.

- [x] **M4-4** -- Implement a safe `ThreadpoolWait` that owns its waitable handle and rearms per activation.
	*(completed 2026-08-17 17:12:00 -04:00)*

	The object owns the handle so it cannot be closed under a pending wait, and the callback receives a
	`WaitActivation` carrying `rearm`, since the SDK consumes the arming on each activation.

- [x] **M4-5** -- Test the wait across signalled and timeout activation, rearming, disarming, and destruction.
	*(completed 2026-08-17 17:12:00 -04:00)*

	Also de-flaked the identity tests in both crates: they required the allocator to naturally reuse an address,
	which is not guaranteed. Natural reuse is now reported rather than asserted, and the hazard is covered
	deterministically by tests that synthesize a stale generation at a live address via `OperationId::from_parts`.

- [x] **M4-8** -- Split the timer into `ThreadpoolTimer` (one-shot) and `ThreadpoolPeriodicTimer`, and give each
	callback a token. *(completed 2026-08-17 17:25:00 -04:00)*

	The first implementation followed the platform and modelled both kinds with one object, where a `period`
	argument silently changed the concurrency contract. A periodic timer may queue its next tick while the
	previous one is still running, so its callback must tolerate overlapping with itself; a one-shot never
	overlaps. `TimerFiring::rearm_after` gives non-overlapping repetition measured from the end of each firing;
	`PeriodicTick::stop` lets a periodic timer end itself. A zero period is rejected rather than silently
	degenerating to a one-shot.

- [x] **M4-6** -- Design and implement safe cleanup-group membership across every callback object.
	*(completed 2026-08-17 17:33:00 -04:00)*

	Option (A) of the three considered: the group creates and owns its members. Flagging objects as
	"group-owned" is insufficient, because each also owns a heap callback context that is only safe to free once
	the bulk release has finished -- a moment an individual object cannot observe. Use-after-release is a compile
	error, pinned by a `compile_fail` doc test. Thread-pool I/O is excluded on purpose: a `TP_IO` object must not
	be closed with an operation outstanding, and a bulk release cannot satisfy that.

- [x] **M4-7** -- Add API examples and generated documentation.
	*(completed 2026-08-17 17:20:00 -04:00)*

	Crate-level guidance, runnable doc examples for every object type, a rewritten README, and docs.rs metadata
	pinning a Windows target -- without which documentation for this Windows-only crate would fail to build.

## Moved 2026-08-17 — M5 timer stress suite

An opt-in load suite for the timer types, gated on `WINDOWS_THREADPOOL_STRESS` and scaled by
`WINDOWS_THREADPOOL_STRESS_SCALE`. 24 scenarios, about a minute at scale 1. See
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) for the decision, and the crate [README.md](README.md) for how to run
it.

Two measurements shaped the whole suite and are recorded in the design note: pool timers fire on the ~15.6ms
system tick, and a loop that arms without pausing outruns the pool entirely -- which made three early scenarios
record zero callbacks while appearing to pass.

- [x] **ST-1** -- Stress harness plus the one-shot arming and re-arming scenarios. *(completed 2026-08-17 20:14:49 -04:00)*

	Env-var gate applied by a macro so it cannot be forgotten per test, a scale knob, and a serialization lane
	so two heavy scenarios never measure each other. Scenarios: self-re-arm chains asserting the documented
	non-overlap guarantee, past-instant `rearm_at` chains, arming churn across eight threads, arm/disarm races,
	deterministic arm-fire cycles, a large one-shot population, coalescing windows, and contained panics.

- [x] **ST-2** -- One-shot teardown stress: `Drop` racing a firing and a re-arming callback. *(completed 2026-08-17 20:14:49 -04:00)*

	Rapid create/arm/drop churn, drops walking across the due time, drops landing mid-callback with a deferred
	re-arm pending, and concurrent teardown across eight threads. Targets the window closed by the second PR
	review round, where a regression appears as a hang or a crash rather than a failed assertion.

- [x] **ST-3** -- Periodic timer stress: high-frequency ticking, self-stop, and deliberate tick overlap. *(completed 2026-08-17 20:14:49 -04:00)*

	Sustained ticking, deliberately overlapping ticks (peak 3 concurrent, confirming the documented contract
	empirically), self-stop from inside the callback, start/stop churn, drops while ticking and mid-tick, a
	large ticking population, and zero-period rejection under repetition.

- [x] **ST-4** -- Cleanup-group timer members and a mixed load scenario. *(completed 2026-08-17 20:14:49 -04:00)*

	A group releasing its members is a distinct teardown path: large member populations released as a unit under
	both dispositions of `cancel_pending`, concurrent release across threads, and a group left to drop. The
	mixed scenario runs a self-re-arming one-shot, a periodic population, and timer and group churn together,
	asserting the non-overlap guarantee survives a loaded pool.

- [x] **ST-5** -- Document how to run the suite. *(completed 2026-08-17 20:14:49 -04:00)*

	Crate README section covering both environment variables, the deliberate CI exclusion, the two measurements
	that shape the scenarios, and what is asserted versus reported.

## Moved 2026-08-18 — M16, thirteenth review round on PR #3

### <a id="mf-3"></a>MF-3 — Name the callback-environment ABI version, and pin ABI values against independently written expectations. *(completed 2026-08-18 02:42:11 -04:00)*

`Version: 3` was written inline while the flag bit beside it was a named constant, contrary to the repository's
rule against inline numeric identities. It is now `ENVIRON_VERSION`, documented as a breaking ABI change.

The assertions were the harder half. They had deliberately kept bare `1` and `3` literals, on the sound ground
that importing the implementation's constant to check the implementation's constant pins nothing. A test-local
`expected_abi` module satisfies both rules at once: the expectation is written independently of the
implementation, and it is still named. Recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

### <a id="mf-4"></a>MF-4 — Correct the `set_pool` ownership comment in the callback-environment tests. *(completed 2026-08-18 02:42:11 -04:00)*

The comment said `set_pool` takes an owned `ThreadpoolPool`; it takes `&ThreadpoolPool` and records the borrow
as `CallbackEnviron<'pool>`. The fourth instance of this same false claim about `set_pool`, and the first found
in test commentary rather than in documentation or the pull request description.

## Moved 2026-08-21 -- M17 custom-close owner for non-`CloseHandle` wait targets

- [x] **M17.1** -- Let `ThreadpoolWait` own a wait target whose close routine is **not** `CloseHandle`. Today
  `WaitableHandle` wraps a std `OwnedHandle`, so `ThreadpoolWait` always closes its handle with `CloseHandle`
  on teardown (see [src/wait.rs](src/wait.rs) `Drop`). Add a seam -- e.g. `WaitableHandle::assume_waitable_with(raw,
  closer)` or a small `WaitClose` owner -- so the caller supplies the close function (for a
  `FindFirstChangeNotification` handle, `FindCloseChangeNotification`), and `ThreadpoolWait` drains the wait
  **before** invoking it exactly once. Keep the existing `OwnedHandle` path as the default. Unit-test that the
  custom closer runs exactly once and only after the wait is drained (direct `ThreadpoolWait::drop`).

- [x] **M17.2** -- Propagate the custom closer through the `CleanupGroup` path. `CleanupGroup::create_wait`
  moves the owner out via `ThreadpoolWait::into_parts` and adopts it as a boxed `OwnedHandle` freed with
  `CloseHandle` (see [src/cleanup_group.rs](src/cleanup_group.rs)), so a coarse handle in a group would be
  closed with the wrong routine. Carry the closer through `into_parts` / `WaitMember` / the adopted resource
  so the group release invokes it (after the group drains the wait) rather than `CloseHandle`, preserving the
  existing `OwnedHandle` default. Unit-test the group-release teardown path.

- [x] **M17.3** -- Integration: exercise **both** teardown paths -- direct `ThreadpoolWait::drop` and
  `CleanupGroup` release (with and without `cancel_pending`) -- and assert the custom closer runs exactly once,
  and only after the wait is drained, for each.

The three items landed as one commit. They are not independently completable: M17.1 changes the type
`ThreadpoolWait` owns, which is the same type `into_parts` hands to `CleanupGroup`, so the crate does not
compile with M17.1 in and M17.2 out. The checklist split them because they are separate *concerns* -- the
individually-owned path and the group path -- and that split is what M17.3's per-path integration coverage
is written against.

What was built: the handle a wait watches became a `WaitTarget`, either `Owned(OwnedHandle)` (the default,
unchanged for events) or `Custom`, holding the raw handle beside a caller-supplied
`unsafe extern "system" fn(HANDLE) -> BOOL`. `WaitableHandle::assume_waitable_with` is the narrow `unsafe`
constructor for one. Because the close is a *destructor* on a value both teardown paths already own, neither
path needed new ordering code: `ThreadpoolWait::drop` already drains before its fields drop, and
`CleanupGroup::release_members` already runs `CloseThreadpoolCleanupGroupMembers` before freeing adopted
resources -- the group change is the single substitution of a boxed `WaitTarget` for a boxed `OwnedHandle`.

`Drop` sits on an inner `CustomClose` struct rather than on `WaitTarget` itself, so the enum stays
destructurable by an ordinary `match`. That is what let `WaitableHandle::into_handle` become
`Result<OwnedHandle, Self>` with no `ptr::read` and no `unreachable!`: it returns `Ok` for the default path
and hands the wrapper back as `Err` for a custom target, rather than emitting an `OwnedHandle` that would
later close the handle with the wrong routine. That is a breaking change to a published signature.

Coverage: five unit tests in [src/wait/tests.rs](src/wait/tests.rs), five in
[src/cleanup_group/tests.rs](src/cleanup_group/tests.rs), and four integration tests at 256 live waits per
scenario in [tests/wait_custom_close.rs](tests/wait_custom_close.rs), covering direct drop, group release,
group drop, group release with `cancel_pending`, a repeated release, and a group holding custom and default
members together. The ordering tests assert that teardown actually blocked, so they cannot pass vacuously
when a callback happens to finish first. Recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

> **-> CROSS-COMPONENT HANDOFF:** M17 is complete, which clears the *external* prerequisite for component
> `crates/windows-file-watcher` -> M6 -> M6.1 (the coarse `FindFirstChangeNotification` watcher). See
> [../windows-file-watcher/CHECKLIST.md](../windows-file-watcher/CHECKLIST.md). It does **not** make M6.1
> startable: that item remains gated behind M2 through M5 of its own crate, whose next actionable item is
> M2.1. Corrected 2026-08-21 -- the original wording said "unblocks", which was read as "is now next".

## Moved 2026-08-21 -- M18: stop containing callback panics

- [x] **M18.1** -- Remove `catch_unwind` from all five trampolines (`io.rs`, `timer.rs`,
  `timer/periodic.rs`, `wait.rs`, `work.rs`), keeping the bookkeeping that precedes it.

- [x] **M18.2** -- Delete the six tests that assert the removed guarantee.

- [x] **M18.3** -- Replace them with subprocess tests that prove the new behaviour.

- [x] **M18.4** -- Update every statement of the removed guarantee.

The four items landed as one commit: removing the containment breaks the tests that assert it and falsifies
the docs that advertise it in the same instant, so no intermediate state compiles-and-passes. Splitting them
would have produced commits that fail their own gate.

Why it was removed: the callback contract already said a callback **must not unwind across the FFI
boundary**, so unwinding was a documented violation -- and the `catch_unwind` then quietly forgave it. A
guarantee that rescues callers from breaking a stated rule makes the rule unenforceable. The containment had
never been reviewed or approved.

Removing it costs no diagnosability, which was the point most likely to be misjudged: `catch_unwind` discards
the panic *payload*, not the message, and the panic hook runs before unwinding begins, so the message and
location already reached stderr. Containment bought process survival only. The abort itself is Rust's, not
this crate's -- since 1.81 an unwind escaping an `extern "C"`-family function aborts -- so removing the catch
stops intercepting a path the language already defines rather than adding one.

Testing it needed a subprocess harness, since an in-process test would kill its own runner:
[tests/callback_panic_aborts.rs](tests/callback_panic_aborts.rs) re-executes the test binary as a child
selected by an environment variable and asserts the child died abnormally. Writing it exposed a trap worth
recording -- `WaitForThreadpoolTimerCallbacks` and `WaitForThreadpoolWaitCallbacks` wait only for callbacks
already *executing*, so the timer and wait children returned from `wait()` before their callback started and
exited cleanly. The first run caught this as two genuine failures. Those children outlive the firing instead;
the work and I/O children need no such treatment, because `WaitForThreadpoolWorkCallbacks` covers pending
submissions and `run_down` waits for the completion.

Recorded in the workspace-root [DESIGN-NOTES.md](../../DESIGN-NOTES.md). Breaking change: a callback that
panics now ends the process instead of being absorbed.

## Moved 2026-09-27 11:24:16 -04:00 -- M-T1, lifecycle parity in the concurrency trace

### <a id="m-t11"></a>M-T1.1 -- Stamp every pool object's establishment, not only its callbacks. *(completed 2026-09-27 11:24:16 -04:00)*

**Found by review, not by the tests.** `M26.13.1` added entry/exit records to all five trampolines
and read "every pool-invoked functor" as the functors alone, leaving the objects that invoke them
uneven: the wait was stamped from creation through every arming to close, while the timer, the
periodic timer and the I/O object were stamped only where their callbacks ran. Nothing failed as a
result -- the question was asked directly.

**Why it was worth fixing before the next experiment rather than after.** A capture that stamps only
firings cannot tell a timer armed on time and dispatched late from one armed late, and that is
exactly the distinction
[windows-ioring-sys](../windows-ioring-sys/CHECKLIST.md) -> `M26.13`'s experiment 3 rests on. The
same distinction, for the wait, is what made `M26.13.3`'s answer readable at all.

**Twenty-four new call sites**, bringing the timer, the periodic timer and the I/O object to parity
with the wait, and closing two holes of the same kind in the wait and the work object:

| Target | Added |
|---|---|
| `timer` | `created`; `armed` and `disarmed` in the shared `arm_raw` / `disarm_raw`, so every `SetThreadpoolTimer` is stamped for **both** timer kinds; `rearm-requested` at both request entry points; `rearm-entered` / `rearm-left` / `rearm-suppressed` around the deferred application; `suppress-and-disarm`; three drop stages |
| `timer-periodic` | `created` carrying the period; three drop stages. Its `start*` and `stop` come free through the shared raw pair |
| `io` | `created` carrying the handle; `started` per `StartThreadpoolIo`; `start-cancelled` at both sites that balance a start with no callback; `rundown-begin` / `rundown-ended`; three drop stages |
| `wait` | `disarmed`, which a plain `ThreadpoolWait::disarm` had never recorded |
| `work` | three drop stages |

**One design point worth keeping.** `timer` and `timer-periodic` share `arm_raw` and `disarm_raw`, so
a single record at each covers both kinds rather than two near-identical ones: the record's second
slot carries `period_ms`, which is zero for a one-shot arming and non-zero for a periodic one, so the
two are told apart by the data rather than by the target.

**Verified by sabotage.** Each of the twenty-four sites was deleted in turn and the guard re-run: all
twenty-four caught. Two events are emitted from two places each -- `timer`'s `rearm-requested` and
`io`'s `start-cancelled` -- so those were additionally checked by removing both sites at once, which
is also caught.

**The guard** is `every_pool_object_records_its_creation_establishment_callbacks_and_teardown` in
[trace/tests.rs](src/trace/tests.rs), renamed because it is no longer only about trampolines, with
the `io` half asserted inside `io::tests` where the only real overlapped exercise already lives. The
timer probe now drives a deferred re-arm through **both** request entry points, so neither is merely
written.

**Stated limits, rather than implied.** The guard is one-sided and env-gated, for the reasons already
recorded on it, and it is **per-event, not per-call-site**: losing one of a two-site pair leaves the
event present and the test green. `io`'s inline-completion `start-cancelled` is not reachable from a
unit test here -- it needs a synchronous completion -- so it is exercised only by the integration
suite, where this assertion does not run.

**Buffer headroom re-checked**, since this adds record sites to a crate `M26.13.2` had just measured
at 14061 records against a capacity of 65536: the traced suite still announces no overflow.

## Moved 2026-09-27 14:25:47 -04:00 -- M-T2, an exception observer for the trace

### <a id="m-t21"></a>M-T2.1 -- The trace installs a vectored exception handler when it turns on, so a first-chance exception that something else swallows is still visible. *(completed 2026-09-27 14:25:47 -04:00)*

**Asked for during an investigation**, to test whether anything was being raised and swallowed
during a five-second window in which the trace otherwise records nothing. A first-chance exception
is invisible to a debugger's default view and leaves no other trace, so it is exactly the kind of
thing that window could have been hiding.

**What was added.** `AddVectoredExceptionHandler(CALL_FIRST, ...)`, installed by the trace itself
the moment it turns on, recording `exception raised` with the `NTSTATUS` code in one payload
slot and `ExceptionRecord->ExceptionAddress` -- the instruction that raised -- in the other. The
two `u64` slots the record already carried were exactly the room needed.

**It changes nothing**, and three properties are what make that true rather than hopeful. The
handler returns `EXCEPTION_CONTINUE_SEARCH`. And because it can fire on a thread that is already
inside the trace holding its lock, it reads every `OnceLock` rather than initialising one, takes
the buffer lock with `try_lock`, and suppresses the overflow announcement -- each costing a
dropped record rather than a deadlock, an allocation in an exception handler, or a write to stderr
from one.

**Installing it starts the trace's clock**, deliberately: `0.000000s` is then by construction the
moment the observer went live, so an empty capture means no exception was raised rather than that
the observer was late. This was not the first design -- the handler recorded **nothing at all**
until the installer was changed to bring the clock and the buffer into existence itself, because the
handler refuses to.

**The guard found that defect.** `the_exception_observer_notes_a_first_chance_exception` raises a
real first-chance exception through `OutputDebugStringA` -- which raises `DBG_PRINTEXCEPTION_C`
and catches it itself, so it is genuinely of the population being observed -- and asserts it was
recorded. Written before the observer worked, it reported `before=0 after=0` twice and was what
identified the initialisation gap.

**One feature cost.** `AddVectoredExceptionHandler` and `EXCEPTION_POINTERS` are gated on
`Win32_System_Kernel` as well as `Win32_System_Diagnostics_Debug` in `windows-sys`, so the
`trace` feature now enables both. A consumer who never traces compiles neither.

**What it found**, for the investigation that asked: no exception is raised during the stall window,
in 18 of 18 captures. Recorded in
[windows-ioring-sys](measurements/2026-09-27-exceptions-during-the-stall/README.md).

## Moved 2026-09-27 14:47:48 -04:00 -- M-T3, call-boundary tracing and a callback census

### <a id="m-t31"></a>M-T3.1 -- Bracket every Win32 call that blocks or takes a pool lock, under its own syscall target. *(completed 2026-09-27 14:47:48 -04:00)*

**Raised by review while reading the source:** the calls that actually execute waits carried no
tracing. True, and the gap was wider than the waits -- `pool.rs` and `cleanup_group.rs` had no
tracing at all, including `CloseThreadpoolCleanupGroupMembers`, which is the longest-blocking call
in the crate.

**Why a record after the call is not enough.** Several sites already stamped a record once the call
returned (`armed`, `created`, `submitted`). That cannot distinguish *issued late* from *took
four seconds to return* -- both look like a record with a later timestamp than expected. A bracket
turns the second case into a visible interval.

**Its own target, so it costs nothing to anyone who does not want it.** `syscall-enter` and
`syscall-leave`, with the Win32 function's own name as the event. A capture narrowed to
`wait,delivery` is unchanged; one narrowed to `syscall` sees only call boundaries. The
`trace_call!` macro keeps argument evaluation behind the filter check and compiles to the call
alone without the feature.

**Coverage is a census, not a judgement.** Every `Create*`, `Set*`, `Submit*`, `Close*`,
`Start*`, `Cancel*` and `WaitFor*` on a pool object, in all seven modules, plus `CancelIoEx`
and `IsThreadpoolTimerSet`. Which of them can contend is not documented, so none was assumed
cheap. A scan for pool calls outside a bracket now reports none.

**Also traced: the caller-supplied wait close routine.** `CustomClose::drop` invokes a function the
caller gave us -- the case it exists for is `FindCloseChangeNotification`, a kernel close -- while
the owning wait is being torn down. A slow one previously showed as a stalled teardown with no
explanation.

**Guarded and sabotage-verified.** The pool-object guard now asserts both halves for twelve calls;
deleting either the enter or the leave record from the macro is caught.

### <a id="m-t32"></a>M-T3.2 -- Census of OS-invoked callbacks, and whether each has entry/exit tracing. *(completed 2026-09-27 14:47:48 -04:00)*

Asked alongside M-T3.1: are there callback mechanisms besides the thread pool, and do they all have
entry/exit tracing? Searched the whole workspace for the registration APIs.

| Mechanism | Used | Entry/exit traced |
|---|---|---|
| `PTP_WAIT_CALLBACK` | yes | yes |
| `PTP_WORK_CALLBACK` | yes | yes |
| `PTP_TIMER_CALLBACK` (one-shot) | yes | yes |
| `PTP_TIMER_CALLBACK` (periodic) | yes | yes |
| `PTP_WIN32_IO_CALLBACK` | yes | yes |
| `PVECTORED_EXCEPTION_HANDLER` | yes, since M-T2.1 | yes |
| `WaitCloseFn` (caller-supplied close) | yes | **yes, added by M-T3.1** |
| `PTP_CLEANUP_GROUP_CANCEL_CALLBACK` | **no** -- `CleanupGroup` passes `None` | n/a |

Checked and **not** used anywhere in the workspace: `RegisterWaitForSingleObject`,
`QueueUserAPC` and alertable waits, `SetConsoleCtrlHandler`,
`SetUnhandledExceptionFilter`, `AddVectoredContinueHandler`, timer-queue timers, and
`InitOnceExecuteOnce`.

Two that look like callbacks and are not. `FindFirstChangeNotification` produces a *waitable
handle*, serviced by `ThreadpoolWait` -- already traced; its custom close is the `WaitCloseFn`
row above. And `ReadDirectoryChangesW` in `windows-file-watcher` is issued through
`ThreadpoolIo` in its `OVERLAPPED` form, not with an APC completion routine, so it arrives on
the already-traced `io` trampoline.

**The one gap left open deliberately:** `CallbackEnviron::set_cleanup_group` is a public raw seam
that lets a caller install their own cleanup-group cancel callback. That function would be the
caller's, not ours, so there is nothing here to bracket.

## Moved 2026-10-01 11:54:34 -04:00 -- M-T4.9, which channel carries a developer-facing report

### <a id="m-t49"></a>M-T4.9 -- Decided: a developer-facing report is a trace event, and the crate writes nothing to stderr. *(completed 2026-10-01 11:54:34 -04:00)*

The decision, its cost, and the two alternatives rejected are recorded in [A developer-facing
report is a trace event, and the crate writes nothing to
stderr](../../DESIGN-NOTES.md#reports-are-trace-events).

The item was raised because the crate had two channels and no rule. It is answered against the
argument the item itself made: it held that a trace-only report would be invisible to the
developer it is addressed to, which is true and was accepted anyway, because the visibility
`eprintln!` buys is a write into a stream the consumer owns and cannot refuse.

The conversion of `ThreadpoolIo::drop`'s `eprintln!` -- the crate's only such write -- is part of
`M-T4.3`, which emits the four new reports on the same occasion.

## Moved 2026-10-01 12:08:01 -04:00 -- M-T4.3, the undischarged-obligation report

### <a id="m-t43"></a>M-T4.3 -- Report at `Drop` when the caller did not close synchronously. *(completed 2026-10-01 12:08:01 -04:00)*

Implemented as `CloseObligation` in [src/obligation.rs](src/obligation.rs), carried on the heap
callback context of `ThreadpoolWait`, `ThreadpoolTimer`, `ThreadpoolPeriodicTimer` and
`ThreadpoolWork`, and reported as a `drop-obligation-owed` trace event. `ThreadpoolIo`'s
`eprintln!` -- the crate's only write to stderr -- was converted to the same event in the same
change, per [M-T4.9](#m-t49).

**The plan had two things wrong, and writing the code is what surfaced them.** Both are recorded
in [What the obligation flag records, and why a dispatch sometimes discharges
it](../../DESIGN-NOTES.md#teardown-drains):

- The item's table had the polarity inverted -- it set the flag on the *close* and cleared it on
  *arming*, which reports against an object that was created and never armed. The flag records
  that a drain is owed, so the quiet state is the initial one.
- The item treated the four types as one rule. They are two: a dispatch discharges the obligation
  on the wait and the one-shot timer, because each is armed for exactly one activation, and does
  not on the periodic timer (the pool re-arms it) or on work (`submit` is repeatable). Without
  that split the report fires on a wait that was armed once, ran, and was dropped.

The item also assumed the flag could live on the owning struct. It cannot: the trampoline is what
discharges it on two of the types, and a trampoline receives only the context pointer.

**Guarded in both directions and sabotage-verified five ways.** The wiring tests in
[src/obligation/tests.rs](src/obligation/tests.rs) read the flag directly rather than the trace,
because the trace's filter is fixed before `main` and a trace-reading assertion would pass
silently in an ordinary `cargo test`. Inverting `record_live` turned 9 red; inverting
`record_settled` turned 10 red; removing each trampoline's discharge turned exactly its own test
red and nothing else, which is what shows those two tests are not measuring the `stop_and_drain`
that follows. The end-to-end emission check in
[tests/obligation_report.rs](tests/obligation_report.rs) runs only under a narrowed trace and
announces a skip otherwise; deleting one type's `Drop` report turns it red.

## Moved 2026-10-01 12:30:30 -04:00 -- M-T4.10, one re-arm suppression instead of two

### <a id="m-t410"></a>M-T4.10 -- Extract the re-arm suppression that `ThreadpoolWait` and `ThreadpoolTimer` each implemented separately. *(completed 2026-10-01 12:30:30 -04:00)*

`RearmSuppression` in [src/rearm.rs](src/rearm.rs) now owns the `Mutex<u32>`, the poison
recovery, and the saturating arithmetic. Each context keeps a thin `suppress_and_disarm` that
supplies its own native call and trace record through a closure, so the per-type parts stayed
where they were and every call site is unchanged. `ThreadpoolPeriodicTimer` is still not a
client: the pool repeats its timer, so it has no deferred re-arm to suppress.

**The extraction found a hole in the existing tests, which is the part worth remembering.** The
item predicted that sabotaging the shared mechanism would turn a test red on each type. For the
*suppress* half that happened -- raising the count without disarming turned two wait tests and
two timer tests red. For the *release* half **nothing went red at all**.

The reason is recorded in [What that exemption cost the tests, and how it was
found](../../DESIGN-NOTES.md#teardown-drains): `arm` and `set_after` deliberately bypass the
suppression, so `a_wait_is_reusable_after_stop_and_drain` and its timer twin -- the tests whose
names suggest they cover this -- pass whether or not the count was ever lowered. The lift is only
observable through a callback-side re-arm after a completed drain, and no test did that.

`a_callback_can_rearm_again_after_stop_and_drain` and
`a_deferred_rearm_is_applied_again_after_stop_and_drain` close it. Both go red under a `release`
that never lowers the count, so the guard now exists on each type in both directions.

Releasing the suppression is the only behaviour distinguishing `stop_and_drain` from `Drop`,
which raises and never releases -- so it had been, until this, a mechanism whose sole
distinguishing behaviour nothing checked.

## Moved 2026-10-01 13:07:40 -04:00 -- M-T4 and M-T5, both complete

Relocated verbatim from [CHECKLIST.md](CHECKLIST.md); the only edits are the two milestone
headings, demoted from `##` to `###` so they nest under this group. The three items that were
already one-line stubs -- `M-T4.3`, `M-T4.9` and `M-T4.10` -- are not reproduced here: each was
moved to this file when it completed, and its entry above is the record.

`M-T4.8` and `M-T4.4` are **not** here. They were the two open decisions in `M-T4`; neither was
actionable in it, so they were renumbered to `M-T6.7` and `M-T6.8` and remain live work in
[CHECKLIST.md](CHECKLIST.md).

### M-T4 -- Teardown drains rather than cancels

Implements [Teardown drains rather than cancels](../../DESIGN-NOTES.md#teardown-drains), decided
2026-09-28. Forced by a measurement in the ring crate:
[closing-too-soon-after-the-disarm](measurements/2026-09-28-closing-too-soon-after-the-disarm/README.md)
shows the cancelling form leaves the default pool unable to make its first worker, 10 failures in
20000 against 0 for the draining form.

- [x] **M-T4.1** -- **DECIDED 2026-09-28: `stop_and_drain` changes rather than gaining a sibling.**
  It always should have drained; the name was right and the body was wrong. This is a breaking
  behavioural change to a published crate -- a call that discarded queued callbacks will now run
  them and block until they finish -- and ships as one. No second method.

- [x] **M-T4.2** -- **Done 2026-09-28, and the item was wrong about two of its three sites.** All
  five teardown call sites now drain (`ThreadpoolWait::drop` and `stop_and_drain`,
  `ThreadpoolTimer::drop` and `stop_and_drain`, `PeriodicTimer::stop_and_drain`, which its `Drop`
  reaches). Only the wait's two are **verifiable**, and finding that out was most of the work.

  **The wait is a real defect and is guarded.** Two new tests use a private pool capped at one
  occupied thread, which is what makes "queued but not started" deterministic instead of a race,
  and assert the callback **ran**. Both sabotage-caught by reverting to `cancel_pending`. All 234
  pre-existing tests passed *before* the change, because they assert quiescence and both forms
  satisfy it -- which is exactly why the wrong form survived.

  **The timers' change is unobservable, measured rather than assumed.** A probe found that
  `SetThreadpoolTimer(NULL)` discards an already-queued tick where `SetThreadpoolWait(NULL)` does
  not: without a disarm the queued tick ran, with one it did not. Both timer teardowns disarm
  before draining, so no queued callback survives for the drain to run and the two forms are
  identical. The change was **kept** -- it is the form the rest of the crate uses and stays correct
  if that asymmetry ever changes -- and is documented as unobservable at both call sites rather
  than left looking verified. No guard was written that could not discriminate; instead the
  asymmetry itself is pinned by
  `disarming_cancels_a_queued_tick_which_a_waits_disarm_does_not`, and sabotaged by inverting it.

- [x] **M-T4.5** -- **`CleanupGroup` already complies; no change needed.** Queued on the strength
  of a grep showing both drain forms at eight sites; reading it, those are the *member* accessors
  (`WaitMember::wait` against `WaitMember::cancel_pending`, and the same pair for work and the two
  timers) -- caller-facing choices, not teardown. The group's own teardown is
  `release_members(false)` in `Drop`, which drains, and `close_members(cancel_pending: bool)` is
  already the explicit early-release method. A member never closes itself either, so nothing in
  that path issues a close behind a disarm.
- [x] **M-T4.7** -- **Answered 2026-09-30, analytically, and the answer is NO.** The item expected a
  20000-run sweep; the question turned out to be decidable from the code. The group dispatches
  member teardown through a vtable whose wait entry includes `TppStopWaitCallbackGeneration`, which
  reaches `NtCancelWaitCompletionPacket` and **threads the caller's cancel-pending argument
  straight through to `RemoveSignaledPacket`**. So a group has exactly the same drain-versus-cancel
  structure as a standalone wait: releasing with FALSE is safe, with TRUE is not. This crate's
  `Drop` already passes FALSE, so a consumer who only drops is safe -- not because the group
  protects them, but because the default was already the safe one. Artifact:
  [which-teardowns-can-still-yank](measurements/2026-09-30-which-teardowns-can-still-yank/README.md).

  **The first answer was the opposite and was wrong**, which is recorded in the artifact because
  the failure mode generalises: a reachability walk over direct calls can prove reachability but
  **cannot prove unreachability** where dispatch is indirect, and the group's release dispatches
  through CFG-guarded indirect calls. Reading the vtable reversed the verdict. Any future use of
  that technique must check the closure for indirect calls before relying on a negative.

- [x] **M-T4.6** -- **Done 2026-09-29: the fix holds on the real path.** The 20000-run arms were a
  hand-rolled model of the teardown, not this crate's code, so the committed change had to be
  measured against `EventDelivery` itself. Two builds differing only in `ThreadpoolWait`'s teardown,
  same reproducer, same session: the reverted (cancel) build reproduces, the committed (drain) build
  does not over seven times the runs. Counts and provenance in
  [measurements/2026-09-29-the-fix-on-the-real-path/](measurements/2026-09-29-the-fix-on-the-real-path/README.md).
  Note what it does *not* establish: it excludes "the rate is unchanged", not "the rate is zero",
  and it is not a root cause. `M26.9` may be called closed on this; the open question is why the
  close-behind-disarm stalls the pool at all.

### M-T5 -- Why the pool stops making workers

Opened 2026-09-30. `M-T4` shipped a fix whose correctness is structural rather than statistical --
the drain cannot reach the primitive that does the damage, so it holds however the timing falls --
but the *cause* is still open. What is established is in
[what-the-disassembly-says](measurements/2026-09-30-what-the-disassembly-says/README.md)
and [STALL-TIMELINE.md](STALL-TIMELINE.md): all four teardown paths converge
on `NtCancelWaitCompletionPacket`, differing only in whether they ask it to remove an
already-delivered packet, and the drain never calls it at all.

Two hypotheses have already been killed by evidence that existed before they were proposed -- "the
queued callback must have run" (refuted by a graded gap sweep) and "a creation-in-progress gate is
stuck" (refuted by a 2026-09-27 capture recording that counter as 0). Treat any third with the
same suspicion, and look for a disconfirming measurement before building on it.

**CLOSED 2026-10-01, by decision rather than by arrival at an answer.** Three hypotheses were
refuted in the end, the third by two flags the capture had been decoding and discarding for days.
What the milestone established is the fault's *shape* -- the port is healthy, the factory is
healthy, and the notification between them is lost -- plus its preconditions, its blast radius, and
what recovers it. What it did not establish is **why**, which is inside the kernel's
queue-to-factory notification and beyond any instrument available here.

The engineer's judgement was that continuing would require fixing the kernel seam, and that the
pattern is now understood well enough to avoid. So the remaining questions -- `M-T5.2`, `M-T5.3`,
`M-T5.4`, `M-T5.10` -- are closed as not-pursued, each with its reason recorded rather than left
looking like an unfinished measurement. **None of them gates the remedy**: the drain is structural
and does not depend on the mechanism, and `M-T6`'s self-heal repairs by a route measured to work
whatever the mechanism turns out to be.

Standing lesson, earned three times: **emit more of what is already in hand before reasoning about
what is not.**

- [x] **M-T5.1** -- **Done 2026-09-30: the work is queued and the pool is idle beside it.** Depth
  **2** on the pool under test, in 15 captures of 15 -- exactly the two victims -- while that pool
  reports 0 workers, `may_create` 1 and `create_in_progress` 0. So the fault is **not** in delivery:
  the packet reaches the port, the factory was entitled to make a thread, and it did not. Also kills
  the benign reading of those counters, which had been consistent with a factory correctly seeing no
  work. Artifact:
  [the-work-is-queued-and-the-pool-is-idle](measurements/2026-09-30-the-work-is-queued-and-the-pool-is-idle/README.md).
  The instrument (`trace::completion_port_depths`) has a sabotage-verified positive control, which
  mattered here because a silently broken probe reports "depth 0" -- the finding that would have
  sent the investigation the other way.

- [x] **M-T5.2** -- **CLOSED 2026-10-01: answered as far as measurement reaches.** `M-T5.6`
  established the shape -- the port is healthy, the factory is healthy, and the notification
  between them is lost -- and that is the end of what any instrument available here can see. The
  remaining "why" is inside the kernel's queue-to-factory notification, and the engineer's decision
  was to stop there and address the fault by repair (`M-T6`) rather than by prevention.

  **Closing this does not weaken the fix.** The drain is structural and does not depend on knowing
  the mechanism; self-heal repairs by a route measured to work regardless of it. What is given up
  is the explanation, not the remedy.

  Original text follows; its property 5 is refuted and the refutation is part of the record.

- [x] ~~**M-T5.2 (original)** -- Establish what prompts a factory to create a worker after work is queued.~~
  **Ungated 2026-09-30: `M-T5.1` returned depth 2, so this is now the live question** -- the create
  test would approve, because the port is non-empty, so the stall is not a decision to decline. It
  is the absence of the question.

  **Narrowed 2026-09-30, and property 5 below is REFUTED.** Emitting two flags the capture had
  always decoded and thrown away -- queued-for-deferred-create, and deferred-timer-armed -- shows
  both **0** in 14 stalls of 14. So nothing is pending and nothing is scheduled to ask again. The
  stalled factory reads as *perfectly idle in every field*; the only thing distinguishing it from a
  factory with nothing to do is the two packets on its port. That is a simpler and stronger
  statement than the wedge it replaces: not a creation that got lost, but a prompt that never
  happened. Artifact:
  [nothing-ever-asks-the-factory](measurements/2026-09-30-nothing-ever-asks-the-factory/README.md).
  Next measurement is `M-T5.6`.

  Our own measurements already bound the answer: a
  healthy run creates a worker 0.24-0.31ms after the delivery is armed, so something on the
  queueing path does prompt it; a stalled run never does, and the only call ever observed to
  release the stall is `NtReleaseWorkerFactoryWorker` from the work-submit path, which reaches the
  factory by a different route than queued work does. That asymmetry is the thing to explain.

  **What the create decision looks like, and why it narrows the search.** The relevant routines are
  named in the public symbols -- `ExpWorkerFactoryCheckCreate`, `ExpWorkerFactoryWantsToCreate`,
  `ExpWorkerFactoryCreateThread`, `ExpSetWorkerFactoryDeferredCreateTimer`,
  `ExpWorkerFactoryManagerThread` -- alongside globals for a creation state, a deferred-creation
  list, and short, medium and long deferral timeouts. Their structure is readable by disassembly
  (the public PDB carries these names but no struct layouts, so field *names* are not available and
  nothing below depends on one).

  **Nomenclature.** The factory's per-instance fields are reachable only as offsets, so the names
  below are **ours, assigned for this investigation**, not the platform's. They are written in
  `snake_case` to keep that visible. The sole exception is `create_in_progress`, which is a real
  field of the public `WORKER_FACTORY_BASIC_INFORMATION` and is already what
  `src/trace/hook.rs` recorded (that file was deleted with the crate's inline hooking facility, under the
  repository's rule against undocumented APIs).

  Four properties matter for this investigation:

  1. **The create test has two independent triggers.** One is work outstanding on the completion
     port; the other is a count of user-mode release requests. `NtReleaseWorkerFactoryWorker`
     arrives on the second. Removing a delivered packet zeroes the first and leaves the second
     untouched -- which is precisely the asymmetry measured between queued work (never recovers)
     and the submit (always recovers).
  2. **A one-at-a-time gate is tested before either trigger**, so a creation believed to be in
     flight suppresses all others. That was the obvious wedge and it is **already refuted**: the
     2026-09-27 captures record `create_in_progress` as 0 in stalled processes.
  3. **The deferral path is built to self-heal.** Three policies can decline a create; each keeps
     its own `policy_retry_level`, which escalates across two deferrals and then causes that policy
     to be skipped outright, forcing the create. So a factory cannot be wedged by a policy that
     keeps saying no -- which is what makes "the factory is never asked again" the remaining shape,
     and why `M-T5.1`'s queue depth is the measurement that matters.
  4. **A transient empty queue can erase the justification for a create that is already pending.**
     This is the interaction that makes the fault plausible at all, and it is a two-step:
     - A deferred request treats "every `policy_retry_level` is clear" as meaning the work was
       already picked up by some existing worker, and returns **without creating**.
     - The basic test declining **clears every `policy_retry_level`** on its way out.

     Those retry levels are the only record that a thread is still wanted. So if the basic test
     runs while the queue happens to be empty, it wipes the justification a pending deferred
     request was going to rely on, and that request then cancels itself.

     The design reads "queue empty" as "the work was consumed". **Removing a delivered packet makes
     consumed and destroyed indistinguishable** -- the same class of aliasing the platform already
     documents elsewhere on this path, where a cancel-then-reassociate to the same port cannot be
     told from the original association. Our teardown manufactures exactly that ambiguity, a few
     microseconds after the work is queued.

  5. **REFUTED 2026-09-30. `queued_for_deferred_create` was the prime suspect and it reads 0.** The
     hypothesis was that a factory stays flagged as queued for a creation nobody services, wedging
     it permanently. Both that flag and the deferred-timer flag read **0** in 14 stalls of 14, so
     no creation is pending and none is scheduled. Kept here, refuted rather than deleted, because
     it was the third hypothesis this investigation has lost and the pattern is worth preserving:
     each was killed by data that either already existed or cost one record to emit. Property 4's
     two-step remains *unrefuted but unsupported* -- nothing measured bears on it either way, and
     it should not be leaned on.

     **The standing lesson: emit more of what is already in hand before reasoning about what is
     not.** These two flags had been decoded into the capture struct and discarded on every run for
     three days, while the hypothesis they refute was being built.

  Verify this structure against the shipped binary before building on it, rather than carrying it
  forward as an assumption: it was read once, and `M-T5.5` may invalidate it.

- [x] **M-T5.3** -- **NOT PURSUED, by decision 2026-10-01.** The question only mattered for
  locating the mechanism, and the engineer's decision was to stop at the kernel seam and address
  the fault by repair instead (`M-T6`). Anchoring the window more precisely would not change the
  repair, the drain, or anything a consumer does. Recorded rather than deleted because the arms
  were built and verified, so anyone who later wants the answer starts from a known position
  rather than from scratch. Original text follows.

- [x] ~~**M-T5.3 (original)** -- Is the hazard window anchored to the queueing or to the disarm?~~ A run was
  built and started for this and stopped at 45% to free the machine; redo it when a quiet machine
  is available. Two arm families place the packet removal the same distance after `SetEvent` while
  putting the delay on opposite sides of the disarm: `SetEvent -> disarm -> spin N -> close`
  against `SetEvent -> spin N -> disarm -> close`. Coinciding curves say the window is anchored to
  the queueing; a flat second family says it is anchored to the disarm. Both families were verified
  to place the removal at matching times (4-5us, 12us, 32us) before the run started, so the arms
  are ready to rebuild.

- [x] **M-T5.4** -- **CLOSED 2026-10-01: blocked, and no longer needed.** It was queued to settle
  `M-T5.2` by reading the factory's state directly. `M-T5.2` is now closed as far as measurement
  reaches, and the investigation is not continuing past the kernel seam, so the blocker no longer
  gates anything. The firmware finding below stands and is worth keeping -- it is the reason no
  kernel-level answer was available to this investigation at all, and anyone who revisits the
  question will hit the same wall. Original text follows.

- [x] ~~**M-T5.4 (original)** -- local kernel debugging is unavailable on this machine.~~ Reading the factory's own state directly would settle `M-T5.2` outright, and
  `kd -kl` is the tool for it. It is blocked by a **firmware** condition rather than a missing
  step: `bcdedit -debug on` fails with "The value is protected by Secure Boot policy", and the
  machine reports Secure Boot enabled with VBS running and Credential Guard active. Enabling it
  needs Secure Boot disabled in UEFI, which is a real security downgrade and may be policy
  forbidden. Two further cautions if it is ever revisited. Local kernel debugging is **read-only**,
  so it cannot set the kernel's thread-pool debug-print mask (the symbol exists, and the component
  id is 84) -- capturing that narration would additionally need a registry filter and a
  kernel-print capture. And **boot-debug mode perturbs what is being measured**: the signal is a
  2-6us race at about one run in a thousand, so a configuration change that alters kernel timing
  could mask it while appearing to test it. A clean result under debug boot is not evidence.

- [x] **M-T5.5** -- **Done 2026-09-30: the update moved the kernel, and nothing else.** ntoskrnl
  went 10.0.26100.9444 -> **10.0.26100.9457**; ntdll is **unchanged** at 10.0.26100.9278, so the
  committed disassembly still describes the shipped binary. Every worker-factory routine the
  analysis rests on is still present in 9457, and the reproducer's rate is unchanged (15 stalls in
  8400 on `hand-spin-3us`, against the 4.15 per thousand measured before the update). Later work is
  measured against 9457.


- [x] **M-T5.6** -- **Done 2026-09-30: arrivals no longer reach the factory.** The posted packet
  lands -- depth 2 -> 4, so the port accepts it -- and **no worker is created**, in 13 captures of
  13. The same poke on a healthy factory in the same starting position takes it from 0 workers to 1
  and consumes the packet, so the stimulus is valid and the null result is a property of the
  stalled process. Artifact:
  [arrivals-no-longer-reach-the-factory](measurements/2026-09-30-arrivals-no-longer-reach-the-factory/README.md).

  **This answers `M-T5.2` and dissolves the standing asymmetry.** Port healthy, factory healthy,
  and the notification between them persistently gone -- a packet posted by hand five seconds into
  the stall is ignored exactly as the originals were, so nothing was special about the victims'
  inserts. `NtReleaseWorkerFactoryWorker` recovers the stall every time because it reaches the
  create decision by the other route, which does not depend on the severed link. The fault's shape
  is now: a per-port, persistent loss of the arrival-to-factory notification, caused by removing a
  delivered packet a few microseconds after it was queued, on a port whose factory has no threads
  yet. Why that link breaks is inside the kernel and out of reach here.

- [x] **M-T5.7** -- **Done 2026-09-30.** The retry timeout, infinite-wait goal, start routine and
  parameter, process id, and stack reserve/commit are now emitted alongside the rest. None proved
  decisive this time -- `M-T5.6` answered the question first -- but they are in every future
  capture at the cost of a handful of records, which is the point: the guessing is over.

- [x] **M-T5.8** -- **DECIDED 2026-10-01; superseded by `M-T6`.** The answer is not removal:
  cancellation stays, renamed `try_cancel_pending` to connote the best-effort attempt the platform
  has always actually performed, and backed by a repair that submits a work item to the affected
  pool. Safe method gated on the `self-heal` feature, `unsafe`
  `try_cancel_pending_no_heal_tracking` always present so that disabling the feature breaks call
  sites loudly rather than silently removing a guarantee. Decision:
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md#cancellation-self-heals). Original text follows, and its
  analysis stands -- in particular that removing `cancel_pending` would not have removed the
  hazard, since the close makes the same call.

- [x] ~~**M-T5.8 (original)** -- the removal is in the close, not only in `cancel_pending`.~~

  > **CORRECTED 2026-09-30, same day it was written.** The first version of this item claimed
  > `cancel_pending` is uniquely dangerous and asked whether to remove it. That premise is **false**
  > and this workspace's own committed data said so before the item was written:
  > [cancel-and-gap-are-both-required.csv](measurements/2026-09-29-what-the-gap-is-made-of/cancel-and-gap-are-both-required.csv)
  > records `hand-nocancel` -- an arm that **makes no cancel call at all** -- failing 22 times in
  > 20004, against `hand-control`'s 16 with the cancel. Dropping the cancel changes nothing
  > measurable, because `CloseThreadpoolWait` performs the same removal, through the same kernel
  > routine (`IopCancelWaitCompletionPacket`) with the same `RemoveSignaledPacket` flag, whenever it
  > finds a packet still outstanding.
  >
  > **So removing `cancel_pending` would not remove the hazard**, and the four options the item
  > originally offered were all answers to the wrong question.

  What the measurements actually support: the removal happens at whichever call first finds a
  delivered packet. `cancel_pending` does it if called; otherwise the close does it, and **every**
  wait teardown ends in a close. The hazard is the removal landing a few microseconds after the
  packet was queued, on a port whose factory has no threads yet.

  That makes the shipped fix the *only* shape of fix available, rather than one option among
  several: a drain lets the queued callback run, which clears the association, after which the
  close has nothing to take. It does not avoid the dangerous call -- it empties it.

  The decision that remains is narrower and is about surface rather than safety:

  1. `cancel_pending` still exists on waits and wait members, and its honest description is now
     "performs the teardown's removal earlier, removing the chance for the callback to run first."
     That is a much less attractive proposition than its name suggests, and arguably has no
     remaining use case -- but it is not the hazard's cause and removing it buys no safety.
  2. The cost is documented on both methods as of this commit. Prose is not a rung on the detection
     ladder, so if the surface is kept, consider whether anything stronger is wanted.

  Still coupled to **M-T6.8** and **M-T6.7** (raised in `M-T4` as `M-T4.4` and `M-T4.8`, renumbered
  2026-10-01), and still the engineer's call rather than an assistant's.

  The audit's exposure table remains correct as written -- every default path is safe, because
  `Drop` and `stop_and_drain` drain and the group releases with false -- but note *why*: not
  because those paths avoid the removal, but because they leave nothing for it to remove. The audit
  in
  [which-teardowns-can-still-yank](measurements/2026-09-30-which-teardowns-can-still-yank/README.md)
  maps every remaining path that can remove a delivered packet, and its table stands.

- [x] **M-T5.9** -- **Done 2026-09-30: severity characterised, and the "end of execution" reading is
  wrong.** The fault did not present at the end of execution -- the trigger finished, and the
  damage blocked work that arrived afterwards. Continued use does **not** hide it: a fresh wait, a
  fresh timer and a completed overlapped read each leave the pool stalled for a full two-second
  window. Only a work submit recovers it, and recovery is complete -- a brand-new wait armed after
  it dispatches in microseconds, in 63 captures across six experiments. Artifact:
  [what-a-process-does-after-the-stall](measurements/2026-09-30-what-a-process-does-after-the-stall/README.md).

  **The consequence worth carrying forward is that the fault is camouflaged, not benign.** A
  program that submits work items near its waits sees a latency spike bounded by the interval to
  the next submit -- easy to mistake for scheduler jitter. A program using only waits, timers and
  I/O has no stimulus that will ever recover it, and hangs. This workspace's reproducer is the
  second kind, which is the only reason the fault was ever seen rather than shrugged off.

- [x] **M-T5.10** -- **CLOSED 2026-10-01: unanswerable from here, and accepted.** The engineer's
  judgement was that this is unanswerable without the kernel seam, and the investigation stopped
  there. It is closed as a **decision**, not because an answer arrived: on everything known, a pool
  whose last worker retires becomes vulnerable again, and `pool::prewarm_default_pool` therefore
  narrows a window rather than removing a cause.

  **`M-T6` is what makes that acceptable.** Self-heal bounds the damage without needing to know
  whether the notification loss is permanent or edge-consumed -- it repairs by submitting work,
  which recovers the pool either way. The design deliberately does not assume an answer to this
  question, and the accepted residual is that the self-heal pool could in principle share the
  fault.

  Original text follows, including an instrument that must not be retried as written.

- [x] ~~**M-T5.10 (original)** -- Does the pool break again once it returns to zero workers?~~ The one
  severity question `M-T5.9` could not close. Every capture observes a pool that still holds the
  worker its recovery created, because the idle timeout is 67s and no capture runs that long. If
  the notification is permanently lost rather than edge-consumed, a long-lived process is in a
  permanent stop-start state -- working while a worker happens to be alive, stopping each time the
  pool drains -- rather than having had one bad moment.

  **The obvious experiment hangs and must not be retried as written.** Draining the stalled pool's
  completion port with `GetQueuedCompletionStatus` at a zero timeout, to make a genuine
  empty-to-non-empty transition, does not return: six workers sat fifteen minutes with no progress
  and captured nothing. The instrument was removed rather than kept behind a warning. A plain
  `GetQueuedCompletionStatus` is not a safe way to inspect a thread pool's own port whatever the
  timeout says; `NtQueryIoCompletion` reads the depth without disturbing it, which is what the
  surviving instruments use.

  Viable alternative: recover a stalled process with a work submit, wait past the 67s idle timeout
  for the worker to retire, then arm a fresh wait and time it. Slow -- a handful of captures at a
  minute-plus each -- but it needs no new primitive and answers the question directly.

- [x] **M-T5.11** -- **Done 2026-09-30: a warm pool does not stall. This is a cold-start hazard.**
  Warming the pool first -- one work item, its callback confirmed to have run, so a worker provably
  exists -- gives **0 failures in 24000** against a cold control's **66**. A delay-matched cold arm
  that pays 400us without warming (more than warming's measured 223-304us) still fails at the cold
  rate, so it is the worker and not the elapsed time. Every warm run is individually confirmed, and
  the arm aborts rather than proceed if its warm-up fails. Artifact:
  [a-warm-pool-does-not-stall](measurements/2026-09-30-a-warm-pool-does-not-stall/README.md).

  **Exposure is therefore bounded**: near process start, before the pool's first dispatch, and
  after each idle-timeout expiry when the last worker retires (67s for the default pool). A process
  keeping its pool busy is not exposed between those points. This fits the mechanism -- the severed
  notification is the one asking the factory to *create* a worker, and a pool with a thread already
  parked does not need that question asked -- but the fit is corroboration, not proof.

  Does **not** establish that a warm pool is unreachable by any trigger, only by this one at this
  rate over 24000 runs. Changes nothing about the fix: draining remains correct regardless, and
  this bounds when a *non*-draining teardown is dangerous.

## Moved 2026-10-01 13:29:21 -04:00 -- M-T6.1, the self-heal feature and pool registry

### <a id="m-t61"></a>M-T6.1 -- Add the `self-heal` feature, default on, and the pool registry. *(completed 2026-10-01 13:29:21 -04:00)*

`self-heal` is a default-on feature; [src/heal.rs](src/heal.rs) holds the registry. Each entry
carries the last-dispatch stamp, the repair-owed stamp, a live-object count, and a work object
created **at registration**, because creating one was measured not to release a stall and only
submitting one is. Every object type -- work, wait, one-shot timer, periodic timer, I/O --
registers its pool at construction, keyed by the `PTP_POOL` the caller's environment names, with
zero meaning the process default. The registration lives on each object's heap context so a
trampoline can reach it in `M-T6.2` and so it survives `into_parts` into a cleanup-group member.

**The decision inside the item came out the other way.** The item expected the default pool to
need a retention rule of its own. It does not: *retire an entry when it has no objects and owes
no repair* covers both kinds, and the argument is recorded in [How long an entry lives, and why
the default pool needs no rule of its own](../../DESIGN-NOTES.md#cancellation-self-heals).

The part the plan did not anticipate is that the second clause is **required** for a private pool
too, not merely harmless. The entry's repair object is created against its pool, and a pool is not
freed while an object bound to it lives -- so retaining an entry that owes a repair is also what
keeps the pool alive to receive it. The suggested special-case would have been both unnecessary
and, applied as stated, wrong for the private case.

**Best-effort registration, deliberately.** If the repair object cannot be created the pool goes
unregistered and object creation still succeeds, because failing it would turn an unrelated
allocation failure into a failure of the caller's actual request. What `try_cancel_pending` does
when its pool has no entry is left to `M-T6.3`, which owns that method.

**Sabotage-verified three ways**, each hitting exactly its own guard: registering without a repair
object turned 10 red; retiring an entry that still owed a repair turned exactly
`an_entry_owing_a_repair_outlives_its_last_object` red; and letting a later cancellation overwrite
an earlier one turned exactly `the_first_cancellation_is_the_one_remembered` red. The tests assert
properties of the entry they own rather than absolute registry counts, because the registry is
process-wide and `cargo test` runs these as threads in one process.

## Moved 2026-10-01 13:46:01 -04:00 -- M-T6.2, stamping every dispatch

### <a id="m-t62"></a>M-T6.2 -- Stamp the last dispatch in every trampoline. *(completed 2026-10-01 13:46:01 -04:00)*

Every trampoline calls `Registration::stamp_dispatch` before invoking the caller's closure: one
counter read and one relaxed store. Before rather than after, so a dispatch that is still running
counts as evidence the pool is live and a long callback does not look like silence.

**Five trampolines, not the four the item named.** It listed work, wait, timer and I/O; the
periodic timer has its own trampoline and is a fifth. All five stamp.

`QueryInterruptTime` is the clock, and the reasoning -- including that reading `KUSER_SHARED_DATA`
directly would be the same read bound to a layout nothing promises -- is in [Which clock the two
stamps are on, and which way its error falls](../../DESIGN-NOTES.md#cancellation-self-heals). That
section also records the resolution finding: the counter advances on the system tick, so a
dispatch and a cancellation in the same tick carry equal stamps and the repair is submitted
anyway. A redundant repair costs one submission; a suppressed one leaves a pool stalled, so that
is the direction the error has to fall.

**The sabotage found an unguarded dispatch kind, which is the part worth keeping.** Making the
shared stamp a no-op turned all five stamping tests red, and moving work's stamp after the
callback turned exactly the ordering test red. But deleting **only** the I/O trampoline's stamp
turned *nothing* red -- across 272 lib tests, every integration test and the doctests.

The obvious place for an I/O assertion is the existing real-read test, which runs on the default
pool -- and the default pool's entry is stamped by every other test in the binary, so an assertion
there would have passed with the stamp deleted. `an_io_completion_stamps_its_pool` uses a private
pool nothing else touches, and asserts the entry starts at zero before submitting. Re-injecting
the same sabotage now turns exactly that test red.

## Moved 2026-10-01 14:30:28 -04:00 -- M-T6.3, the cancellation rename and its unsafe sibling

### <a id="m-t63"></a>M-T6.3 -- Rename to `try_cancel_pending`, and add the ungated `unsafe` sibling. *(completed 2026-10-01 14:30:28 -04:00)*

`ThreadpoolWait` and `WaitMember` each lose `cancel_pending` and gain a pair: `try_cancel_pending`,
gated on `self-heal`, which marks its pool as owing a repair; and
`try_cancel_pending_no_heal_tracking`, `unsafe` and always present, which does not. The contract is
identical in both feature states -- best-effort cancellation, the pool may stall briefly -- and only
the repair latency differs, so it is not documented as a behavioural difference.
`CleanupGroup::close_members(true)` marks each wait member's pool through a new per-kind hook on
`OwnedResource`.

**Waits only, and the crate already says why.** The item named `WaitMember` and `close_members`
and no other type, which matches what `TimerMember`'s own documentation records: the removal that
can sever a pool's arrival-to-factory notification operates on a *wait completion packet*, and only
a wait owns one. `ThreadpoolWork`, `ThreadpoolTimer` and the work/timer members keep
`cancel_pending` unchanged. Whether that leaves the surface inconsistent is `M-T6.7`'s question.

**Doc links had to avoid the gated method.** A link to `try_cancel_pending` dangles in a build with
the feature off, so the five intra-doc links that pointed at the old name now point at
`try_cancel_pending_no_heal_tracking`, which is always present, and name the safe one in prose.
Verified with `cargo doc` in both feature configurations, which `cargo check` does not cover.

**Running the sabotage harness found three cases broken by this session's own commits**, none
noticed when they broke. Two are fixed here -- the `stop_and_drain` case whose `find` block
`M-T4.3` split by inserting a line, and the handle-scan case whose single-line `find` stopped being
unique when a second `const CEILING` arrived with the completion-port scan. The third, *the factory
layout is truncated*, is queued as `M-T6.9`.

## Moved 2026-10-01 15:32:42 -04:00 -- M-T6.9, the last mis-declared sabotage case

### <a id="m-t69"></a>M-T6.9 -- Repair the one sabotage case still declared wrong. *(completed 2026-10-01 15:32:42 -04:00)*

The case was *the factory layout is truncated*, reporting `MANIFEST DOES NOT COMPILE`.

**It was neither of the two possibilities the item anticipated.** Not a stale `find`, and not a
sabotage being caught by a `const` assertion -- the check the item asked for before repairing.
Applying the patch by hand and reading the compiler output gave five `E0609: no field ... on type
Basic` errors: the five fields it deletes had **acquired readers** when the factory snapshot began
emitting them, so the patch became a compile error instead of the runtime failure it exists to
produce.

Repaired by keeping the defect and changing how it is reached. The recorded defect is a struct of
the wrong *size*, which makes the query reject every call; the case now pads the struct through
`_tail`, which nothing reads, rather than deleting fields that now have consumers. Renamed to *the
factory layout is the wrong size* to match. Verified: the repaired case reports `caught`.

**The full sweep then found a regression the item had not asked about**, which is why it was worth
running rather than stopping at the one case. Two wait-drain sabotages now report `survived`, and
one of them was `caught` three commits earlier. Queued as `M-T6.10` with everything already
measured, including that the callback runs rather than being discarded and that it is not
cross-test interference.

**Manifest health at this commit:** 12 of 14 cases behave as declared, both controls included. The
two exceptions are `M-T6.10`.

## Moved 2026-10-01 15:39:55 -04:00 -- M-T6.10, the guard that proved less than it claimed

### <a id="m-t610"></a>M-T6.10 -- The two wait-drain sabotages stopped detecting; cause found and both guards restored. *(completed 2026-10-01 15:39:55 -04:00)*

The second of the two readings the item named is the right one: **the guards were always
timing-dependent**, and unrelated work shifted the timing enough to expose it. The full account is
in [A test that needs a pool thread occupied must wait for the occupier to
run](../../DESIGN-NOTES.md#teardown-drains).

**The cause, in one line:** `submit` queues, it does not dispatch, and nothing waited for the
occupying work item to actually start -- so the occupier and the wait's callback were two queued
items on a one-thread pool and the pool could run either first.

The `assert_eq!(ran, 0)` that was meant to establish the precondition cannot detect that: it is
equally true when the occupier has not started, which is exactly the case it needed to exclude.

**How it was separated from the other reading.** A probe reading the counters back by asserting
against a wrong value, with the cancelling sabotage applied: one run gave
`(occupier_entered, ran_before_drop, ran_after_drop) = (1, 1, 1)` -- the callback had already run
before `Drop` -- and the next, with thread-id probes, gave `(0, 0, <tid>)`: the occupier never
entered at all. Non-determinism between consecutive runs of the same binary ruled out any
explanation that depended on a specific change in `M-T6.1` or `M-T6.2`.

**Fixed** by blocking until the occupier signals entry, in both tests. Verified three ways: both
pass clean; both fail under their own sabotage; and twenty-five consecutive runs pass, where the
old form could not be relied on for one.

**This was not a regression introduced by `M-T6.3`.** The guard had been reporting `caught` for
weeks while proving less than it claimed. What changed was the timing, not the strength of the
test -- so the earlier `caught` results were luck rather than evidence.

## Moved 2026-10-01 15:49:10 -04:00 -- M-T6.4, the self-heal timer

### <a id="m-t64"></a>M-T6.4 -- The self-heal timer. *(completed 2026-10-01 15:49:10 -04:00)*

A periodic timer on a private pool, created lazily on the first cancellation, so a consumer who
never cancels creates no pool and no thread. Each tick walks the registry: a pool that has
dispatched since its cancellation is skipped, because that is direct evidence it is still
delivering callbacks; otherwise the pre-created repair item is submitted. Either way the mark is
discharged, and `retire_idle` then retires entries kept alive only by it.

The cadence -- 250 ms with a 250 ms coalescing window -- and the decision that the healer never
stops once started are recorded in [The healer's cadence, and why it never stops once
started](../../DESIGN-NOTES.md#cancellation-self-heals), together with why stopping is not safe
without a lock the cancellation path should not pay for.

**Both guards were vacuous when first written, and the sabotage is what showed it.** The tick
clears the owed mark whether it submits a repair or decides to skip one, so a test that waited on
the mark passes under *either* mutation. The end-to-end test would have gone green with the
submission deleted, and the skip test would have gone green with the liveness check removed.

Fixed by counting repairs that actually **ran** -- a counter in the repair trampoline rather than
a trace record, because the trace is narrowed by an environment variable that neither the harness
nor CI sets, which is the same trap this crate's sabotage manifest already documents. With that,
deleting the submit turns the end-to-end test red, and making the liveness check never fire turns
the skip test red.

## Moved 2026-10-01 16:31:04 -04:00 -- M-T6.5, the sabotage that matters

### <a id="m-t65"></a>M-T6.5 -- Guard it, with the sabotage that matters. *(completed 2026-10-01 16:31:04 -04:00)*

All three load-bearing claims are now sabotage-verified **through the manifest**, not only by hand,
which is the distinction `M-T6.9` was about: a hand-verification is discarded, and only
[sabotage.json](sabotage.json) re-runs.

| claim | sabotage | result |
|---|---|---|
| a cancellation arms a repair | the cancel never marks its pool | 3 tests red |
| a dispatch after it suppresses the repair | the stamp never updates | caught |
| the repair submits a **pre-created** object | the heal creates a fresh one and submits that | caught |

**The third claim had no guard at all, and the counter written for `M-T6.4` could not have given
it one.** That counter was process-wide, so it was satisfied by *any* repair running -- including
one from a fresh object, which is exactly the mutation this claim forbids, and including another
test's repair, since the registry is shared and these run as threads in one process.

The fix carries both: the counter moved onto the entry and is reached through its work object's
own callback **context**, so only that entry's pre-created object can raise it. A fresh object
carries a different context and cannot.

**The first attempt at that sabotage was scored `caught` for the wrong reason** and is recorded in
the manifest so it is not re-introduced. Giving the fresh work object a null context made the
trampoline dereference it and abort the process, which the harness scores as caught -- but by a
crash, not by a test noticing, and this repository treats a crash-caught mutant as uncovered. The
case now gives the fresh object its own valid counter, so it fails deterministically on an
assertion instead.

## Moved 2026-10-01 16:47:46 -04:00 -- M-T6.6, the self-heal-off build

### <a id="m-t66"></a>M-T6.6 -- Verify the `self-heal`-off build. *(completed 2026-10-01 16:47:46 -04:00)*

The item asked for the shape of the off build to be **asserted rather than assumed**, which it now
is at three rungs:

- **The build.** A `const` assertion that the feature-off `Registration` is zero-sized. Five types
  carry it as a field and five trampolines call its methods unconditionally, so a byte there would
  charge every one of them for a feature that is off. Verified load-bearing: giving the type a
  `u8` fails compilation with the assertion's own message.
- **Doctests.** `FeatureShapeDoctests` in [src/heal.rs](src/heal.rs) asserts the safe method exists
  when the feature is on, that the `unsafe` sibling exists either way, and -- via `compile_fail` --
  that a call to a method the type does not have is an error. The `compile_fail` case names a
  *made-up* method deliberately: one naming the real gated method would start passing for the wrong
  reason as soon as the feature was on.
- **CI.** A `threadpool-no-self-heal` job, mirroring the existing `--no-default-features` jobs for
  `windows-ioring-sys` and `windows-placement-probe`.

**The CI job was the necessary part, and it caught a defect on its first run.** Nothing else here
builds this configuration: every `--workspace` step takes the default set, and `--all-features`
turns the feature back on, so `--no-default-features` became unbuilt the moment `self-heal` was
made default-on in `M-T6.1`.

What it found was mine, from `M-T6.3`: both `try_cancel_pending_no_heal_tracking` doc comments
linked `Self::try_cancel_pending`, which does not exist in the build those methods exist *for*.
That commit's message claims the links were verified with `cargo doc` in both configurations --
true, and insufficient, because plain `cargo doc` reports a broken intra-doc link as a **warning**.
The job runs it with `-D rustdoc::broken_intra_doc_links`, which is what turns it into a failure.
Both links are now plain code spans with the reason recorded at each site.

## Moved 2026-10-01 17:12:52 -04:00 -- M-T6.7, the name of the synchronous close

### <a id="m-t67"></a>M-T6.7 -- Decided: `stop_and_drain` is the name, added to the four types that lacked it, with `run_down` and `close_members` deliberately left alone. *(completed 2026-10-01 17:12:52 -04:00)*

Decided by the engineer; the rule and what was rejected are in [`stop_and_drain` is the name of the
synchronous close](../../DESIGN-NOTES.md#stop-and-drain-is-the-name). Additive: `ThreadpoolWork`,
`WorkMember`, `TimerMember` and `WaitMember` gained the method, nothing was renamed.

**The inventory found a missing capability, not just a naming inconsistency.** `TimerMember` and
`WaitMember` had no synchronous close at all, while `PeriodicTimerMember` did and both of their
standalone twins did -- so moving an object into a cleanup group silently lost the method that
makes its teardown deterministic, despite `create_timer` and `create_wait` being documented as
equivalent to the standalone constructors. That is why the member additions carry the real
suppression rather than aliasing `wait`.

To keep the member and standalone forms from drifting, each type now has **one** body:
`stop_and_drain_parts` takes a detached context, and the standalone method calls it.

**One of the two new tests was vacuous, caught by sabotage, and failed for the reason `M-T6.10`
recorded.** Dropping the suppression from both member drains turned the wait test red and left the
timer test green: its callback returned immediately, so the drain could land between firings where
a plain `disarm` also leaves the timer idle. Both callbacks now sleep, so the drain begins while
one is in flight and the re-arm it asks for is one the suppression has to discard -- which is the
pattern the standalone tests already used. With that, the same sabotage turns both red.

## Moved 2026-10-01 17:20:28 -04:00 -- M-T6.8, how the fail-fast is selected

### <a id="m-t68"></a>M-T6.8 -- Decided: the fail-fast is a default-off `fail-fast` Cargo feature that arms it directly. *(completed 2026-10-01 17:20:28 -04:00)*

Decided by the engineer. The rule, the accepted cost, and what was declined are in [The teardown
fail-fast is a default-off Cargo feature that arms it
directly](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature). The implementation is
`M-T6.11`.

**This item asked only for the mechanism, and that is all it settled.** Two of the three
sub-questions it carried are now answered -- off by default, and selected by a Cargo feature. The
third, what happens when the object is dropped on an already-unwinding path where a panic aborts,
is deliberately still open and moves to the implementation item.

**The discussion is worth keeping because the rejected options were rejected for different
reasons.** The inverse polarity -- on by default with a feature to turn it off -- is not merely
undesirable but *inexpressible*: Cargo features cannot be subtracted, so a consumer could never
say "not for me". Gating availability rather than behaviour, which would have removed the
unification leak entirely by keeping the fail-fast inert until the application armed it, is a
viable design that was declined in favour of one mechanism rather than two.

## Moved 2026-10-01 20:42:11 -04:00 -- M-T6.11, the teardown fail-fast

### <a id="m-t611"></a>M-T6.11 -- Implement the teardown fail-fast: all five owning types panic at `Drop` when a drain is owed, after draining, and the whole workspace was made conformant. *(completed 2026-10-01 20:42:11 -04:00)*

The mechanism is settled by [The teardown fail-fast is a default-off Cargo feature that arms it
directly](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature): a `fail-fast` feature, off
by default, which when enabled arms the fail-fast with no second runtime switch. The accepted
cost -- feature unification means any crate in the graph enabling it changes teardown behaviour
for every crate in the graph -- is recorded there, including why the inverse polarity was
rejected outright and why gating availability instead was declined.

**What to build.** At `Drop`, when the obligation flag says a drain is owed, fail fast instead of
reporting. `M-T4.3` already put that flag on every type and deliberately kept it outside the
`trace` feature, so the fact is there without new bookkeeping; what changes is what `Drop` does
with it.

**Not to be made piecemeal, and if made, made uniformly.** That is a constraint on the work, not
a note about it: implementing it for `ThreadpoolWait` alone -- the type the M26.13 measurement
happens to implicate -- would leave the crate with one linear type and the rest affine, which is
a worse surface than either choice made consistently.

**It is not greenfield.** `ThreadpoolIo` already ships a soft version: its `Drop` reports a
skipped rundown and then continues. A hard fail-fast changes that type's existing behaviour too.

**`M-T6.7` gave the crate a uniform `stop_and_drain`, but not across all six types.**
`ThreadpoolIo::run_down` and `CleanupGroup::close_members` keep their own names for reasons
recorded with that decision, so this item has to say what a fail-fast means for those two rather
than assume the uniform method covers them.

**`Drop` panics, decided 2026-10-01**, which settles the sub-question the mechanism left open.
A second panic on an already-unwinding path aborts, which is the right outcome rather than an
accident, and is consistent with the abort-on-unwind contract the crate already enforces for
callbacks. See [What it does: `Drop` panics, and a double panic
aborts](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature).

**Drain first, then panic.** A constraint on the work: panicking before the drain would unwind
past the close and the context free, leaving the pool able to dispatch into a context that is
leaked but still live -- the abandonment the bound below forbids, reached through the mechanism
meant to prevent it.

**Guard it the way the callback contract is guarded.** `tests/callback_panic_aborts.rs`
re-executes itself as a child process, because an abort would otherwise take the test runner
with it; a fail-fast that aborts on an unwinding path needs the same treatment, and that file is
the worked example to follow rather than re-derive.

**One bound is already fixed and constrains every answer: forward progress is not the
alternative.** A teardown that cannot drain may abort, or fail fast by some other route, but it
may not return to its caller having abandoned the callback. Bounding the wait is a question
about which failure to take, never about whether to continue.

#### What implementing it found

The two questions the item posed are answered in [The two types whose close is not named
`stop_and_drain`](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature): `run_down` is
`ThreadpoolIo`'s discharge and the panic names it, and `CleanupGroup` is deliberately out of
scope because it has no per-object obligation to report.

**Arming it measured the whole workspace, and the workspace did not pass.** Feature unification
means `--all-features` -- how CI and the sabotage harness both run -- arms this for every crate
here. The first armed run failed across four crates, every failure a real undischarged
obligation rather than a false positive, including two of this crate's own published rustdoc
examples. All were made conformant; see [Arming it found the workspace was not
conformant](../../DESIGN-NOTES.md#fail-fast-is-a-default-off-feature).

**A failing list from an aborting run is a sample, not a population.** Early runs aborted
partway -- a `Drop` panic landing inside an already-unwinding test -- so libtest never printed a
summary and each "complete" failure list was a truncation. Converging took five runs. This is
worth remembering for any future sweep under this feature.

**The first ordering guard was vacuous, and the sabotage is what said so.** Asserting that the
queued callback *ran* cannot distinguish the two orders: a panic placed before the drain unwinds
past `CloseThreadpoolWait`, so the pool goes on watching a leaked context and the callback still
runs, just later and unsupervised. Only the *timing* separates them, so the guard now snapshots
the count the instant the panic surfaces and asserts before joining the releasing thread. Three
cases in [sabotage.json](sabotage.json) hold this down: the panic placed before the drain, a
fail-fast that never fires, and one that fires when nothing is owed.

**No separate CI job was added**, deliberately: the existing `--all-features` jobs arm the
feature by construction, unlike the feature-*off* configuration that `M-T6.6` had to add a job
for.

## Moved 2026-10-02 22:50:33 -04:00 -- M-T9, make the repair mark lossless, and guard the teardown that frees it

Opened 2026-10-01 by a code review of this branch, which found two defects in `heal.rs` that the
whole test suite and a full sabotage sweep had both missed. The first is fixed; `M-T9.2` is the
second, and `M-T9.1` is the guard both of them want.

### <a id="m-t92"></a>M-T9.2 -- Replace the repair mark with three derived timestamps, and point the trampoline at the entry. *(completed 2026-10-02 22:50:33 -04:00)*

Implemented 2026-10-02 to the shape recorded below.

**What landed.** `last_cancelled`, `last_started` and `last_submitted`, each written by an
unconditional store. `unhealed()` is `last_cancelled != 0 && last_cancelled >= last_started`;
`repair_in_flight()` is `last_submitted > last_started`; `tick` submits when unhealed and nothing
is in flight, and **clears nothing**. The trampoline takes a `*const PoolEntry`, so all four
values live on the entry, and `repair` is the entry's first field.

**Correction, and then a fix, both from `M-T9.1`:** this entry first said that placement made the
drain run "before anything it writes to is freed", and that is false. Every other field owns no
heap, so dropping one frees nothing; the single allocation belongs to the `Arc` and is released
only after all drop glue. The ordering was load-bearing for the `Box<AtomicU64>` design it
replaced, and the justification was restated for a design where it no longer applied. The drain
has since moved onto `PoolEntry::drop`, so declaration order carries nothing and the question
cannot be got wrong a third time.

**Verified by sabotage, not by reading.** Restoring the keep-earliest `compare_exchange` fails
`a_cancellation_during_a_tick_is_not_lost` on the assertion that a later cancellation moves the
stamp forward. Both `heal.rs` entries in `sabotage.json` were re-anchored and re-run through the
harness; both are caught.

**Equality reads as unhealed**, per the first detail the item asked to decide deliberately. A
cancellation and a dispatch inside one `QueryInterruptTime` tick are indistinguishable, so this
errs toward a redundant repair; `a_dispatch_in_the_same_tick_as_the_cancellation_does_not_heal_it`
pins it.

**The submit guard is `!repair_in_flight()`, not `last_cancelled > last_submitted`.** The item
proposed the latter. The former is the same rule stated against the pair that already answers
"has the pool given it back", so there is one comparison rather than two that must agree -- and it
behaves better in the case the item did not reach: a cancellation arriving while a repair is in
flight is answered by that repair if it dispatches afterwards, and by the next tick if it does
not.

**The user-dispatch signal is gone, with the tests that proved its wiring.** `stamp_dispatch`,
`last_dispatch` and `dispatched_since` are removed, and with them the five per-kind tests that
asserted each trampoline stamped its entry. They tested a mechanism this item replaces: health is
now evidenced by *our own* repair dispatching, which a quiet pool cannot fake. Removing it also
takes one atomic store off every callback the crate delivers. `Registration` is still held by the
work, timer, periodic-timer and I/O contexts, now purely for its `Drop`, and each field says so.

**The item's second deferred detail -- a wedged pool never retrying -- was decided 2026-10-02
and is implemented.** A repair still in flight past `OVERDUE_AFTER` (five seconds, twenty healer
periods) is reported as `repair-overdue` and re-submitted. The allowance is one reattempt,
counted per episode and reset whenever a repair actually runs; past it, `fail-fast` ends the
process and a build without that feature goes on reporting and re-submitting indefinitely. The
threshold is generous on purpose: "wedged" and "merely busy under load" look identical from the
healer, the second is enormously more likely, and the cost of being hasty is crying wolf about a
healthy pool -- or, under `fail-fast`, ending the process over one.

**The `fail-fast` here aborts; the teardown ones unwind.** It fires on the healer's timer
trampoline, and Rust turns a panic escaping an `extern "system"` boundary into an abort. That is
stated at `fail_fast_if_unrepairable` rather than left to be discovered, and it is defensible on
its own terms: the condition being reported is that a pool in this process has stopped
dispatching, so there is no thread known to still work to unwind to. The message still reaches
stderr, because the panic hook runs first -- asserted, not assumed.

**What a second submit is worth is not established, and the code says so.** The recovery this
feature rests on is measured for *a* submit; whether a factory re-evaluates its create decision
on a later arrival is the question `poke_completion_ports` exists to ask, and the answer recorded
there is that an ordinary arrival has never been seen to recover this stall. The reattempt is a
cheap thing tried in a state that should not arise, not a mechanism with evidence behind it.

**Three tests, and the state is real rather than simulated.** `StalledPool` occupies a
single-thread pool's only thread, so a submitted repair genuinely never dispatches. That is not
decoration: `repair_trampoline` stamps `last_started` as its first act, so merely *holding* the
callback has already made `repair_in_flight` false, and an entry whose repair has started can
never be made overdue again without waiting out the real threshold. The fail-fast case runs in a
child process and asserts on how it died, because an in-process abort would take the runner with
it. Both branches are in `sabotage.json`: deleting the overdue test, or the reattempt
comparison, is caught.

### <a id="m-t91"></a>M-T9.1 -- Write the test that would have caught the repair-object use-after-free. *(completed 2026-10-02 22:50:33 -04:00)*

Opened 2026-10-01 by a code review of this branch, which found a use-after-free in `RepairWork::drop`
that the whole test suite and a full sabotage sweep had both missed.

Done 2026-10-02 as `retiring_an_entry_waits_for_a_repair_callback_already_running`.

**The window, now that `M-T9.2` has landed.** `repair_in_flight` keeps an entry alive while a
repair is queued, so the original shape -- submitted, not yet dispatched, entry freed -- is no
longer reachable. What remains is narrower, and is what the drain actually covers: the callback
stamps `last_started` on entry, which is precisely what makes the entry retirable, and then writes
to the entry again. A retirement landing between those two writes drops the last `Arc`, and
`CloseThreadpoolWork` does not wait.

**Asserted on the drop blocking, not on a fault** -- a deliberate departure from this item's
suggestion of `windows-guard-alloc`, for the reason the item itself gives. A crash-caught result
is treated as uncovered here, and a guard page reports a use-after-free by crashing. Measuring
that the retirement waited states the property directly, needs neither a second process nor a
global allocator, and fails by name. Under sabotage: **3.7 us against a 400 ms hold**, scored by
the harness as `exit 101` -- an ordinary test failure rather than a crash code.

**The window is reached by holding the callback inside it**, under a `#[cfg(test)]` hook, because
it is a few instructions wide and nothing lands there by timing. Added to `sabotage.json`, so the
guard is re-checked rather than verified once and discarded.

**DECIDED 2026-10-02 and done: the invariant is unrepresentable rather than guarded.** `PoolEntry`
now drains in its own `Drop`, which runs to completion before any field is dropped, so the
teardown no longer depends on declaration order at all. Checked by moving `repair` to the **last**
field and re-running the guard: it still passes, where the arrangement it replaced would have had
exactly the hazard its own comment warned about. The drain stays guarded by the test above, which
still fails under sabotage at `exit 101`.


## Moved 2026-10-07 18:14:51 -04:00 -- M-T14, events come from `win-sync-sys`

Opened 2026-10-07. The engineer moved event creation out of this crate, which exists to wrap the
thread pool, into the new `win-sync-sys`
([WS-D-5](../win-sync-sys/DESIGN-NOTES.md#ws-d-5)), and asked that this crate be made to work well
with it as part of the same work.

### <a id="m-t141"></a>M-T14.1 -- `WaitableHandle` is built on `win-sync-sys`' `Event`, and the documented way to signal or reset a pool-owned event needs no `unsafe`. *(completed 2026-10-07 18:14:51 -04:00)*

**What landed beyond the item's text.** A third example used `unsafe { ResetEvent(...) }`: the
reset-before-re-arm example on `WaitActivation::rearm`, found by sweeping the workspace for raw
`SetEvent`/`ResetEvent`/`CreateEventW` outside tests. It now resets through the callback's clone
of the `Event`. The module documentation said to signal through `ThreadpoolWait::handle`, and now
says to keep a clone; the README's "Safety highlights" names the conversion. `Win32_Security` left
the `windows-sys` features, having existed only for `CreateEventW`.

**Two tests the item did not name.** `the_conversion_keeps_the_event_it_was_given` asserts object
identity with `CompareObjectHandles`, with a second event as the negative case, so the conversion
is pinned without a timing window; and `the_event_constructor_makes_each_reset_mode_and_initial_state`
walks all four combinations, so the constructor's booleans still mean what they meant when it
called `CreateEventW` itself. A second sabotage covers that mapping.

**How the sabotages were verified.** Both are caught under this crate's manifest, but that run is
workspace-wide and `windows-file-enumeration-sys`' completion-ring tests fail first, ending the run
before this crate's tests report. Each was therefore re-applied by hand and run against this
crate's wait tests alone; each was caught by the test its `why` names.

The item as it stood at completion:

- [x] **M-T14.1** -- **`WaitableHandle` built on `win-sync-sys`' `Event`.**
  - `From<Event> for WaitableHandle`, safe because an `Event` is never a mutex
    ([WS-D-4](../win-sync-sys/DESIGN-NOTES.md#ws-d-4)).
  - `WaitableHandle::event` keeps its signature and creates through `Event`, so this crate makes no
    event of its own; drop the `windows-sys` feature that existed only for its `CreateEventW`, if
    nothing else needs it.
  - The two `ThreadpoolWait` examples in `wait.rs` signal through a kept `Event` clone rather than
    `unsafe { SetEvent(wait.handle()...) }`, so the documented way to signal a pool-owned event
    needs no `unsafe`.
  - **Tests:** a wait built from an `Event` fires when a kept clone is set; and both `From` and
    `WaitableHandle::event` preserve the reset mode, an auto-reset event firing once per `set`.
  - **Sabotage:** the conversion creating a fresh event instead of taking the one it was given.

  > **CROSS-COMPONENT PREREQUISITE:** `crates/win-sync-sys` -> `WS-M1` -> `WS-1.1` (the crate, with `Event`). See [CHECKLIST.md](../win-sync-sys/CHECKLIST.md).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M3` -> `DI-3.2.2` (dioring's plain I/O and Model A front end, whose readiness event is an `Event`). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## Moved 2026-10-18 22:58:35 -04:00 -- M-T15.1, a wait that shares its handle

### <a id="m-t151"></a>M-T15.1 -- A wait target may be a shared handle, through `assume_waitable_shared`, and `into_handle` gives it back only to its last holder. *(completed 2026-10-18 22:58:35 -04:00)*

Decided by the engineer on 2026-10-18 ("take the decision as yes now"); the decision, and the two points the
assistant settled within it (the seam's contract, and what `into_handle` returns), are in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md#a-wait-may-share-its-handle). `WaitTarget` has a `Shared(SharedHandle)` arm;
releasing the wait's clone is the target's destructor, so both teardown paths already order it after the drain.
Restructured while `WS-2.2` (then numbered `WS-2.1`) was implemented: switching `From<Event>` to the shared arm moved to
[WS-2.2](../win-sync-sys/COMPLETED-CHECKLIST.md#ws-22), because it needs a shareable `Event` and `WS-2.2` needs this arm first.
Six tests, and three sabotages (two caught, one control survived); one is caught by the pool failing fast on a
null handle, which the manifest says.

## Moved 2026-10-21 21:46:52 -04:00 -- windows-threadpool-sys: milestones M-T10, M-T11, M-T12, M-T13 and M-T15 are complete

### <a id="m-t10----the-second-review-round-ordering-serialization-and-a-test-that-never-ran"></a>M-T10 -- The second review round: ordering, serialization, and a test that never ran

Opened 2026-10-02 by a review of everything on this branch, and grown by each review round that
followed it; every item is in `windows-threadpool-sys`. The round's lesson is recorded with
`M-T10.1`: the reviewer named two sites for a rule that lived at eight, and the sweep the house
rules require is what found the third.

- [x] **M-T10.1** -- **Record the drain obligation before the arming is published, at one site
      rather than eight.** The re-arm paths in `wait` and `timer`, and `work`'s submit, set the
      obligation flag *after* the native call that publishes the arming. A dispatch entering in
      that window settles the obligation and is then overwritten, so a correctly-drained object
      claims it owes a drain -- under `fail-fast` a panic, and an abort if `Drop` was already
      unwinding. Fixed by moving the pair into `CloseObligation::record_live_before` and making
      the bare store private, so the inverted order is a compile error. Recorded as
      [The drain obligation is recorded before the arming is
      published](../../DESIGN-NOTES.md#obligation-recorded-before-arming).

- [x] **M-T10.2** -- **Make `tests/obligation_report.rs` run in CI, then make it pass there.**

  **The defect, and which half matters.** The test deliberately drops objects that owe a drain, so
  under `--all-features` -- which arms `fail-fast` -- it exits 101. That is the reported symptom.
  The *defect* is why nobody noticed: the test early-returns when no trace filter is set, so on
  every CI run it does nothing and reports success. It is inert, and an inert test that reads as
  green is worse than an absent one.

  **Target.** Fix the inertness first and confirm the test then fails, which is what shows the
  early return was hiding it. Then make it pass under `fail-fast` -- either by catching the
  expected panic, or by gating the owing cases off the feature -- without reintroducing a path
  where the whole body is skipped silently.

  **Sabotage it**: break the behaviour the test asserts and confirm it goes red *in the
  configuration CI actually runs*. A guard that only fires under a locally-set environment
  variable has not been shown to guard anything.

- [x] **M-T10.3** -- **Serialize every entry point that suspends other threads.** Done by a
      `Mutex` inside `with_others_suspended` itself rather than a guard at each caller, so the
      rule holds for every path into it including ones not yet written. `install_requested`'s
      `AtomicBool` stops it running twice, which is idempotence and not mutual exclusion;
      `install_by_label` had nothing.

  **The reported hang did not reproduce, and the report should not be relied on.** The review
  said two named tests hang at `--test-threads 2` and pass 20 of 20 at `--test-threads 1`. Neither
  half holds here: 0 hangs in 40 runs of that pair at `--test-threads 2`, 0 hangs in 25 runs of
  the whole `trace::` module at `--test-threads 2`, and the pair *fails* rather than passes at
  `--test-threads 1` -- for the unrelated reason that became `M-T10.6`.

  **The fix is justified analytically, not by that measurement.** Two threads inside
  `with_others_suspended` can each enumerate the other and then suspend it, leaving neither
  running to resume the other; `SuspendThread` is documented as not guaranteeing the suspension
  is complete when it returns, which is the window. One `Mutex` removes the state entirely and
  costs nothing on a facility that installs hooks once per process. An unreproduced hazard that a
  cheap change makes unrepresentable is worth closing; what is not acceptable is recording it as
  measured when it was not.

- [x] **M-T10.4** -- **DECIDED 2026-10-02: hot-patching is exclusive, and the inline hooks spend
      that exclusivity knowingly.** Documented as a hazard rather than engineered around, and
      recorded as [Hot-patching is
      exclusive](../../DESIGN-NOTES.md#hot-patching-is-exclusive).

  **The decision reframed the finding, and the reframing is the useful part.** The review raised
  a patch window: suspension does not place a thread's instruction pointer *outside* the fourteen
  bytes being overwritten. True, and the narrower half. The larger half is that inline hooking is
  **process-exclusive** -- one slot per stub, no way to share or negotiate, and a second patcher
  captures the first one's jump as though it were the original. Setting
  `WINDOWS_THREADPOOL_TRACE_HOOKS` therefore gives up hot-patching those stubs for the life of
  the process, and the crate never undoes it.

  **Why documenting beats fixing.** Relocating trapped instruction pointers would close the
  window and would not make two patchers able to share a stub, because the exclusivity belongs to
  the technique and not to this implementation. The expensive fix buys the smaller half. The cost
  is better spent making the choice visible where it is made, which is what this item did: the
  module docs own the statement, the variable's own documentation and the README point at it.

  **The window itself is narrower than reported -- but not for the reason this item first gave.**
  Installation runs from the `.CRT$XCU` initialiser before `main`, so no other thread exists to be
  inside the range. This item originally asserted that as already true; a later review found it
  was not. Installation sat at the end of the public `observe_exceptions`, which `enabled` calls
  only when the trace is armed, so a process with `WINDOWS_THREADPOOL_TRACE_HOOKS` set and
  `WINDOWS_THREADPOOL_TRACE` unset could reach the installer after its threads existed. Closed by
  `M-T10.8`, which moved installation into the initialiser and sealed the window. What remains is
  `install_by_label`, which is `#[cfg(test)]`.

- [x] **M-T10.6** -- **Stop the recogniser's live canary reading a stub a sibling test has
      patched.** Found while trying to reproduce `M-T10.3`'s reported hang, which is the only
      reason it was found at all.

  **The defect.** `the_stub_recogniser_accepts_the_shape_and_rejects_everything_else` ended by
  resolving the `selftest` hook target and asserting the bytes there are a syscall stub -- the
  canary that says the recognised shape still describes this machine's Windows.
  `a_hooked_stub_records_both_ends_and_still_performs_its_syscall` patches that same stub and, by
  design, never removes it. Whichever ran first decided the answer, so the canary was asserting
  the planted jump rather than the shape Windows shipped.

  **Measured.** 25 of 25 runs of the `trace::` module failed at `--test-threads 2` before the
  fix and 0 of 25 after. It is an order dependency and not a thread-count race: running just the
  two tests single-threaded, with the hooking one first, failed too. The full suite passes only
  because the default thread count happens to order them the other way -- luck that changes with
  the test count, the machine, or a rename.

  **Fixed** by giving the canary `unhookable_stub_entry()`, an `ntdll` export deliberately absent
  from `HOOKS` and asserted to be absent, so this module cannot patch it however many hooks a run
  installs.

- [x] **M-T10.8** -- **Enforce the one-thread hook-installation window at the installer.** Found
      by the third review round, which falsified a claim `M-T10.4` had written into three
      documents one commit earlier.

  **The defect.** Hook installation sat at the end of `observe_exceptions`, which `enabled` calls
  only when the trace is armed. A process with `WINDOWS_THREADPOOL_TRACE_HOOKS` set and
  `WINDOWS_THREADPOOL_TRACE` unset therefore left the installer's once-flag unconsumed, and
  `observe_exceptions` is public -- so a later call from a running process would patch `ntdll`
  with the pool's threads alive, the one case the recogniser cannot make safe.

  **The shape is the lesson.** The guarantee was carried by *which function happened to call the
  installer*: correct while nobody moved the call, silently false afterwards, and checked by
  nothing. Fixed by moving installation into the pre-`main` initialiser, unconditionally and
  independent of the trace, then sealing the window; `install_requested` now refuses and records
  `refused-after-seal`. Guarded by `the_hook_installation_window_is_shut_before_any_test_runs`,
  verified by sabotage.

- [x] **M-T10.9** -- **Take no process lock while the other threads are suspended.** Found by the
      third review round.

  **The defect.** The suspended window took three process-wide locks: the allocator (through
  `ntdll_proc`'s owned name, and through the suspend loop consuming its enumeration vector by
  value so the `Vec` was freed with threads already stopped), the loader (`GetModuleHandleA`,
  `GetProcAddress`), and the memory manager (`VirtualAlloc`, `VirtualProtect`). A thread stopped
  holding any of them can never give it back.

  **The shape is the lesson, again.** The window's own comment said "Nothing in here may
  allocate", and the enumeration vectors were reserved ahead of time for exactly that reason, with
  a control in `sabotage.json` recording it -- while the install the window wrapped called the
  loader three lines later. The rule was stated at the allocation somebody noticed rather than at
  the window it belongs to.

  **Fixed** by splitting install into prepare / commit / finish: everything that can take a lock
  happens while the process is still running, and the window holds a single fourteen-byte store.

- [x] **M-T10.10** -- **Mark a cancelling group release's repairs after the cancellation, not
      before.** Found by the third review round.

  **The defect.** `CleanupGroup::close_members(true)` marked each member's pool as owing a repair
  *before* calling `CloseThreadpoolCleanupGroupMembers`, justified in a comment by the claim that
  the native release frees the member contexts. It does not -- this crate frees them, in the loop
  immediately after. So the justification was false and the ordering it bought was harmful: the
  healer could see the mark, find the pool still dispatching, clear it as repaired, and then the
  real cancellation would happen with no mark outstanding. An unrepaired wedge from a **single**
  cancellation, where the overlapping-cancellation race already recorded in
  [`M-T9.2`](COMPLETED-CHECKLIST.md#m-t92) needs two.

  **Guarded.** `a_cancelling_release_marks_its_pool_after_the_cancellation` installs a test-only
  hook that clears the mark at the instant before the native release, standing in for the healer's
  tick landing there -- which is necessary because both orderings leave a mark outstanding once
  the release has returned, so no end-state assertion can tell them apart. Verified by sabotage:
  restoring the old ordering fails it.

- [x] **M-T10.11** -- **Give the worker-factory snapshot named fields.** Found by the third review
      round.

  **The defect.** `worker_factory_snapshot` returned `(usize, u32, u32, u32)` and its doc comment
  described `(handle, total, waiting, pending)` -- every field after the handle shifted by one
  from what the code returns, which is the handle, the thread *maximum*, the total workers and the
  waiting workers. A caller who believed the comment read the configured maximum as the number of
  workers that exist, so a cold or stalled pool looked fully staffed: the exact inverse of the
  reading the data is gathered for. There is no `pending` field at all.

  **Fixed at the build rung, not in the comment.** The return type is now
  `WorkerFactorySnapshot` with named fields, so the mismatch is unrepresentable. Nothing could
  have caught the tuple version: both halves type-check, and a tuple carries no statement about
  which field is which for anything to check against. The consuming tests destructured correctly,
  which is why the suite stayed green and the defect lived only in what a reader was told.

- [x] **M-T10.12** -- **Make the lifecycle trace test run, and expect what teardown actually
      calls.** Found by the third review round; the same inertness as `M-T10.2`, which was fixed
      without sweeping for siblings.

  **Two defects, one hiding the other.** The test early-returned when the trace filter was unset,
  which is every ordinary run and every CI run. Underneath that, it expected
  `WaitForThreadpoolWaitCallbacks(cancel)` and `WaitForThreadpoolTimerCallbacks(cancel)` -- forms
  that only `try_cancel_pending` makes, and that teardown stopped making when it changed from
  cancelling to draining. Run alone with the filter armed it failed; run in a full armed suite it
  passed, because sibling tests supplied those records into the one process-wide buffer and
  `counted` only asks for a non-zero count.

  **Fixed** by re-executing the one test single-threaded in a child with the filter set, and by
  correcting the two expectations. The child exits with a distinctive code, so a filter that
  matched nothing -- a rename -- fails loudly instead of exiting 0 and looking like a pass.
  Verified by sabotage in both directions: the old labels fail the body, and a wrong test name
  fails the parent.

  **The sweep this time:** two tests had the early-return shape;
  `the_exception_observer_notes_a_first_chance_exception` passes when armed.

- [x] **M-T10.13** -- **Restore page protection once per page, not once per stub.** A defect
      introduced by `M-T10.9`'s own fix, found by the fourth review round.

  **The defect.** `VirtualProtect` reports the previous protection of the whole **page**, not of
  the byte range asked about. Splitting install into prepare / commit / finish moved protection
  to one save-and-restore per stub, so two stubs sharing a page had the second save the writable
  state the first had just installed -- and the last restore left `ntdll` executable **and
  writable** for the rest of the process's life. `park`, `set-info` and `shutdown` share a page on
  this host. The single-phase install it replaced did not have this, because it protected and
  restored around each store in turn.

  **Guarded** by `stubs_sharing_a_page_are_opened_once_and_the_page_is_restored`, hermetic on a
  page the test allocates rather than on `ntdll` -- the real stubs are patched once per process,
  so whichever test ran first would decide the answer. Verified by sabotage.

- [x] **M-T10.14** -- **Read `AlreadySignaled` only on a successful association.** The
      `associate` hook read its out-parameter unconditionally after forwarding the call. A failed
      `NtAssociateWaitCompletionPacket` need not have written through that pointer, or validated
      it, so the instrument could turn an ordinary error return into an access violation raised
      by the tracing facility itself. A failed association has nothing to report there anyway:
      no packet was associated, and the `-leave` record already carries the status.

- [x] **M-T10.15** -- **Make the periodic-timer exercise reach its second re-arm entry point.**
      The callback switched on a `load` taken *after* its own `fetch_add`, so the second firing
      compared 2 against 1 and the `rearm_at` arm was unreachable: the exercise finished a firing
      early having driven only `rearm_after`, while its comment claimed both. Both entry points
      emit the same event tag, so neither the test nor the trace assertion it feeds could notice.
      Now switches on the value `fetch_add` returns.

- [x] **M-T10.16** -- **Run every trace assertion in a trace-armed child, from one site.** The
      sweep for `M-T10.12` asked the wrong question -- it grepped for early `return`, while
      `io::tests`' two trace blocks are `if wants(..) { .. }` -- so both were still inert.

  **Fixed at one site rather than a fourth copy.** `trace::in_a_trace_armed_child` now owns the
  parent/child protocol; the lifecycle test and both `io::tests` blocks go through it. The first
  two hand-written copies had already disagreed about how to distinguish a child that ran from
  one whose filter matched nothing. Verified by sabotage: removing the `start-cancelled` record
  fails the child on its assertion and the parent on its exit-code check.

  **The real population**, found by grepping for `wants(` rather than for `return`: four sites.
  Two were these; one is `the_filter_narrows_to_the_targets_named`, where the gating *is* the
  subject; the fourth was the hook records, converted by `M-T10.5`. Re-run after that conversion,
  the grep leaves no assertion *silently* skipped on an unset filter: what remains is this crate's
  `wants` definition and macros, four sites that **assert** `wants` is true inside an armed child,
  the one test whose subject is the gating, and `tests/obligation_report.rs`, which gates but then
  exits with a distinct code its parent checks rather than passing quietly.

- [x] **M-T10.17** -- **Bound the warm-up's teardown on the path where its callback never
      arrived.** `prewarm_default_pool` timed out after two seconds and then called
      `stop_and_drain`, which waits with no deadline for the very callback whose absence caused
      the timeout -- so the bounded check was followed by an unbounded one and the `false` it
      exists to return could never arrive. It now cancels the queued invocation first, on that
      path only.

  **Not covered by a test**, and stated plainly rather than papered over: reaching the timeout
  needs a pool that has stopped dispatching, which is the stall this repository has spent `M-T7`
  failing to produce on demand.

- [x] **M-T10.19** -- **Handle the failures the patch path assumed away, and check the branch the
      trampoline does not rebuild.** Three findings from the fifth review round, all in
      `trace/hook.rs`, two of them the same recorded rule broken twice.

  **Two unchecked failable calls**, against [A failable call has its failure handled,
  always](../../DESIGN-NOTES.md#a-failable-call-has-its-failure-handled-always). The trampoline
  page's `VirtualProtect` result was discarded and the pointer published regardless, so a failure
  left the hook jumping into a page the processor will not execute. And `with_others_suspended`
  skipped past a thread it could not open or suspend and patched anyway -- while that thread was
  still running through the bytes being overwritten, which is the precondition the suspension
  exists to establish. Both now refuse, with their own `Refusal` variants.

  **The refusal had to learn a distinction, and a test is what taught it.** Refusing on every
  failure made `install_by_label` fail in an ordinary test process, because the thread snapshot is
  taken while the process runs and a pool starting and finishing workers routinely leaves a listed
  thread gone before `OpenThread` reaches it. `ERROR_INVALID_PARAMETER` names that case, and a
  thread that does not exist cannot be executing the bytes -- so it is skipped and everything else
  refuses.

  **An assumption the module did not admit to.** The recognised stub tests a shared-data byte and
  branches past `syscall`; the trampoline rebuilds only the fall-through, so "the same call by
  construction" held while that bit is clear and not otherwise. It is clear on ordinary x64, which
  is why it went unstated -- an observation about this machine rather than a property of the
  technique. `syscall_path_is_direct` now reads it, derived from the recogniser's own bytes rather
  than written out a second time, and an install refuses when it is set.

- [x] **M-T10.20** -- **Stop the fail-fast and the cancellation safety claim promising more than
      they detect.** Two findings from the fifth review round that are not defects in the code but
      in what it says.

  **The fail-fast** records an outstanding *arming*, not "this `Drop` blocked". A dispatch settles
  it for a wait and a one-shot timer while `Drop` still waits for that callback, so dropping
  during an executing callback drains without reporting -- and `CleanupGroup` carries no
  obligation flag at all. Both now stated at the one site that owns the message.

  **`try_cancel_pending`** is documented as safe because the crate repairs the pool afterwards.
  On a pool whose repair item could not be created there is no entry and nothing repairs it, and
  `owe_repair` returned in silence. It now records `cancel-untracked`, and both statements of the
  claim name the exception.

- [x] **M-T10.21** -- **Make the thread enumeration fail closed before anything is patched.** The
      sixth review round. `with_others_suspended` refused when a thread could not be opened or
      suspended, but every way of failing to *find* the threads reported success: a snapshot that
      could not be taken, an enumeration that could not be started, and a thread count past `ROOM`
      each left `ids` empty or short, which read as "everything is suspended" and patched fourteen
      bytes of live code with nothing stopped. Strictly worse than the per-thread case fixed one
      block above it in `M-T10.19`. An `enumerated` flag now gates `quiesced`, and the suspend loop
      is skipped entirely when it is false, so a refused install perturbs nothing.

- [x] **M-T10.22** -- **Stop a transient healer-start failure disabling self-heal for the process.**
      `ensure_running` cached its result in a `OnceLock<Option<Healer>>` built with `.ok()?`, so one
      failed `ThreadpoolPool::new()` -- a transient condition -- latched `None` permanently and
      silently, and no later cancellation anywhere in the process was ever repaired. Now an
      `AtomicBool` fast path over a `Mutex<Option<Healer>>`: a failure records `healer-start-failed`
      and leaves the slot empty, so the next cancellation retries.

- [x] **M-T10.23** -- **Isolate the heal tests from every other tick in the process.** `tick` walks
      the whole registry, so a private pool does not isolate a test: another test's tick, or the
      background healer's, can clear a mark between the cancellation that sets it and the assertion
      about it. A `TICK_GATE` now brackets each critical section.

  **Reproduction, stated honestly.** The review saw this fail on run 19 at 32 test threads. 60 runs
  here with the gate disconnected, and 60 more with a thread calling `tick` in a loop, did not
  reproduce it -- the window is microseconds wide, so missing it settles nothing. What settles it is
  placing a tick in the window by hand: inserting `tick_inner()` between
  `a_cancelling_group_release_marks_a_wait_members_pool`'s `close_members_cancelling()` and its
  assertion fails that test every time, on the assertion the review named.

- [x] **M-T10.24** -- **Retry a refused quiesce instead of weakening the refusal.** Found while
      verifying `M-T10.23`, not by the review: the full lib suite at 32 test threads failed 13 of 30
      runs in `trace::tests::a_hooked_stub_records_both_ends_and_still_performs_its_syscall` with
      `NotQuiesced`. Instrumenting rather than guessing named the cause -- `SuspendThread` returning
      `ERROR_ACCESS_DENIED`, enumeration intact -- which is a thread that is *terminating*.

  This was `M-T10.19` half-converted: the benign/dangerous distinction was given to `OpenThread` and
  not to `SuspendThread` one line below. The carve-out was **not** extended, because the two cases
  differ -- an id that names nothing is gone, whereas a terminating thread may still be running its
  exit path, which is the property the refusal establishes. The refusal stands and the whole attempt
  is retried outside the suspended window. Measured 13/30 before, 0/30 after.

- [x] **M-T10.25** -- **Split the cancelling group release out of `close_members(bool)`, and make
      posting fabricated packets `unsafe`.** Two findings of the same shape: a hazard gated by the
      build for one entry point and by prose for another.

  **`close_members(cancel_pending: bool)`** was safe and ungated in every configuration, while the
  per-object `try_cancel_pending` it reaches is `self-heal`-gated and `unsafe` without the feature.
  With the feature off, `close_members(true)` cancelled every wait member and landed on a no-op
  `owe_repair`. Now `close_members()`, `close_members_cancelling()` (gated), and
  `close_members_cancelling_no_heal_tracking()` (ungated, `unsafe`) -- a `bool` cannot be `cfg`-gated,
  an item can.

  **`poke_completion_ports`** is now `unsafe fn`. It posts a zero key and a null `OVERLAPPED` to
  every completion port in the process above a depth, including ports this crate does not own; the
  "**Destructive** ... only for a process that has already failed" rule was enforced by prose alone.

  Both breaking; both recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [x] **M-T10.18** -- **DECIDED 2026-10-02: a cancellation registers the pool itself, and reports
      it when even that fails.** `heal::register` is best-effort, so a pool whose repair item could
      not be created had nothing for a later `try_cancel_pending` to mark.

  **The decision.** The cancellation retries the registration at cancel time rather than giving up.
  Allocating at the moment of need is what the pre-created repair object exists to avoid, but that
  argument does not reach this path: the alternative is not "allocate earlier", it is "never repair
  this pool at all". If the retry also fails, `cancel-untracked` is recorded **with the pool key**
  and, under `fail-fast`, the call panics.

  **A prior defect surfaced while deciding it, fixed in the same change.** `Registration` held only
  `Option<Arc<PoolEntry>>` and discarded the key, so an object whose registration failed could not
  find an entry another object later created for the same pool -- the pool was repairable, a healthy
  entry existed, and the cancellation still reported it untracked. The same omission made
  `cancel-untracked` record `0` rather than naming the pool. `Registration` now keeps its key.

  **Where the fail-fast fires is load-bearing.** `CleanupGroup`'s cancelling release marks members
  between the native release and the loop that frees their contexts, so a panic raised there would
  unwind past the frees and leak every context. The report is accumulated and acted on after the
  frees, matching `fail_fast_if_owed`'s rule that the panic must report a violation and not cause
  one.

  **The error edge is now reachable from a test.** It runs only when `CreateThreadpoolWork` fails,
  which no test can arrange, so a test-only `FORCE_REPAIR_FAILURE_FOR` forces it -- keyed to one
  pool, because a global flag failed an unrelated test's registration on the first run. Both
  directions are asserted, and sabotaging the retry fails the recovery test by name.

  Recorded in [DESIGN-NOTES.md](../../DESIGN-NOTES.md#a-cancellation-allocates-its-own-repair-rather-than-giving-up).

- [x] **M-T10.26** -- **Release a trampoline page when the batch that prepared it is refused.** The
      seventh review round, and its only finding. `prepare` allocates a 4 KiB executable page per
      hook and publishes it into `TRAMPOLINES[index]` before the batch knows whether it will commit,
      because the patch it builds points at that page. Both post-preparation refusals -- `open_pages`
      failing, and the quiesce failing after all its attempts -- returned with the pages still
      allocated and still published. Bounded at one page per hook and once per process, but a leak.

  **The obvious fix would have introduced a use-after-free, which is why it is not the one taken.**
  `prepare` publishes unconditionally, so if an index were already installed its live, patched stub
  would be jumping through the slot this preparation just overwrote; freeing there would release a
  page a running hook is inside. `discard_prepared` therefore releases only when it found the slot
  **empty**, and restores the displaced pointer otherwise. Only the test-only `install_by_label` can
  reach the non-empty case -- `install_requested` runs the batch once per process behind `DONE` --
  but the branch exists because the function must be correct for its callers rather than for the one
  that happens to exist today.

  **Verified on the real path rather than by reading.** Forcing `open_pages` to return `None` and
  running the hook test printed `DISCARD index=5 previous=0 slot_now=0` and reported `NotWritable`:
  the cleanup runs, takes the releasing branch, and clears the slot before the free. Neither refusal
  branch is reachable from a test without that kind of injection, which is pre-existing and remains
  true.

  The macro's `SAFETY` note that the slot is "never cleared" was corrected in the same change; it is
  now cleared on exactly one path, and the comment states why that cannot race a call already inside
  the hook body.

- [x] **M-T10.27** -- **Run the exception-observer assertion in a trace-armed child.** The eighth
      review round's headline finding, and this branch's own signature defect recurring:
      `the_exception_observer_notes_a_first_chance_exception` opened with
      `if !wants("exception") { return; }`, so in every ordinary run and in CI it returned having
      raised nothing and asserted nothing. `M-T10.12` and `M-T10.16` exist to remove exactly this,
      and the remedy they introduced -- `in_a_trace_armed_child` -- was already applied at three
      other sites. Measured before the fix: the test passed in 0.00s with the filter unset, and
      since its only other exit is a `panic!`, passing was itself the proof it returned early.
      Measured after: 0.06s, and sabotaging the recorder fails it by name.

- [x] **M-T10.28** -- **Stop the healer parking on the one pool it is trying to repair.** `tick`
      submits a repair, clears the mark, then calls `retire_idle` in the same pass. An entry whose
      objects are gone is then idle, so the last `Arc` drops, `RepairWork::drop` drains the work
      object -- and that drain cannot return until the pool dispatches. The pool is by construction
      the one suspected of not dispatching, and the healer is built with `set_max_threads(1)`, so
      one wedged pool parked the only thread the facility has and no pool in the process would ever
      be repaired again.

  `retire_idle` already moved the drop out of the registry lock for this exact reason, and its
  comment says so; that protected every *other* pool's registration from the wait but not the
  healer's own thread. Retirement now also requires `repair_settled()` -- a submission counter
  paired against the existing `runs` -- so an entry is retired only once the pool has given every
  repair back. An entry kept alive for a pool that never dispatches is the cheaper failure by a
  wide margin. The counter is incremented *before* the submit, because a repair can be dispatched
  the instant it is handed over and a count taken afterwards could be overtaken by the run it is
  meant to be paired against.

- [x] **M-T10.29** -- **Bound the writable window to one attempt, not sixteen.** A defect introduced
      by `M-T10.24`'s retry. `install_batch` opened the `ntdll` pages, called the retrying
      `with_others_suspended`, and restored them only afterwards -- so a contended install held live
      code pages `PAGE_EXECUTE_READWRITE`, with every other thread running, across all sixteen
      attempts and their snapshots, where one attempt holds them for microseconds. The retry now
      lives in `install_batch` and opens and restores per attempt; `quiesce_once` is a single
      attempt again and its doc records why the retry cannot wrap it. `NotWritable` is not retried,
      because a page this process cannot make writable will not become writable a millisecond
      later. Re-verified that the flake `M-T10.24` fixed stays fixed: 0 failures in 30 runs at 32
      test threads.

- [x] **M-T10.30** -- **Carry the factory handle on the two records the investigation turns on, and
      five smaller corrections.** `counts-waiting` and `counts-pending` passed a literal `0` while
      `read_one`'s doc promised every record carries the handle -- and those two are precisely what
      the facility exists to read. `counts()` reads every factory in the process rather than
      guessing which is the default pool's, so without the handle a two-factory capture could only
      be attributed by row adjacency. Both now carry it, and the doc names the records that
      genuinely spend the slot on a partner value.

  Also: the module doc linked `../../../windows-ioring-sys/STALL-TIMELINE.md`, a path `b4a0a886`
  emptied when it re-homed that file here; `commit`'s safety contract credited `prepare` with
  opening the page, which `prepare` explicitly does not do (`open_pages` does, per `M-T10.13`);
  `fail-fast` appeared in neither the README nor the crate docs, leaving its build-unification
  hazard stated only in a `Cargo.toml` comment; the `compile_fail` guarding the feature-off API
  shape called a made-up method and so asserted only that `rustc` rejects unknown names -- it now
  names the real method under `cfg_attr(not(feature = "self-heal"))`, verified bidirectionally by
  forcing the block on with the feature enabled and watching it fail; and `Win32_System_IO` under
  `trace` was redundant against the unconditional dependency feature and justified by a comment
  calling it test-only, which `poke_completion_ports` being public contradicts.

- [x] **M-T10.31** -- **Claim a member's repair before the release that can free its pool.** The
      ninth review round, and a use-after-free introduced by `M-T10.18`'s cancel-time retry. A wait
      whose pool could not be registered holds nothing that keeps that pool alive -- the repair work
      object, which would be a bound object deferring `CloseThreadpool`, was never created. A
      cancelling group release frees the last bound object and *then* marks, so the retry inside
      `owe_repair` could call `CreateThreadpoolWork` against freed memory, reachable through the
      safe API. `release_members` now takes each member's claim **before** the native release and
      holds it across, which both creates that object and keeps the pool alive. The per-object path
      was never exposed: `self` is a live bound object for the whole call.

- [x] **M-T10.32** -- **Give retirement one predicate instead of two.** `M-T10.28` added the
      outstanding-repair condition to `retire_idle` and not to `release`, so the hazard it was added
      for -- `RepairWork::drop` draining an undispatched repair on the healer's only thread -- was
      still reachable through the other site. Both now call `is_retirable`. This is the fourth
      partial fix of this shape on this branch; the answer is one definition, not a third careful
      reading.

- [x] **M-T10.33** -- **Make `close_members_cancelling_no_heal_tracking` actually not track.** It
      called `release_members(true)`, whose repair pass was unconditional, so the method named for
      *not* tracking marked pools, allocated repair objects, started the process-lifetime healer,
      and could panic under `fail-fast` -- a contract its own documentation denies, and the opposite
      of what a caller diagnosing a stall asks for. Tracking is now a parameter the safe form passes
      and this one does not, which also stops the fail-fast firing for a caller who took the
      obligation deliberately.

- [x] **M-T10.34** -- **Check the patch-window precondition rather than asserting it from
      placement.** The module documented the hazard exactly -- a suspended thread's instruction
      pointer can be inside the bytes being replaced -- and then claimed the supported path avoids
      it "because it patches when the process has no other threads", justified by running from a
      `.CRT$XCU` initialiser. Placement does not establish that: an initialiser ordered earlier may
      have started threads, and in a DLL the same initialiser runs at attach inside a running
      process. No instruction pointer was ever read.

  Each suspended thread's `Rip` is now compared against the ranges about to be written, and the
  install refuses when one is inside. Verified by sabotage: forcing the comparison to match refuses
  with `NotQuiesced`. The `CONTEXT` buffer is wrapped in a 16-byte-aligned type because
  `GetThreadContext` documents that alignment on this architecture while `align_of::<CONTEXT>()`
  from `windows-sys` measures 8 -- it worked without the wrapper, which is incidental behaviour
  rather than a contract.

- [x] **M-T10.35** -- **Stop the hook tests failing whenever hooks are enabled.** The initialiser
      installs whatever `WINDOWS_THREADPOOL_TRACE_HOOKS` names before any test runs; the two
      installation tests then installed again, met their own patch, and failed on the recogniser's
      correct `NotAStub` refusal. The suite was green only because nothing ever set the variable.
      Reproduced at `exit 101` before the fix. They now install only when `installed_by_label`
      reports the stub is not already patched, which does not weaken rejection of a foreign patch.

  One assertion was wrong in the same configuration for a different reason: it required
  `factory_handle()` to be zero, but `FACTORY` is process-wide and the worker-factory hooks
  legitimately write it, so with every hook installed a real handle is already learned. The claim is
  that *this* call contributes none, so it now compares against the value captured before the call.
  Measured after: 299 pass with hooks unset, `*`, `selftest`, and `associate`.

- [x] **M-T10.7** -- **DECIDED 2026-10-02: yes. CI documents private items.**
      `--document-private-items` added to all five `cargo doc` steps, which already denied the link
      lints but only ever resolved the public surface.

  **The cost it was weighed against did not materialise.** The open question was whether the flag
  would surface links that are useful to a maintainer but meaningless in published docs, forcing a
  choice between noise and rewriting prose to satisfy a build nobody reads. It surfaced **nine dead
  links across four crates**, and every one was simply wrong -- naming an item that had been
  renamed, removed, or gated out of the configuration being documented. None was a judgement call:

  - `heal.rs` x3 -- `record_repair_run` (renamed by [`M-T9.1`](COMPLETED-CHECKLIST.md#m-t91)),
    `PoolEntry::arm` (never existed; `arm_repair` is a free function),
    `PoolEntry::dispatched_since` (removed by [`M-T9.2`](COMPLETED-CHECKLIST.md#m-t92)). All
    three were written during this branch's last two commits.
  - `trace/hook.rs` x3 -- `install`, which has been `install_batch` throughout.
  - `windows-waitable-queues` x3 -- `reserving_mpsc::ClaimLayout` from two sibling modules needing
    a `crate::` path, and `BOUNDS` where the item is `BOUNDS_MAX`.
  - Found afterwards, because cargo stops at the first failing crate: `windows-file-watcher` x3
    (`WatcherInner::reopen`, renamed to `install`) and a `windows-platform-probes` link ambiguous
    between a function and a macro.

  **Verified the flag is what catches them**, rather than assuming: a dead link planted in a
  private item passes `cargo doc -p windows-threadpool-sys --no-deps --all-features` and fails the
  same command with `--document-private-items`. The planted link was first placed in a
  `self-heal`-gated module and checked under `--no-default-features`, where both forms passed --
  the module is not compiled there, so the first attempt proved nothing.

  **One link was deliberately left unlinked.** `pending.rs` referenced
  `Completion::with_injected_failure`, which is gated on `fault-injection`; a link dangles in the
  `--no-default-features` build CI also documents. Named in backticks with the reason, matching the
  precedent set for `close_members_cancelling`.

- [x] **M-T10.5** -- **Make the hook tests' trace-record assertions reachable, so a sabotage can
      reach them.** Done 2026-10-02.

  **What landed.** `a_hooked_stub_records_both_ends_and_still_performs_its_syscall` now runs its
  body in a trace-armed child through `trace::in_a_trace_armed_child`, the one site `M-T10.16`
  built for exactly this, and its two record assertions lost the `if wants("wfactory")` guard that
  made them inert on every ordinary run and in CI. The child opens by asserting `wants("wfactory")`
  is true, so a filter that failed to reach it fails loudly rather than passing vacuously -- the
  trap being closed here, arriving one level up.

  **`fired` stays counted rather than traced**, deliberately. It is the half of that test which
  needs no armed trace, and keeping it independent of the filter means the two halves cannot fail
  for the same reason.

  **Both record sabotages are in `sabotage.json` and are caught by name**, not by a crash:
  deleting the `-enter` record fails on "the hook must record entering the call", and deleting
  `-leave` on its own message, each surfacing through the parent's exit-code check. Verified
  individually before being written down, and the manifest's `notCoveredHere` note -- which had
  cited a completed, unrelated decision for this work -- was rewritten to say what is now swept.

  **The note did not simply go away, because the gap it describes did not.** What `M-T10.5` could
  fix was assertions that existed and could not run. The hook module's other records have no
  assertions at all, which is a different defect; it is now stated precisely in the manifest and
  queued as `M-T10.36` rather than left implied by a note that would read as though everything
  were covered.

- [x] **M-T10.36** -- **Assert the hook module's remaining records, which no test checks at all.**
      **Dissolved 2026-10-03 by [`M-T12.2`](#m-t122): the hook module was deleted.** The records
      it names no longer exist, so there is nothing left to assert. Recorded as dissolved rather
      than deleted because the gap it describes was real while the module was.

- [x] **M-T10.37** -- **Gate the five heal tests that drive an entry with synthetic stamps.**
      Found 2026-10-02 by `M-T10.5`'s stress run, and a defect in `M-T9.2`'s own tests.

  **The failure.** `an_entry_owing_a_repair_outlives_its_last_object` failed on run 18 of 25 at
  `--test-threads 32`, on its final assertion -- the entry it had just healed was still in the
  registry. It took no tick gate.

  **The mechanism, and why `M-T9.2` created it.** These tests stamp an entry by hand with small
  values (`1`, `2`, `10`, `12`). A concurrent tick -- the background healer's, or another test's --
  writes `last_submitted` with `now()`, and the repair it submits writes `last_started` the same
  way. Both are astronomically larger than the synthetic values, so a single interleaved tick
  inverts `repair_in_flight`, `unhealed`, or retirability, whichever the test went on to assert.
  The flag this design replaced could not do it: holding no clock value, a tick could only set or
  clear it, so these tests were safe when they were written and stopped being safe when the stamps
  landed.

  **Fixed at all five sites, not at the one that failed.** Grepping for tests that stamp without
  taking the gate found five; fixing only the observed one would have left the identical defect at
  four. This is the half-converted-rule shape the house rules name, and it is the fifth time it has
  appeared on this branch.

  **Confirmed by placing the interleaving rather than by waiting for it**, since a window this
  narrow proves nothing by failing to recur: inserting one `tick_inner()` between the test's
  `stamp_cancelled(1)` and `stamp_started(2)` fails it every time, on the same assertion the stress
  run hit. 30 runs at 32 threads are clean afterwards. The mechanism is recorded on `gate()`, beside
  the first reason it exists.

### <a id="m-t11----the-tenth-review-round-a-lost-trait-two-stamp-defects-and-the-hooks-abi"></a>M-T11 -- The tenth review round: a lost trait, two stamp defects, and the hook's ABI

Opened 2026-10-03 by a review of everything on this branch. Seven findings. Four are this branch's
own regressions and two of those are `M-T9.2`'s, landed the same day; two are long-standing in the
`trace`-gated hook facility. The round's lesson is that **the gate cannot see any of them**: every
one passed `cargo clippy -D warnings` on four configurations, the full test suite on five, 30
stress runs at 32 threads, and a 27/27 sabotage sweep. Four of the seven are a claim in a doc
comment that the code next to it does not keep.

- [x] **M-T11.1** -- **Restore `TimerMember`'s `Send` and `Sync`, and put the rule on the build.**
      Done 2026-10-03. The two `unsafe impl`s, with `WaitMember`'s SAFETY reasoning, plus a
      `const _` block instantiating `needs_send` / `needs_sync` over all four member types.
      Verified by sabotage in the direction that matters: with the impls removed the build fails
      `E0277` **at lines 942-943 of the assertion itself**, not at some incidental use, so the
      guard is what catches it rather than a bystander.

  **The regression.** At the merge base `TimerMember` held only a `PTP_TIMER` and a `PhantomData`,
  so it was auto-`Send + Sync`. This branch added `context: *mut c_void` and did not add the
  `unsafe impl`s, so a public type stopped being movable or shareable between threads. Verified by
  compiling an assertion: `TimerMember` fails both bounds with `E0277` while `WorkMember`,
  `PeriodicTimerMember` and `WaitMember` pass.

  **`WaitMember` is the precedent and the tell.** It carries two pointers and has both impls with a
  SAFETY note. The same reasoning applies verbatim to the timer's context, so this is one rule
  applied at one of two sites -- the sixth time that shape has appeared on this branch.

  **Fix at the build rung, not by adding two lines.** A `const _: () = ...` assertion over all four
  member types makes the next pointer-shaped field a compile error rather than a silent API break.
  Assert the accepting direction too, so a bound that is accidentally removed from every type is
  not reported as success.

- [x] **M-T11.2** -- **Make the repair predicates say what their doc comments claim.** Two defects
      in `M-T9.2`, found together because they are the same block of accessors. Done 2026-10-03:
      `outstanding: AtomicU32` answers "is a repair queued", and `last_cancelled` / `last_started`
      are published with `fetch_max`. Each has a test that traverses the defect
      (`a_retry_leaves_two_repairs_outstanding_until_both_have_started`,
      `a_cancellation_published_out_of_order_does_not_lower_the_stamp`), each verified to fail by
      name when its fix is reverted, and both are in `sabotage.json`. `last_submitted` stays a
      plain store and says why: single-writer, and the tests must be able to move it backwards.

  **A plain store cannot publish a monotonic clock.** `stamp_cancelled` is an unconditional store
  whose doc says "a later cancellation can only move the stamp forward", and `now`'s doc goes
  further: "the opposite mistake cannot happen at any resolution, because the counter never goes
  backwards." The counter does not, but the *store* can land out of order: a thread that reads 10
  and is preempted past a dispatch at 20 and a second cancellation at 30 will write 10 last, and
  `unhealed` then reads healthy with a cancellation unanswered. That is exactly the lost
  cancellation `M-T9.2` was written to eliminate, re-entering through the publication rather than
  through the compare-exchange. `fetch_max` makes the stated property true.

  **The retry broke the outstanding-repair predicate.** `repair_in_flight` is
  `last_submitted > last_started`, which was sound only while at most one repair could be
  outstanding. `M-T9.2`'s overdue path submits a second before the first has started, so the first
  starting makes the predicate false while the second is still queued. `is_retirable` then permits
  retirement, and `PoolEntry::drop` drains -- waiting for a callback on a pool that is by
  construction suspected of not dispatching, on the healer's only thread. Self-heal stops
  process-wide.

  **Count outstanding submissions rather than comparing timestamps**, so the predicate answers the
  question it is named for. Timestamps then serve only health (`unhealed`) and retry timing
  (`repair_overdue`), which is what they can actually support.

- [x] **M-T11.3** -- **Update the self-heal feature guide, which documents the design `M-T9.2`
      deleted.** Done 2026-10-03. The guide still tells a consumer that every callback this crate dispatches stamps
  a per-pool marker, that a busy pool therefore suppresses repair submissions, and that the per
  callback cost is an interrupt-time read and a relaxed store. None of that is true: health is now
  evidenced only by the crate's own repair dispatching, and that store was removed from every
  callback. This is the public-facing document and nothing swept it.

- [x] **M-T11.4** -- **Stop a failed pre-release recovery claiming to pin a pool it does not.**
      Done 2026-10-03. The marking pass now iterates the **claims** rather than the members'
      contexts, and `Registration::owe_repair_claimed` reports untracked instead of re-registering.
      Losing that retry costs little: the claim *is* a retry, taken at the only moment it is safe,
      with the members still live. The `OwnedResource::owe_repair` hook is gone, since the claim
      now carries the same fact and two mechanisms could disagree -- which orphaned
      `ThreadpoolWait::owe_repair` in the feature-off build and it is gated to match its one
      caller. Guarded by a test that drives the *difference*, not the symptom: it lifts the forced
      allocation failure in the `before_release` hook, so the removed retry would have succeeded.
      `recover_repair` returns `Some(reclaim())` unconditionally, and `reclaim` re-attempts
  registration. When that attempt fails the `Registration` holds no entry and therefore no `Arc`,
  so it keeps nothing alive -- yet the field's doc says the caller "holds [it] across the native
  release so the pool cannot be freed under the marking pass that follows". On that path the
  marking pass can reach `owe_repair_untracked`, which registers and arms a work object against a
  pool pointer whose last member has just been released. Reachable only under allocation failure,
  which is why it is stated rather than assumed.

- [x] **M-T11.5** -- **Give each hook the arity of the stub it is planted over.** Done 2026-10-03,
      and **deliberately only where the arity is establishable** -- read the next paragraph before
      treating the remaining five as an oversight.

  **What landed.** The `hooks!` table gained an argument-list column that becomes each hook's own
  signature. `selftest` declares three and `associate` eight. Two `const` assertions sit beside the
  table: no entry may exceed the forwarding width, and the `associate` entry may not drop below
  eight, since it reads `AlreadySignaled` through a `get` whose `None` arm would otherwise make the
  record quietly stop being emitted with every test still passing. Both verified by sabotage: each
  fails the build with its own message.

  **The other five keep the full width, and that is a decision rather than a gap.** Their stubs are
  undocumented and at least `NtWaitForWorkViaWorkerFactory` has been described with different
  argument counts on different Windows versions. Declaring too many reads stale bytes from the
  caller's frame and forwards values the kernel ignores; declaring too few silently drops a real
  argument, and on that stub the result is every pool worker parking with a zeroed parameter -- in
  the exact path this facility exists to observe. So the rule is to narrow only where this crate
  establishes the arity from something it owns: `call_selftest` calls its stub directly, and
  `already_signalled` reads the eighth argument of the other.

  **`HOOKS` no longer stores the replacement function.** With per-stub signatures there is no single
  function type for that column, so the address is reached through a generated `hook_address`; a
  `const` cannot cast a function to an integer, which is why it is a function.

  > **DECISION TO RAISE: narrowing the remaining five needs an arity source good on every supported
  > Windows version.** That is a different kind of work from this item -- acquiring and validating
  > an external contract, not editing a table -- and is not queued. Say if it is wanted.

- [x] **M-T11.7** -- **Make an arity change break something, rather than being discovered by a
      crash.** Asked 2026-10-03, done the same day.

  **The build rung is not available here, and that is the finding rather than a limitation to work
  around.** Arity is a property of the *running* operating system. A build script could read the
  build machine's Windows version, but the binary may run on a different one, so a build-time check
  would be a proxy that cannot establish the fact -- the "cheaper rung that looks like enforcement"
  the house rules name. Runtime is the honest rung.

  **Two guards already existed without being labelled.** `call_selftest` invokes its stub through a
  *three*-argument pointer and the test asserts all three out-parameters are written and ordered,
  so a changed arity fails CI; that is now stated at the call, with a note not to relax those
  assertions into "the call returned", which is the half that would still pass. And the five wide
  hooks are immune to arity *changes* by construction, since twelve is an upper bound -- if
  `NtWaitForWorkViaWorkerFactory` were mis-forwarded the pool would stop dispatching, which is the
  stall this crate's whole suite detects.

  **The real gap was `associate`**, the only hook that interprets a high-numbered argument rather
  than forwarding it. A shifted arity makes its eighth slot some other parameter, and dereferencing
  that raises an access violation *inside the instrument*. `already_signalled` now returns a
  three-way `Signalled` and checks twice before believing the value: the slot must be a possible
  user-mode address, and what it points at must be a `BOOLEAN`. A failure records
  `associate-flag-implausible` carrying the raw argument, because the finding is what the slot held
  instead.

  **Stated limit:** a plausible-looking address may still be unmapped, and no check affordable on
  every wait registration proves otherwise. What the range rejects is the likely shape of a shifted
  argument -- a small integer, which is what this call's other parameters hold.

  **Only one half is swept, deliberately.** The value check fails by assertion and is in
  `sabotage.json`. Removing the *range* check makes the test dereference address 1 and the process
  dies with `STATUS_ACCESS_VIOLATION` -- caught by a crash rather than by a test noticing, which
  this repository treats as uncovered, so it is verified by hand and recorded here rather than
  written down as a guard it is not. Every replacement
      is a twelve-argument `extern "system"` fn, but the stubs take fewer. Forwarding twelve
  arguments *to* a smaller callee is fine; *receiving* twelve from a caller that passed three is
  not -- the hook reads incoming stack slots the caller never had to supply. On x64 those slots
  land inside the caller's own frame, so it reads stale data rather than faulting, which is why
  this has never been seen to misbehave and why it is still wrong. The macro already varies
  per-hook data; arity is one more column.

- [x] **M-T11.6** -- **Make the quiesce account for threads created while it is enumerating.**
      Done 2026-10-03. After the suspend loop, the thread set is swept again and any thread the
      quiesce did not suspend refuses the attempt; `install_batch`'s retry then picks it up in a
      fresh snapshot.

  **`NtGetNextThread`, not a second toolhelp snapshot, and that is forced rather than chosen.**
  Re-enumerating with `CreateToolhelp32Snapshot` allocates, and an allocation with threads
  suspended deadlocks against one suspended while holding the allocator lock -- the hazard this
  phase is built around, which an earlier version of this function hit for real. The export is
  resolved *before* anything is suspended, because `ntdll_proc` allocates and `GetProcAddress`
  takes the loader lock. Unresolvable means refuse, matching the module's posture.

  **The guard needed a test, and finding that out is the point.** Making the sweep blind
  (`known = true`) left all 292 lib tests passing, so the `sabotage.json` entry would have claimed
  `caught` falsely -- caught only by running it rather than assuming. The race cannot be landed on
  demand, so `OMIT_ONE_FROM_SNAPSHOT` produces its *effect*: it drops one id the snapshot did list,
  which from the sweep's side is indistinguishable from a thread created after it.
  `a_thread_the_snapshot_missed_refuses_the_install` asserts both directions, and the blind sweep
  now fails it by name. The
      installer snapshots thread IDs, then suspends that snapshot. A listed thread can create
  another before it is suspended, and the new thread is neither in the snapshot nor suspended, so
  it can execute a stub while its fourteen bytes are being written. The instruction-pointer check
  covers only threads that were successfully suspended. Reach a fixed point -- re-enumerate after
  suspending and repeat until a pass adds nothing -- or refuse, which is the established posture
  for this module when it cannot establish its precondition.

### <a id="m-t12----the-eleventh-review-round"></a>M-T12 -- The eleventh review round

Opened 2026-10-03 by a review of everything on this branch. Four findings; this milestone holds
the one acted on so far.

- [x] **M-T12.1** -- **Submit the repair inline when the healer cannot be started.** Done
      2026-10-03.

  **The gap.** `owe_repair` stamped the entry, called `ensure_running`, and reported the
  cancellation as tracked **whatever that returned**. When the healer could not be started there
  was then a pool marked unhealed with nothing scheduled to visit it, while `try_cancel_pending`'s
  claim that this crate repairs the pool afterwards silently did not hold -- and the untracked
  fail-fast did not fire either, because by its own account nothing had gone wrong. `M-T10.22`
  makes a *later* cancellation retry the start; what it does not cover is a process where no later
  cancellation arrives.

  **The fix is to do what the tick would have done.** `ensure_running` now reports whether a healer
  exists, and when it does not the entry's own pre-created repair is submitted on the spot. That is
  the action measured to release the stall, the work object already exists so it allocates nothing,
  and the only thing given up is the coalescing the timer provides -- a cost this path pays once
  rather than a correctness question. The cancellation is still reported as tracked, which is
  honest: its repair has been handed over rather than scheduled, which is the stronger answer.

  **One site, not one of three.** The first attempt fixed `owe_repair` alone and left
  `owe_repair_claimed` and `owe_repair_untracked` with the old unconditional `true` -- the
  half-converted shape this branch keeps producing. The common tail is now
  `mark_and_arrange_repair`, which all three call, and the submit itself is `submit_repair`, shared
  with `tick_inner` so the order of stamp, count and call cannot drift between them.

  **Guarded in both directions** by `a_cancellation_submits_its_repair_when_the_healer_will_not_start`,
  which holds the tick gate so a submission it observes can only be the inline one: with a healer
  available nothing is handed over, and without one the repair is submitted and counted
  outstanding. In `sabotage.json`, and caught by name when the fallback is reverted.

  **Extracting `submit_repair` broke two existing sabotage anchors**, which the pre-sweep check
  caught before a run was wasted on them. That check is now the habit this branch was missing.

- [x] **<a id="m-t122"></a>M-T12.2** -- **Delete the inline hooking facility: this crate ships no
      undocumented APIs.** Decided and done 2026-10-03 by the engineer.

  **The constraint.** No shipped code may call an undocumented entry point or depend on behaviour
  Microsoft does not publish -- names *and* shape: instruction encodings, structure layouts, fixed
  addresses, argument counts. Recorded as
  [This crate ships no undocumented APIs](../../DESIGN-NOTES.md#no-undocumented-apis), which also
  records what rebuilding the observations on documented ground would take.

  **The surface was wholly confined to one module**, which is why the removal is surgical: all 61
  undocumented references were in `trace/hook.rs` and 11 more in its tests. The threadpool
  wrappers, the obligation model, the self-heal and the trace *record* layer contained none.

  **What went:** `trace/hook.rs`; the hook tests; four public functions (`worker_factory_counts`,
  `completion_port_depths`, `worker_factory_snapshot`, `poke_completion_ports`); the
  `WorkerFactorySnapshot` type; fourteen `sabotage.json` entries; and two integration tests.

  **A real reduction, stated rather than tidied away.** One of those tests,
  `prewarming_makes_the_default_pool_create_a_worker`, was the only external evidence that
  `prewarm_default_pool` causes a worker to be created. That claim now rests on the function's own
  documented contract -- a callback having run is the proof, because it ran on a worker -- which is
  self-evidencing. **I had told the engineer the hook layer's only consumers were its own tests;
  that was wrong**, and the compiler found the two integration tests I had missed.

  **The manifest lost both of its `expect: "survives"` controls**, since both lived in the deleted
  module. A sweep with no control can only say the tests are sensitive, never that they are
  sensitive to the right things, so one was added: strengthening a `Relaxed` load to `SeqCst`
  cannot change observable behaviour, so a suite reporting it CAUGHT is failing for some other
  reason and the sweep's other results cannot be trusted until that is explained.

  **Two of the open review findings are dissolved with the code** -- the quiesce sweep's fail-open
  and the recogniser's 16-byte validation. The third, the trace buffer's `Vec::remove(0)` eviction,
  is in the record layer and survives; see `M-T12.3`.

- [x] **M-T12.3** -- **Make the trace buffer evict in constant time.** Found by the eleventh review
      round; the one finding of that round that the hook removal does not dissolve. Done
      2026-10-03.

  **What landed.** `Buffer` keeps a `head` and overwrites in place, so eviction is one indexed
  write and a modulus. The vector is no longer in logical order after the first wrap, so every
  reader goes through `iter`, which yields `records[head..]` then `records[..head]`; before the
  first wrap `head` is zero and that is simply the vector. `dump` and the tests' `held` helper
  both moved onto it.

  **The modulus is of the vector's own length, not of `capacity`.** The two can differ: a buffer
  that reached a larger capacity keeps that length if a later push passes a smaller one, exactly
  as the remove-and-push it replaces did. Taking `capacity` would index outside the vector.

  **The risk this fix introduces is a rotation, not a loss**, which is why the guard is shaped the
  way it is. An off-by-one head, or an iterator that splits the wrong way, returns the right
  records in the right number and the wrong order -- a capture of a sequence that never happened,
  and nothing about it looks wrong. `the_window_stays_in_order_across_repeated_wraps` pins the
  exact window after every push through several full turns; both failure shapes were verified to
  fail it by name, and the reversed split is in `sabotage.json`.

  **What is deliberately not swept:** restoring `Vec::remove(0)`. It would survive, and correctly
  -- it is slower, not wrong, and no test can assert the absence of a memmove without timing. The
  case against it is analytic and is recorded above rather than measured.

  **The defect.** `Buffer::push` calls `self.records.remove(0)` once the buffer is full, which
  shifts every remaining record. `Record` is about 72 bytes and `CAPACITY` is 262,144, so each
  push past the limit memmoves roughly 18 MB, under the trace mutex.

  **Why it matters more here than the figure suggests.** This is an instrument for observing
  timing-sensitive behaviour. A buffer that fills mid-capture starts perturbing -- and plausibly
  manufacturing -- the very stall it is being used to study. The buffer is deliberately sized not
  to fill in an ordinary capture, which is what has kept this invisible.

  **Shape.** A ring: a head index and wraparound, so eviction and insertion are both constant
  time. `dump` reads from the head rather than from index zero; `dropped` and the
  overflow announcement are unaffected. Watch that the existing buffer tests pin *order* as well
  as contents, since a ring is where that can silently invert.

- [x] **M-T12.4** -- **Unfuse eight doc blocks, and check the half that is checkable.** Found by
      the twelfth review round, which found them by reading; done 2026-10-03.

  **The defect, and why ten rounds missed it.** An edit that replaces an item's documentation by
  anchoring on its signature and prepending new prose leaves the previous block above the new one.
  The result compiles, renders, and passes every gate: rustdoc validates link *syntax* rather than
  prose, clippy does not read doc semantics, and `missing_docs` is satisfied by the presence of *a*
  comment rather than by its being the right one. All eight are mine, all from this branch.

  **Five were a false claim repeated.** The `registration` field on every context type said it sat
  there "so the trampoline can reach it to stamp a dispatch (`M-T6.2`)" -- a mechanism `M-T9.2`
  deleted. In four of them the true text sat directly underneath, so each field stated two
  contradictory things; `wait.rs` had only the false one. `M-T9.2`'s own entry claims "each field
  says so", which was half-true, and the true half is what let the sweep look done.

  **Three left an item with no documentation of its own**: `Registration::owe_repair` and
  `release` had their docs stolen by the item above them, and `ThreadpoolWait::recover_repair`
  carried two `# Safety` sections. `missing_docs` cannot see any of these -- two are `pub(crate)`,
  and the third has a comment, just not its own.

  **One was actively misleading.** `WaitMember::stop_and_drain` carried a section headed "This
  brings a process-wide hazard forward; it does not create it", which is the opposite of what
  draining does. The reviewer read that text as belonging to no existing method and suggested
  deleting it; that was wrong -- `WaitMember` has two cancelling methods, and the paragraph makes
  a group-specific point neither of them stated. It was **relocated** to
  `try_cancel_pending_no_heal_tracking`, whose obligation it describes.

  **The check, and what it deliberately does not cover.** `tests/doc_blocks.rs` errors on a doc
  block carrying the same heading twice: there is no legitimate item with two `# Safety` sections,
  so that signal is sound. It skips fenced regions, because a doctest hides a line with `# ` and
  would otherwise be indistinguishable from a heading. It walks `src` recursively -- a shallow
  walk would skip `heal/`, `timer/` and `trace/` and report success over a fraction of the tree --
  and asserts both a floor on files visited and a floor on headings recognised, so a broken walk
  or a blind parser fails loudly instead of passing vacuously. Its rejecting direction is verified
  by sabotage: re-fusing the block the reviewer found fails it by file and line.

  **The general case is not automated, and that is the decision rather than an omission.** Two
  fused *summary sentences* carry no sound signal. The best heuristic -- a one-sentence paragraph
  with no blank line before it and one after -- flags about twenty-five sites in this crate, of
  which four are real; shipping it would be shipping an alarm nobody can act on. It is a useful
  one-off grep, recorded here, and the sweep it drove found exactly the reviewer's eight and no
  more.

### <a id="m-t13----the-twelfth-review-round-an-uncovered-rule-a-half-converted-twin-and-the-hooks-residue"></a>M-T13 -- The twelfth review round: an uncovered rule, a half-converted twin, and the hook's residue

Nine findings, all confirmed. The round is characterised by *residue*: four of the nine are
leftovers from two of this branch's own larger edits -- the hook deletion in `1eca1e16` and the
heal rework in `M-T9.2` -- that removed a thing without removing what pointed at it. The first
finding is of a different and worse kind: a rule nothing tests.

- [x] **M-T13.1** -- **Make the tick's coalescing skip actually tested.** `tick_inner`'s
  `if !entry.unhealed() { continue; }` is the coalescing rule: a pool that dispatched after its
  cancellation is live and is owed no repair. **Deleting the skip outright leaves all 286 lib
  tests passing** -- measured, not argued. Both tests that exist to cover it are inert:
  `a_tick_leaves_a_pool_its_own_repair_has_answered` asserts `repairs_run() == 0` on the line
  after `tick_inner()`, and `repairs_run` counts repairs that have *run*, which on a fresh private
  pool with no thread yet is asynchronous and still 0 whether or not one was submitted. Its own
  assertion message names the choice -- "on the repair having run rather than on the mark" -- so
  the reasoning is recorded and wrong. `a_tick_leaves_a_pool_alone_when_nothing_is_owed` asserts
  only `!unhealed()`, which is false-by-construction when nothing ever stamped a cancellation, so
  no behaviour of the tick can make it fail.

  **Target:** assert on the synchronous evidence -- `last_submitted()` and `repair_in_flight()` --
  which are written by `submit_repair` before it returns. Add a sabotage entry for the skip, since
  none covers it, and verify the rejecting direction by deleting the skip and watching the test go
  red.

- [x] **M-T13.2** -- **Give the periodic timer one drain body instead of two.**
  `PeriodicTimerMember::stop_and_drain` passes `TRUE` to `WaitForThreadpoolTimerCallbacks` --
  cancel -- while `ThreadpoolPeriodicTimer::stop_and_drain` passes `FALSE` and documents at length
  that "the drain no longer cancels". The member's own doc says "As with
  `ThreadpoolPeriodicTimer::stop_and_drain`", so it cross-references the twin it disagrees with,
  and its trace label still reads `(cancel)`.

  **This is a two-site rule half-converted, and the structural cause is visible in the siblings
  that did not drift:** `TimerMember` and `WaitMember` both delegate to a shared
  `ThreadpoolTimer::stop_and_drain_parts` / `ThreadpoolWait::stop_and_drain_parts`. The periodic
  timer has no such shared body, which is why it was possible to convert one copy and not the
  other. **Target:** extract `stop_and_drain_parts` for the periodic timer and have both call it,
  so the next conversion cannot reach one site only.

- [x] **M-T13.3** -- **Repair the sabotage entry that now invokes undefined behaviour.** The entry
  "the heal creates a work object instead of submitting the pre-created one" passes
  `Box::new(AtomicU64::new(0))` as the fresh work object's context. `repair_trampoline` casts its
  context to `&PoolEntry` and writes stamps and counters through it, so the patched build writes
  past the end of an 8-byte allocation. The entry's `whyNotNullContext` explains that a null
  context was avoided precisely so the result would not be a crash-caught one -- and the context
  type changed underneath that reasoning, so it now risks exactly the outcome it was written to
  avoid, plus heap damage that other tests in the same process would see.

- [x] **M-T13.4** -- **Correct the disarm documentation, which says the opposite of what the crate
  measures.** `ThreadpoolTimer::disarm` says "a callback already queued still runs; use
  `cancel_pending` to drop those as well", and `ThreadpoolPeriodicTimer::stop` says "a tick already
  queued still runs". `disarming_cancels_a_queued_tick_which_a_waits_disarm_does_not` pins the
  opposite for the one-shot timer: with a disarm the queued tick is discarded, asserted as
  `outcomes[1] == 0` against `outcomes[0] == 1` without one. The doc therefore sends a caller to
  `cancel_pending` for something that has already happened.

  **The periodic half needs its own measurement rather than a copied claim.** The pinning test
  drives a one-shot `ThreadpoolTimer`; `ThreadpoolPeriodicTimer::stop_and_drain`'s doc already
  extrapolates from it ("the way a one-shot timer's disarm does"), and this item must either
  measure the periodic type or state plainly that the behaviour is pinned only for the one-shot.

- [x] **M-T13.5** -- **Drop the four `windows-sys` features the deleted hook facility needed.** The
  `trace` feature still pulls `Win32_System_Diagnostics_ToolHelp`, `Win32_System_LibraryLoader`,
  `Win32_System_Memory` and `Win32_System_SystemInformation`, with a five-line comment explaining
  the page-size arithmetic "the hook installer needs" and how `VirtualProtect` reports a whole
  page's previous protection. `1eca1e16` deleted that facility; none of the four has a single use
  left in the crate. A consumer who turns tracing on compiles four modules for nothing, and the
  comment describes machinery that is not there.

- [x] **M-T13.6** -- **Read the error before freeing the context in `ThreadpoolWork::new`.** The
  failure path runs `drop(Box::from_raw(ctx))` and *then* `io::Error::last_os_error()`. Since this
  branch, `ctx` holds a `Registration`, whose drop can reach `PoolEntry::drop` and from there
  `WaitForThreadpoolWorkCallbacks` and `CloseThreadpoolWork`. `timer.rs` and `timer/periodic.rs`
  both capture the error first and free second; `work.rs` is the only one of the four that does
  not.

  **State the finding honestly: the clobber is not established.** Nothing in the Win32
  documentation says those two calls set the last error on success, so this is an inconsistency
  among four sites that should agree rather than a demonstrated bug. It is worth fixing because it
  costs one line and because the ordering that is obviously safe should not be the minority.

- [x] **M-T13.7** -- **Stop crediting a `Drop` impl that does not exist.** Eight comments across
  `heal.rs` (lines 295, 353, 774, 807, 849, 865, 1170) and `heal/tests.rs` (1058) attribute the
  repair object's drain to `RepairWork::drop`. There is no `impl Drop for RepairWork`; the drain
  moved into `PoolEntry::drop` when the teardown-order invariant was made unrepresentable. The
  trampoline's `SAFETY` comment is the one that matters, because it justifies a raw dereference
  with "the entry's first field drains this object in its `Drop`" -- an argument about field order
  that the `PoolEntry::drop` impl deliberately replaced.

- [x] **M-T13.8** -- **Say what the fail-fast child test establishes, which is not the abort.** Its
  doc opens "**This aborts rather than unwinds**, so it cannot be observed with `catch_unwind`",
  and attributes that to the panic escaping the healer's `extern "system"` trampoline. The child
  calls `crate::heal::tick_inner()` directly on the libtest thread, so the panic unwinds normally
  and the child exits 101. Every parent assertion -- not `SURVIVED`, not `SETUP_FAILED`, non-zero,
  stderr text present -- holds under that unwind, so none of them distinguishes an abort from a
  caught panic. The test is worth keeping: it proves the panic fires and that the message reaches
  stderr, which is the claim `fail_fast_if_unrepairable` makes. Only the doc overstates it.

- [x] **M-T13.9** -- **Sweep the hook facility's remaining name.** `trace.rs`'s module doc tells a
  reader the crate records under `wait`, `work`, `io`, `timer` and `timer-periodic`; it also
  records under `heal`, `syscall-enter` and `syscall-leave`, so a reader building a filter from
  that list silently loses the self-heal subsystem. `sabotage.json`'s `whyTestArgs` justifies
  `--all-features` with "the whole hook module is behind the `trace` feature", and three messages
  in `trace/tests.rs` (614, 617, 630) still speak of hooks installing. The `--all-features`
  justification needs replacing with the real reason rather than deleting, since the flag is still
  required.

### <a id="m-t15----a-wait-that-shares-its-handle"></a>M-T15 -- A wait that shares its handle

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/win-sync-sys` -> `WS-M2` -> `WS-2.2`. See
  > [COMPLETED-CHECKLIST.md](../win-sync-sys/COMPLETED-CHECKLIST.md#ws-22).

Items, each archived above under its own heading: [M-T15.1](#m-t151).
