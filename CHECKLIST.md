# Checklist: workspace

Workspace-level and cross-crate work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M6 — Third review round on PR #3

Five findings, raised as suppressed comments on
[PR #3](https://github.com/MikeGrier/windows-threadpool-sys/pull/3) and all verified against the code. Two are
functional defects in safe APIs; three are documents that no longer describe the code they sit next to.

The two defects share a shape worth naming: each is a guard that checks the *stated* precondition rather than
the one that actually matters. A period is checked for zero when what breaks is rounding to zero, and a group's
release is guarded against running twice when what breaks is members arriving after it.

- [x] **RW-1** — Reject periods below one millisecond in `ThreadpoolPeriodicTimer`.

	**Gap:** `new` rejects only a zero period, but every arming converts the period with `Duration::as_millis()`,
	which floors anything under 1ms to `0` -- and `SetThreadpoolTimer` reads a zero period as "do not repeat".
	Measured: a 999us period fires **once** in 300ms, where 1000us fires 31 times. A safe constructor accepts the
	value and silently returns an object that does not do what its type says.

	**Target:** reject below 1ms at construction, consistent with the existing zero rejection, naming the
	millisecond field as the reason. Cover the 999us/1000us boundary in the tests, and mirror the contract on
	`CleanupGroup::create_periodic_timer`.

- [x] **RW-2** — Release cleanup-group members created after a previous release.

	**Gap:** `release_members` latches `released` and returns early forever after. The `create_*` methods take
	`&self`, so once `close_members` has returned, new members can still be created; their contexts are appended
	to `resources`, and both a later `close_members` and `Drop` then skip releasing them. The group is closed
	with live members and the Rust-owned contexts leak.

	**Target:** stop latching. Releasing is already idempotent at the native call, so each release should close
	whatever members exist and free whatever resources are currently tracked, which fixes the leak and makes
	reuse work rather than merely rejecting it.

- [x] **RW-3** — Correct the wait-handle provenance claim in the crate README.

	The safety summary still says `ThreadpoolWait` takes an `OwnedHandle`. It takes a `WaitableHandle`, and that
	difference is the whole point of the change: it is what keeps unsupported handles out of the safe API.

- [x] **RW-4** — Remove the stale `SESSION-CONTEXT.md` snapshot.

	It is a design-phase snapshot added on this branch which states that no safe API has been implemented and
	lists the safety boundary as unresolved. Both are now false. Every contract it records is already in
	[DESIGN-NOTES.md](DESIGN-NOTES.md) or
	[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md)
	(verified before removal), so deleting it loses nothing and stops a contradictory document reaching `main`.

- [x] **RW-5** — Update the operation-identity seam decision to the API that exists.

	[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md) still
	justifies `OperationId::from_ptr`, which the generation-stamped redesign removed. The decision needs to
	describe the seam that replaced it, `OperationId::mint` and `OperationId::from_parts`, and why an identity
	carries a generation.
