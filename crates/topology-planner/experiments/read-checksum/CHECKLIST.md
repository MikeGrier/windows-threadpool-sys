# Read/checksum experiment

## RC: first comparison

- [ ] **RC-1** -- Implement and exercise separate direct-owner, bounded-handoff, and
  independent-direct-worker arrangements under [DESIGN-NOTES.md](DESIGN-NOTES.md).
  Use one overlapped-file backend and checksum, fixed aggregate read depth and buffer
  allowance, per-block reference validation, balanced repetitions, explicit processor
  binding, observed buffer-page placement, CPU/wall timing, latency and pressure data.
  Add deterministic unit cases and live fixture/error tests, capture a bounded release
  run, and retain its structured evidence. Compare observations without choosing a
  universal winner; record retain/revise/delete decisions for each experimental path.
  > **CROSS-COMPONENT PREREQUISITE:** parent `topology-planner` -> `EP-R1.7`
  > selected the experiment and its constraints; see [CHECKLIST.md](../../CHECKLIST.md).
  > **-> CROSS-COMPONENT HANDOFF:** return to `topology-planner` -> `MR1` ->
  > `EP-R1.7` to use the observations when refining the workload and archetype contract.

## RC1+: additional experimental dimensions

- [ ] **RC-2** -- Extend the first comparison with independently paced arrivals,
  batch-size and cache/device-path controls, and broader placement and working-set
  sweeps. Define each measurement contract before running it; do not infer these
  results from the closed-loop buffered-file capture.
