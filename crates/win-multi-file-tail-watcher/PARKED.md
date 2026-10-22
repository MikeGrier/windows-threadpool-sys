# Parked: win-multi-file-tail-watcher

Work not in scope now. Nothing here is pending; an item leaves only by graduating into a milestone of
[CHECKLIST.md](CHECKLIST.md), or by a ruling that withdraws it.

## M2+ -- Decisions to revisit once M2 is done

Parked, not pending: the milestone that is next after M2 pulls these in.

- [ ] **TW-2+.1** -- **Decide whether the diagnostic sink's machinery moves to a crate both this one
  and durable-ioring use**, generic over the event type, or whether this crate's mirror stays. The
  mirror is deliberate duplication, so that the sink could be built without touching another
  crate's layering; the decision is to merge it or to delete the question, and is made when there
  is more than one event to compare. durable-ioring's `StderrDiagnostics` and `FanOutDiagnostics`
  were not mirrored, so shared machinery would bring them here. ([TW-D-13](DESIGN-NOTES.md#tw-d-13))

- [ ] **TW-2+.2** -- **Decide whether the buffers give chunks back after an unusually large poll**, and how many they keep. Needs a measurement the captures do not have: a roster that grows and then shrinks, polled across the change. ([TW-D-14](DESIGN-NOTES.md#tw-d-14))
