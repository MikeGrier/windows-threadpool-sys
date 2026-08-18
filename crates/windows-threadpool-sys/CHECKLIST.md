# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

The crate covers the callback environment, owned private pools, cleanup groups whose members the borrow
checker protects, work objects, one-shot and periodic timers as distinct types, waits that own their handle and
rearm per activation, and the `TP_IO` completion backend over the shared overlapped submission seam.

## M5 — Timer stress suite

The timer types carry the crate's subtlest concurrency contracts: a one-shot's self-re-arm is guaranteed not to
overlap, a periodic's ticks are explicitly allowed to, and both gate re-arming against teardown. The existing
unit tests establish each contract once, deterministically; none of them apply *pressure* to it. This milestone
adds a load suite that does.

The suite is opt-in. CI runs `cargo test --workspace --all-features`, so an ungated load test would run there by
accident, where it would be slow and -- because these scenarios are timing-sensitive under a contended shared
runner -- unreliable. Gating is by environment variable rather than `#[ignore]` so a single knob turns the whole
suite on, and so the tests still compile and lint in CI.

Assertions are restricted to what is actually invariant under load: non-overlap where the type guarantees it,
quiescence after a drain, no hang, and no crash. Rates, latencies, and exact fire counts are **reported**, never
asserted -- under load they are properties of the machine, not of the code.

- [x] **ST-1** — Stress harness plus the one-shot arming and re-arming scenarios.

	The harness owns the gate (`WINDOWS_THREADPOOL_STRESS`), a scale knob (`WINDOWS_THREADPOOL_STRESS_SCALE`)
	that multiplies every load count, and a serialization lane so two heavy scenarios never contend for the
	process-wide pool and distort each other. The gate is applied by a macro rather than a line in each test,
	because a gate that can be forgotten in one test is the failure mode that puts a load test into CI.

	Scenarios: self-re-arm chains asserting the documented non-overlap guarantee, external arming churn from
	many threads, arm/disarm races, past-instant `rearm_at` chains, and coalescing windows under load.

- [ ] **ST-2** — One-shot teardown stress: `Drop` racing a firing and a re-arming callback.

	Directly targets the window closed by the previous review round: create, arm, and drop in a tight loop, and
	drop while a callback is mid-flight with a deferred re-arm pending. This is the scenario that would surface
	a regression in the teardown gate as a hang or a crash rather than as a failed assertion.

- [ ] **ST-3** — Periodic timer stress: high-frequency ticking, self-stop, and deliberate tick overlap.

	The periodic type documents that ticks may overlap and that `stop` neither retracts queued ticks nor affects
	running ones. Load is the only way to exercise those paths in bulk: a short period against a slow callback
	forces overlap, and a self-stopping callback under concurrent external `start`/`stop` exercises the rest.

- [ ] **ST-4** — Cleanup-group timer members and a mixed load scenario.

	A group releases its members' contexts itself, so a group holding many armed timers is a different teardown
	path from dropping the timers individually. The mixed scenario runs one-shots, periodics, and create/drop
	churn concurrently, which is the closest thing to how the crate would be used under real load.

- [ ] **ST-5** — Document how to run the suite.

	The crate README gains a short section: the two environment variables, what the suite covers, and the fact
	that it is deliberately excluded from CI.
