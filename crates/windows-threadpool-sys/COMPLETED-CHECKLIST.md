# Completed checklist: windows-threadpool-sys

Append-only archive of completed work items. See [CHECKLIST.md](CHECKLIST.md) for pending work and
[PLANS.md](PLANS.md) for plan status. Design decisions are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md).

## Moved 2026-08-17 — M1 callback environment, M2 work submission, M3 TP_IO backend

### M1 — Callback environment

- [x] **M1-1** — Implement SDK-equivalent `TP_CALLBACK_ENVIRON_V3` initialization and mutation — the
	header-only helpers that `windows-sys` does not emit. See [DESIGN-NOTES.md](../../DESIGN-NOTES.md).

- [x] **M1-2** — Unit-test callback-environment init and mutation against the documented default field values
	(version, priority, size).

### M2 — Work submission and callback ownership

- [x] **M2-1** — Implement and test one end-to-end work submission abstraction over `CreateThreadpoolWork`,
	`SubmitThreadpoolWork`, `WaitForThreadpoolWorkCallbacks`, and `CloseThreadpoolWork`.

- [x] **M2-2** — Validate the callback ownership model with the work abstraction before extending it to timers
	and waits.

### M3 — TP_IO backend over the shared seam

- [x] **M3-1** — Wire the `windows-threadpool-sys` → `windows-overlapped-io-sys` dependency (path plus
	version), and update the publish workflow so the overlapped crate releases before this one.
	*(completed 2026-08-17 15:37:00 -04:00)*

	The publish workflow blocks the `windows-threadpool-sys` job until the required
	`windows-overlapped-io-sys` version is live on crates.io, because `cargo publish`'s verification build
	resolves the versioned dependency from the registry rather than the workspace path. That makes the
	release order independent of the order the two tags are pushed.

- [x] **M3-2** — Implement the `TP_IO` backend over the shared seam with balanced `StartThreadpoolIo` /
	`CancelThreadpoolIo` accounting and callback-driven reclamation.
	*(completed 2026-08-17 15:56:00 -04:00)*

	Added `ThreadpoolIo` and `IoCompletion`. One counter serves as both the pool's start/cancel accounting
	and the crate's rundown state, since an unbalanced start is exactly an operation whose storage the
	kernel or pool still owns. Required widening the shared seam with `OperationId::from_ptr`, which was
	fixed in `windows-overlapped-io-sys` rather than worked around here.

- [x] **M3-3** — Test `TP_IO` across the behavioral matrix: immediate failure, immediate success, pending
	completion, cancellation, and object rundown with operations outstanding.
	*(completed 2026-08-17 16:06:00 -04:00)*

	21 integration tests in `tests/tp_io_behavioral_matrix.rs` covering the five matrix states plus the
	accounting and reclamation invariants at scale (512 file reads, 256 simultaneously outstanding pipe
	reads, 8-thread concurrent submission) and the edge cases: a panicking callback, reads past EOF,
	zero-length reads, mixed payload types, unclaimed completions, and repeated idempotent rundown.

	The matrix surfaced that an operation identity is unique only among *simultaneously outstanding*
	operations — reclaiming an operation returns its storage address to the allocator, which may reissue it.
	That contract was undocumented and is now recorded on `OperationId` and in the overlapped crate's
	design notes.

## Moved 2026-08-17 — M4 safe abstractions and documentation

M4 was restructured during execution. Its original first item bundled work, timer, wait, and I/O, but work had
landed in M2 and I/O in M3, so only timer and wait remained; those became separate implementation and test
items. Two items were added from defects found while building: M4-1 (a `CallbackEnviron` soundness hole) and
M4-8 (the one-shot/periodic timer split).

- [x] **M4-1** — Close the `CallbackEnviron` soundness hole: add an owned `ThreadpoolPool` and change
	`set_pool` to accept it, and make `set_cleanup_group` `unsafe` pending a full cleanup-group design.
	*(completed 2026-08-17 17:00:00 -04:00)*

	`set_pool` and `set_cleanup_group` were **safe** functions accepting a raw `PTP_POOL` / `PTP_CLEANUP_GROUP`
	(bare `isize`) that the thread pool later dereferences, so safe code could cause undefined behavior.
	`set_library` next door was already `unsafe` for exactly this reason.

- [x] **M4-2** — Implement a safe timer over `CreateThreadpoolTimer`, `SetThreadpoolTimer`,
	`IsThreadpoolTimerSet`, `WaitForThreadpoolTimerCallbacks`, and `CloseThreadpoolTimer`.
	*(completed 2026-08-17 17:05:00 -04:00)*

- [x] **M4-3** — Test the timer across one-shot, periodic, absolute, disarming, cancellation, and destruction.
	*(completed 2026-08-17 17:05:00 -04:00)*

	The tests corrected the implementation's specification: `IsThreadpoolTimerSet` reports whether a due time is
	set, not whether the timer will fire again, so a one-shot timer stays set after firing. The `is_set`
	documentation had said the opposite.

- [x] **M4-4** — Implement a safe `ThreadpoolWait` that owns its waitable handle and rearms per activation.
	*(completed 2026-08-17 17:12:00 -04:00)*

	The object owns the handle so it cannot be closed under a pending wait, and the callback receives a
	`WaitActivation` carrying `rearm`, since the SDK consumes the arming on each activation.

- [x] **M4-5** — Test the wait across signalled and timeout activation, rearming, disarming, and destruction.
	*(completed 2026-08-17 17:12:00 -04:00)*

	Also de-flaked the identity tests in both crates: they required the allocator to naturally reuse an address,
	which is not guaranteed. Natural reuse is now reported rather than asserted, and the hazard is covered
	deterministically by tests that synthesize a stale generation at a live address via `OperationId::from_parts`.

- [x] **M4-8** — Split the timer into `ThreadpoolTimer` (one-shot) and `ThreadpoolPeriodicTimer`, and give each
	callback a token. *(completed 2026-08-17 17:25:00 -04:00)*

	The first implementation followed the platform and modelled both kinds with one object, where a `period`
	argument silently changed the concurrency contract. A periodic timer may queue its next tick while the
	previous one is still running, so its callback must tolerate overlapping with itself; a one-shot never
	overlaps. `TimerFiring::rearm_after` gives non-overlapping repetition measured from the end of each firing;
	`PeriodicTick::stop` lets a periodic timer end itself. A zero period is rejected rather than silently
	degenerating to a one-shot.

- [x] **M4-6** — Design and implement safe cleanup-group membership across every callback object.
	*(completed 2026-08-17 17:33:00 -04:00)*

	Option (A) of the three considered: the group creates and owns its members. Flagging objects as
	"group-owned" is insufficient, because each also owns a heap callback context that is only safe to free once
	the bulk release has finished — a moment an individual object cannot observe. Use-after-release is a compile
	error, pinned by a `compile_fail` doc test. Thread-pool I/O is excluded on purpose: a `TP_IO` object must not
	be closed with an operation outstanding, and a bulk release cannot satisfy that.

- [x] **M4-7** — Add API examples and generated documentation.
	*(completed 2026-08-17 17:20:00 -04:00)*

	Crate-level guidance, runnable doc examples for every object type, a rewritten README, and docs.rs metadata
	pinning a Windows target — without which documentation for this Windows-only crate would fail to build.

## Moved 2026-08-17 — M5 timer stress suite

An opt-in load suite for the timer types, gated on `WINDOWS_THREADPOOL_STRESS` and scaled by
`WINDOWS_THREADPOOL_STRESS_SCALE`. 24 scenarios, about a minute at scale 1. See
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) for the decision, and the crate [README.md](README.md) for how to run
it.

Two measurements shaped the whole suite and are recorded in the design note: pool timers fire on the ~15.6ms
system tick, and a loop that arms without pausing outruns the pool entirely -- which made three early scenarios
record zero callbacks while appearing to pass.

- [x] **ST-1** — Stress harness plus the one-shot arming and re-arming scenarios. *(completed 2026-08-17 20:14:49 -04:00)*

	Env-var gate applied by a macro so it cannot be forgotten per test, a scale knob, and a serialization lane
	so two heavy scenarios never measure each other. Scenarios: self-re-arm chains asserting the documented
	non-overlap guarantee, past-instant `rearm_at` chains, arming churn across eight threads, arm/disarm races,
	deterministic arm-fire cycles, a large one-shot population, coalescing windows, and contained panics.

- [x] **ST-2** — One-shot teardown stress: `Drop` racing a firing and a re-arming callback. *(completed 2026-08-17 20:14:49 -04:00)*

	Rapid create/arm/drop churn, drops walking across the due time, drops landing mid-callback with a deferred
	re-arm pending, and concurrent teardown across eight threads. Targets the window closed by the second PR
	review round, where a regression appears as a hang or a crash rather than a failed assertion.

- [x] **ST-3** — Periodic timer stress: high-frequency ticking, self-stop, and deliberate tick overlap. *(completed 2026-08-17 20:14:49 -04:00)*

	Sustained ticking, deliberately overlapping ticks (peak 3 concurrent, confirming the documented contract
	empirically), self-stop from inside the callback, start/stop churn, drops while ticking and mid-tick, a
	large ticking population, and zero-period rejection under repetition.

- [x] **ST-4** — Cleanup-group timer members and a mixed load scenario. *(completed 2026-08-17 20:14:49 -04:00)*

	A group releasing its members is a distinct teardown path: large member populations released as a unit under
	both dispositions of `cancel_pending`, concurrent release across threads, and a group left to drop. The
	mixed scenario runs a self-re-arming one-shot, a periodic population, and timer and group churn together,
	asserting the non-overlap guarantee survives a loaded pool.

- [x] **ST-5** — Document how to run the suite. *(completed 2026-08-17 20:14:49 -04:00)*

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
  [hook.rs](src/trace/hook.rs) records.

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
