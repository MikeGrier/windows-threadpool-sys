# Parked: rc-errors

Work that is not in scope now: gated on a deliverable (`M{n}+`), on the horizon (`M-inf`), or deferred by a ruling. It lives
here and not in [CHECKLIST.md](CHECKLIST.md), under the workspace's rule "Parked work lives in `PARKED.md`, never in
`CHECKLIST.md`" in [copilot-instructions.md](../../.github/copilot-instructions.md).

## M-inf -- The horizon

- [ ] **RCE-inf.1** -- **Run the crate's tests under Miri**, locally or in CI, for undefined behaviour inside the `unsafe`
  blocks that shows as no wrong pointer, layout or count ([RCE-D-8](DESIGN-NOTES.md#rce-d-8)). Deferred by the engineer,
  2026-10-23: "I don't care to take on the complexity of a nightly toolchain in CI if we can help it. ... I think we can at
  least defer for now." Miri needs a nightly toolchain beside the pinned one.
