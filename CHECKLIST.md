# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M7 — Fourth review round on PR #3

Ten findings on [PR #3](https://github.com/MikeGrier/windows-threadpool-sys/pull/3), all verified against the
code before being planned. Nine were correct as stated; one was correct about the problem but named the wrong
method, and the measurement is recorded with the item so the record is not quietly "reviewer was right".

Several are the same defect this project keeps producing: a value is *accepted* by a safe API and then not
honoured. A period that rounds away, a buffer length that truncates, a counter that wraps. Each is fixed the
same way -- reject what cannot be honoured -- so the API never returns an object that does something other than
what it was asked for.

- [x] **TR-1** — Give the timer and the wait a real stop-and-drain.

	**Gap:** the review claimed `cancel_pending` cannot quiesce a self-re-arming object. Measured, that is not
	what happens: `disarm()` + `cancel_pending()` *is* quiescent, as is `cancel_pending()` alone. The method that
	fails is [`wait()`](crates/windows-threadpool-sys/src/timer.rs) -- after `disarm(); wait();` a self-re-arming
	timer was still set and fired four more times -- and its documentation promises exactly the quiescence it
	does not deliver ("block until all queued and executing callbacks have completed").

	`cancel_pending`'s success is also not ours to rely on: it works because the pool happens to drop a callback
	armed by the trampoline during an in-flight cancel, which no SDK contract promises. Per the design-autonomy
	rule, quiescence must be guaranteed by our own mechanism.

	**Target:** a `stop_and_drain` on both types that suppresses re-arming under the same lock `Drop` uses,
	disarms, drains, and then lifts the suppression so the object stays usable -- matching
	`ThreadpoolPeriodicTimer::stop_and_drain`, which already exists and makes the current asymmetry an
	inconsistency as well as a gap. The suppression becomes a depth count rather than a flag, so concurrent
	callers and a later `Drop` compose instead of one lifting another's suppression. Fix the `wait()` and
	`cancel_pending` documentation to state what each actually guarantees.

- [x] **TR-2** — Reject periods a periodic timer cannot honour exactly.

	**Gap:** [`M6`](COMPLETED-CHECKLIST.md) added a lower bound but nothing else. Every start still converts the
	period with `as_millis()` and clamps to `u32`, so a 1.5ms period is *reported* as 1.5ms and *scheduled* at
	1ms, and any period above `u32::MAX` ms is silently shortened.

	**Target:** reject a period that is not a whole number of milliseconds, and one too large for the field, so
	`period()` can never disagree with what was scheduled. Same treatment as the lower bound: reject rather than
	silently substitute.

- [x] **TR-3** — Fail generation minting at exhaustion instead of wrapping.

	**Gap:** `OperationId::mint` uses `fetch_add`, which wraps at `u64::MAX` and then reissues generations from
	zero. The type states that a (address, generation) pair names exactly one submission *for the life of the
	process*, which wraparound would break -- and stale-identity aliasing is precisely what generations exist to
	prevent.

	**Target:** panic at exhaustion rather than wrap, stickily, so a caught panic cannot resume handing out
	recycled generations. Exhaustion stays unreachable in practice (centuries at one submission per nanosecond);
	the point is that the invariant is enforced rather than asserted in prose. Make the counter injectable so the
	boundary is actually testable.

- [x] **TR-4** — Reject ioctl buffers too large for the Win32 length field.

	`clamp_u32` silently caps input and output lengths at `u32::MAX`, so a larger buffer submits only a prefix
	while the API's signature and documentation accept the whole slice. Return `InvalidInput` instead, before
	allocating the output buffer.

- [x] **TR-5** — Gate `windows-threadpool-sys` behind `cfg(windows)` like its sibling.

	The root [README.md](README.md) states that platform-specific code lives behind `cfg(windows)`.
	`windows-overlapped-io-sys` gates every module; `windows-threadpool-sys` declares its modules
	unconditionally, so on a non-Windows target it fails to compile rather than resolving to an empty crate.
	Gate it to match, and verify by checking against a non-Windows target rather than by inspection.

- [x] **TR-6** — Correct two API documents that describe behaviour the code does not have.

	`ThreadpoolIo::new` offers a cleanup group as an environment option, which the design deliberately excludes
	-- a `TP_IO` object must not be closed with an operation outstanding, which bulk release cannot guarantee.
	`CallbackEnviron::clear_pool` says it drops the `ThreadpoolPool` the environment named; it clears the
	selection and drops nothing.

- [ ] **TR-7** — Archive the completed plans per the repository's own convention.

	Both crates' [PLANS.md](PLANS.md) files keep their checklists in the active table marked "completed". The
	convention is that a completed checklist moves to a `COMPLETED-PLANS.md` table in the same directory and
	leaves the active file. Neither archive exists. Create them and move the rows, at both crate level and the
	workspace root, which has the same problem.
