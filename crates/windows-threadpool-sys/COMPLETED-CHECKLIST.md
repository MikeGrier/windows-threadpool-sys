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

> **-> CROSS-COMPONENT HANDOFF:** M17 is complete, which unblocks component `crates/windows-file-watcher`
> -> M6 -> M6.1 (the coarse `FindFirstChangeNotification` watcher). See
> [../windows-file-watcher/CHECKLIST.md](../windows-file-watcher/CHECKLIST.md).
