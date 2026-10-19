# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M8 -- Every probe's normal run emits a row of raw data

Applies [Every probe's normal run emits a row of raw data](DESIGN-NOTES.md#d-every-probe-emits-a-row),
decided by `M4.10`. Each row carries what the probe observed, not a verdict; it is not a formal
contract, and a row whose meaning later changes is renamed rather than reused. Where a probe's prose
and row render the same figures, derive both from one list as the cost probes do (`M2.9`). Where a
probe can decline to measure, its item decides what the row says, with topology's
`report_unmeasured` as the example to follow or to depart from on purpose.

Each row item also moves its probe into `EMIT_A_ROW` in
[tests/the_probes_that_emit_a_machine_readable_row.rs](tests/the_probes_that_emit_a_machine_readable_row.rs),
which fails until it does.

- [x] **M8.1** -- Run `probe-queue-contention` in the row census with a short sampling, replacing the exclusion whose reason `M4.2` made false. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m81)

- [x] **M8.2** -- `probe-queue-contention` emits a row.
- [x] **M8.3** -- `probe-peer-index-cache` emits a row.
- [x] **M8.4** -- `probe-core-affinity` emits a row, including when it declines to measure.
- [x] **M8.5** -- `probe-pool-growth` emits a row.

- [x] **M8.6** -- Both long-path probes emit one `x-probe-long-path` row, told apart by `manifest_aware`. -> [completed 2026-10-19](COMPLETED-CHECKLIST.md#m86)

- [x] **M8.7** -- `probe-error-mode` emits a row.
- [x] **M8.8** -- `probe-handle-state` emits a row.
- [x] **M8.9** -- `probe-worker-context` emits a row.
- [x] **M8.10** -- `probe-device-map` emits a row.
- [x] **M8.11** -- `probe-ioring` emits a row.
- [x] **M8.12** -- `probe-completion-port` emits a row.
- [x] **M8.13** -- `probe-cancel-io` emits a row, including for a case its watchdog abandons.

- [ ] **M8.14** -- Make the row census assert that every registered probe emits a row, so a probe
  added later without one fails rather than joining an exception list.
