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
