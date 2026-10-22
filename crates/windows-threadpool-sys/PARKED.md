# Parked: windows-threadpool-sys

Work not in scope now. Nothing here is pending; an item leaves only by graduating into a milestone of
[CHECKLIST.md](CHECKLIST.md), or by a ruling that withdraws it.

## M-inf -- Diagnostic work with no gating deliverable

- [ ] **M-T-inf.1** (was `M26.14.4`) -- **Find the threshold the close races.** `M26.14.2` used 1ms because it is
  about 80x the natural 12us gap; nothing establishes what the minimum is. A sweep -- 0, 10us,
  50us, 100us, 500us, 1ms, interleaved at 20000 each -- would say whether the window is
  microseconds or milliseconds, which is itself evidence about what the close races.

  **Moved here 2026-10-01, when its gate resolved the "never ships" way.** This item was gated on
  `M26.14.3` and said so itself: if the draining teardown were adopted, the sleep never ships and
  the sweep is only of diagnostic interest. It was adopted, so that is where this now sits -- not
  cancelled, because the question it asks is about the *kernel* window the close races, which the
  drain avoids rather than explains, and that remains the one thing about this stall nobody has
  been able to see directly.
