# Parked work

Work in scope nowhere yet, held with the gate or the ruling that parked it. The rules for this file are in
the Copilot instructions, under "Parked work lives in `PARKED.md`": an item leaves it by being moved into a
milestone of [CHECKLIST.md](CHECKLIST.md), and the file is deleted when it is empty.

## <a id="m3425-plus"></a>M34.25+ -- The catcher record: keep or remove the Actions cache path (parked behind M34.24)

- [ ] **M34.25** -- Decide whether the Actions cache path for the catcher record stays beside the
  `sabotage-records` branch or is removed.

  **Gate:** [M34.24](CHECKLIST.md), the post-merge run that shows the branch holds a usable record and a
  clone can adopt it. The cache was kept beside the branch on purpose, as a second path so a defect in
  the new one cannot cost a run its record (PLATFORM INTEGRITY rule 1, duplicate then decide); the
  decision is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md#sabotage-records). Until that run, nothing
  says the branch works in CI, which is why the duplicate is not yet debt.

  **What the decision weighs:** removing the cache drops a restore step, a save step, and the
  `record` job's dependence on two stores; keeping it costs a second copy that can disagree with the
  first, which the merge by latest result already tolerates. Either way, the merge-or-delete is made
  when the new path is proven and is not pre-empted by a testing or efficiency shortcut.

## <a id="m45-plus"></a>M45+ -- What a sabotage sweep does with a control that times out (parked behind the first post-merge sweeps after M45)

- [ ] **M45+.1** -- Decide whether a sabotage entry declared `survives` that times out is run again once before
  it is reported caught, or whether a crate whose suite does real I/O gets a manifest `timeoutSeconds` well
  above its baseline, or neither.

  **Gate:** the post-merge sabotage sweeps that run after the M45 branch has landed on `main`. [M45.10](COMPLETED-CHECKLIST.md#m4510)
  re-measured the suites (and cut the serial run from 140 s to 101.6 s), but a stall reaching a control is a property of a
  runner, so only sweeps on runners can show how often it still happens. The question arose from controls in
  `win-durable-ioring` timing out on runners whose tests stalled while idle (post-merge sweeps of
  `c714e83b` and `13f06742`); the engineer, 2026-11-03, chose first to make the suites do less, and this
  is left until it is known how often a stall still reaches a control once they do. A control's patch
  changes nothing, so a timeout on one cannot be a detection; a timeout on an entry declared `caught`
  can be, and stays one either way.

## <a id="m45-plus-publishers"></a>M45+ -- The two publishers of a record to a branch (parked behind the first scheduled long-haul runs)

- [ ] **M45+.2** -- Merge or delete the copy. `tools/publish-probation-results.ps1` and `Get-ProbationBranch` in
  `tools/probation-common.ps1` repeat the approach of `tools/publish-sabotage-catchers.ps1` and
  `Get-SabotageRecordBranch` in `tools/common.ps1`: ask the remote for the branch, build a blob, a tree and a
  commit with plumbing in an index of its own, push a fast-forward, retry from a fresh fetch. Decide whether
  one parameterised publisher serves both, or whether the two stay apart.

  **Gate:** the first scheduled `long-haul.yml` runs on `main`. The probation publisher is a copy so that building it
  left the sabotage harness alone (`DESIGN-NOTES.md`, "probation-results"), and it is to be judged after the scheduled
  workflow has run it against the real remote, which nothing in M45, [M45.10](COMPLETED-CHECKLIST.md#m4510) included, does. A merge edits `tools/common.ps1`, which every
  sabotage sweep takes as an input.
