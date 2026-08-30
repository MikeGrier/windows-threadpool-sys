# Checklist: windows-ioring-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-ioring-architecture.md](design-sessions/DESIGN-SESSION-2026-08-22-ioring-architecture.md).
Everything through M18 is archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md): M1-M6
[here](COMPLETED-CHECKLIST.md#moved-2026-08-22----m1-through-m6-ring-lifecycle-through-consumer-documentation),
M7 [here](COMPLETED-CHECKLIST.md#moved-2026-08-23----m7-ring-copy-a-topology-aligned-sample), M11-M14 in their
own dated groups, M8-M10
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m8-through-m10-handle-lifetime-cross-ring-identity-and-the-contract-audit),
and M15-M18
[here](COMPLETED-CHECKLIST.md#moved-2026-08-30----m15-through-m18-the-testing-strategy-response-to-eight-defects).

**`M19` is release-blocking; `M6+` is parked rather than pending** -- see the `M{n}+` convention: it is gated
work with no current obligation, not an unfinished milestone.

## M19 -- Close the `get()` borrow hole (release-blocking for 0.2.0)

Found by the code review of the M15-M18 branch, and **measured**: safe code can hold the `&[u8]` that
`RegisteredBuffers::get` returns across a submit that makes the kernel write into that same buffer. A probe
observed the bytes change from `0x11` to `0xEE` through the live borrow while a *fresh* `get(0)` at that same
instant correctly refused with `WouldBlock`.

The [D-36](DESIGN-NOTES.md#d-36) fix checks `kernel_writes` at the instant of the call, but returns a slice
whose lifetime is tied to `&self` -- and `Batch::read_registered` takes the registration by **shared**
reference, so the borrow and the read coexist. `get_mut` is unaffected: `&mut self` conflicts with the shared
borrow, so the compiler already rejects the analogous sequence. Only `get` is exposed.

This matters more than its severity alone suggests: it is the same hazard class D-36 was filed to close, in
the API whose breaking change 0.2.0 is being cut for, and it is reachable with no `unsafe` anywhere.

- [ ] **M19.1** -- Make the borrow's *existence* conflict with starting a read into that buffer, not merely
  its creation. The options differ in what they cost, and the choice is the engineer's:
  (a) `get(&mut self)`, which is one line and closes it completely, but forfeits the concession D-36
  deliberately kept -- a caller could no longer read a buffer while its own *write* is in flight, which is
  sound because a write means the kernel only reads;
  (b) a `with_bytes(i, |bytes: &[u8]| ...)` callback, or a returned guard type holding a reader count that
  `begin_use(KernelAccess::WritesBuffer)` then refuses against, which preserves (a)'s concession at the cost
  of a wider API change.
  Whichever is chosen, the SAFETY comment on `get` must stop claiming an invariant the signature cannot
  provide -- it currently says the check excludes the kernel writing "for this borrow's life", which is
  exactly the part that is untrue.

- [ ] **M19.2** -- Add the regression test the probe became: hold the borrow across a submit and assert the
  bytes cannot change, or that the sequence no longer compiles. Verify it by sabotage like every other
  instrument in M15-M18 -- reverting the fix must turn it red.

- [ ] **M19.3** -- Sweep for the same shape elsewhere. The M18.1 audit asked what a returned value *permits*,
  not how long its borrow *lasts*; those are different questions and only the first was asked. Re-check every
  borrow-returning entry in [BORROW-SURFACE.txt](BORROW-SURFACE.txt) against the second, and record the
  distinction in [DESIGN-INSTRUCTIONS.md](DESIGN-INSTRUCTIONS.md) so the recurring question covers both.

## M6+ -- Model B: explicit-thread delivery and affinity

Parked, not pending. Deferred by the engineer's explicit direction during the 2026-08-22 design session,
with the plan scoped now so the shape is not lost. This is **not** a fallback for a missing capability
(D-3) -- it is the high-performance architecture, and M4's thread-pool path is the convenient one.

- [ ] **M6+.1** -- `DeliveryMode::{ThreadpoolWait, PinnedThread}` as an explicit consumer choice, never an
  automatic degradation.

- [ ] **M6+.2** -- Resolve the contention between a thread parked in `SubmitIoRing(ring, n, INFINITE, ..)`
  and callers wanting to build SQEs. This is the hard part and the reason this is its own milestone: it
  directly contradicts M3.1's `&mut`-enforced serialization, and needs either a submit-ownership handoff
  or an internal lock. Neither is obviously right.

- [ ] **M6+.3** -- Shutdown: waking a thread parked on `INFINITE`. `IORING_OP_NOP` is supported and is the
  wake mechanism.

- [ ] **M6+.4** -- Affinity: binding a ring's thread with `SetThreadGroupAffinity`, and documenting the
  execution-domain pattern (one pinned thread, its ring, its node-local registered pool, its shard).

- [ ] **M6+.5** -- A test seam forcing the pinned-thread path even where the completion event is available,
  so it stays testable on every machine rather than only on hardware that lacks the feature.

- [ ] **M6+.6** -- Decide `IoBuf`: extract to a shared crate, re-export from
  `windows-overlapped-io-sys`, or leave duplicated (D-1). The merge-or-delete decision that duplicate-then-decide
  defers to the point where the new path is proven -- which is here, not earlier.
