# Checklist: workspace

Workspace-level and cross-cutting work. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

Two completed groups govern how contracts are written and maintained here, and are worth reading before
touching one:
[M1 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m1) recorded the
[ten specification-gap categories](DESIGN-NOTES.md#specifying-a-delivery-contract) -- what a contract fails to
say -- and [M2 (2026-08-27)](COMPLETED-CHECKLIST.md#moved-2026-08-27-m2) addressed
[restatement drift](DESIGN-NOTES.md#restatement-drift), the separate failure mode where a *correct* rule fails
to reach every place that states it.

M1's follow-on audits are tracked per crate, not here:
[windows-overlapped-io-sys M14](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[windows-ioring-sys M10](crates/windows-ioring-sys/CHECKLIST.md)
(windows-file-watcher's M14 is complete).

## M3 -- Make the sequencing rules executable too

[M2](COMPLETED-CHECKLIST.md#moved-2026-08-27-m2) made the *value-level* contract facts derived rather than
restated, and recorded that sequencing rules -- bracket entry states, what may follow what, terminality of an
exchange -- would stay prose. That was too pessimistic: what cannot express them is the **type system**, not
the codebase. A shared executable oracle can, and it is the same "derive, don't restate" move at runtime.

The evidence that they need it: **eight** tests in the harness's
[generator_is_reproducible.rs](crates/windows-file-watcher-example-test-harness/tests/generator_is_reproducible.rs)
each hand-encode a sequencing or cross-message rule, and one of them already asserts something the M14.2 audit
found is false of the contract. Separately, **nothing validates that the real watcher's output is
contract-legal** -- the harness checks the generator, and the crate's own integration tests make point
assertions.

- [ ] **M3.1** -- Fix `first_two_notifications_of_a_liveness_watch_are_established_then_subscribed`, which is
  a defect independent of the rest of this milestone: it asserts as a *contract* rule something
  [M14.2](crates/windows-file-watcher/COMPLETED-CHECKLIST.md) established is **not** universally true -- a
  route coalescing onto an already-faulted watcher sees `Completion { Subscribed }` first and its
  `Established` only after recovery. The test passes solely because the generator never produces that case,
  so it is a property of the generator wearing a contract name. Rescope and rename it to say which it is.

- [ ] **M3.2** -- Add a per-watch `ContractChecker` to `windows-file-watcher` behind `test-util`: a state
  machine over one subscription's notification stream (`NotYetEstablished` / `Live { mode }` /
  `Faulted { question_outstanding }` / `Ended`), with `observe(&Notification) -> Result<(), ContractViolation>`.
  It belongs in the **crate**, not the harness: one definition then serves the crate's own tests, the
  harness, and a consumer validating its own test doubles, where a harness-side copy would be a second
  implementation again -- the exact mistake M1 and M2 exist to stop. Derive each transition from the M14
  audit rather than from how the watcher currently behaves, or it blesses today's accidents instead of the
  contract; cite the decision on each.

- [ ] **M3.3** -- Adopt the checker in `windows-file-watcher`'s own integration tests, so the **real**
  watcher's output is validated against the contract rather than only spot-checked. This is the item most
  likely to find a behavioural defect rather than a documentation one, since nothing has ever asserted this.

- [ ] **M3.4** -- Make the harness bind to the checker and collapse the hand-written sequencing invariant
  tests into "generate, then validate". Each collapsed test is one fewer restatement that can drift from the
  contract, which is the whole point; keep any test that genuinely asserts a *generator* property (coverage,
  reproducibility) and say so in its name.
