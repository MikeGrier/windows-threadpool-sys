# Checklist: windows-file-watcher

Memory-safe Windows path-change watcher. The design session that opened the crate recorded D-1...D-20 in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md).
The authoritative Tier-1 set is [DESIGN-NOTES.md](DESIGN-NOTES.md), which now runs to **D-59** -- later
decisions (D-21 from M1 review, D-22...D-26 and D-34/D-35 from M2, D-36...D-49 from M3, D-50...D-52 from M4,
D-53...D-59 from M5, D-32 from M8.1, and D-25/D-27...D-31 plus D-33 from the [2026-08-21 fault-protocol session](design-sessions/DESIGN-SESSION-2026-08-21-fault-protocol-and-doorbells.md),
which **overturned D-16**) are added there as milestones complete.

Work items are dependency-ordered. Each milestone ends with integration tests. The implicit
end-of-milestone gate (default **and** `--all-features` build/test/clippy/doc clean, encoding check, sync
with origin) is standard procedure and is not listed as an item.

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

> **NEXT ACTIONABLE ITEM: M6.1.** M1 through M5 are archived; nothing else is in progress. Note that a
> *satisfied cross-component prerequisite does not make its item startable* -- M17 in
> `windows-threadpool-sys` cleared the external dependency for M6.1, and M5's fault machine now exists for
> M6 to extend with the downgrade-to-coarse edge; work the milestones in order.

## M4 -- Coalescing by directory and file targets

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m4-coalescing-by-directory-and-file-targets).

## M5 -- Fault model and the retry protocol

Archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#moved-2026-08-21----m5-fault-model-and-the-retry-protocol).

## M6 -- Coarse fallback

- [ ] **M6.1** -- Coarse handle: `FindFirstChangeNotification`, owned and closed with
  `FindCloseChangeNotification` (not `CloseHandle`), reaching `ThreadpoolWait` through the custom-close
  waitable owner -- a std `OwnedHandle` would be closed with `CloseHandle` by the pool on teardown,
  which is the wrong routine for a change-notification handle. The two-tier arrangement this serves is D-17;
  the ownership mechanism itself is a `windows-threadpool-sys` decision, recorded in the workspace-root
  [DESIGN-NOTES.md](../../DESIGN-NOTES.md) under "A wait target owns its close routine".

  > **CROSS-COMPONENT PREREQUISITE -- SATISFIED 2026-08-21:** component `crates/windows-threadpool-sys` ->
  > M17 (custom-close owner for non-`CloseHandle` wait targets, across both the direct and `CleanupGroup`
  > teardown paths) has landed. See
  > [../windows-threadpool-sys/COMPLETED-CHECKLIST.md](../windows-threadpool-sys/COMPLETED-CHECKLIST.md).
  > The seam to use is `WaitableHandle::assume_waitable_with(raw, FindCloseChangeNotification)`, which works
  > with both `ThreadpoolWait::new` and `CleanupGroup::create_wait`.
  >
  > **This clears only the external dependency.** M6.1 is still gated by M2 through M5 in the ordinary
  > dependency order of this crate and is not startable ahead of them.

- [ ] **M6.2** -- Coarse watcher: `ThreadpoolWait` per activation -> emit `Desync { Coarse }` to the
  directory's subscriptions -> `FindNextChangeNotification` re-arm, under the same fault/backoff discipline
  (D-15/D-17).

- [ ] **M6.3** -- Downgrade edge in establish (D-17): an unsupported-class error (`ERROR_INVALID_FUNCTION` /
  `ERROR_NOT_SUPPORTED`) transitions to coarse establishment; the mode is re-resolved on each
  establish/re-establish; retryable errors still use the reopen loop.

- [ ] **M6.4** -- `Established { mode }` opt-in report (D-13), plus a test seam to force coarse mode
  regardless of the underlying volume.

- [ ] **M6.5** -- Integration: force coarse via the seam -> assert `Established { Coarse }` and that mutations
  surface as `Desync { Coarse }`; assert coarse teardown closes the notification handle correctly.

## M7 -- Documentation, examples, stress

- [ ] **M7.1** -- A crate README and the [lib.rs](src/lib.rs) top-level docs: the monitor/session/watch model, the
  two queues and their doorbells (D-25), the fidelity-and-limitation contract, the `Desync` primitive, and
  the D-27 retry protocol including how to choose between defaults and interactive at registration.

- [ ] **M7.2** -- Runnable examples: a minimal directory watch, a single-file watch, and a fault-recovery
  demonstration.

- [ ] **M7.3** -- Finalise Tier-1 [DESIGN-NOTES.md](DESIGN-NOTES.md) / Tier-2 [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) from the session, with
  every shipped decision cross-referenced.

- [ ] **M7.4** -- Opt-in, env-gated stress suite: change churn, fault storms (repeated delete/recreate),
  teardown races, and coalesced multi-subscription load.

- [ ] **M7.5** -- Publication readiness: crate metadata, changelog, and a final review pass over the public
  surface for the v1 scope (D-18) and the deferred seams (D-19).

## M8 -- Adopt wtf-string for relative names

- [x] **M8.1** -- Add the [wtf-string](../wtf-string/README.md) dependency (published; pin the current
  release) and migrate `RelativeName` from its hand-rolled `Box<[u16]>` to `Wtf16Str` / `Wtf16String`, so
  decoded names carry the native-`u16`, conversion-free representation and feed Windows APIs without
  re-encoding. Preserve the lossless `OsString`/`Path` and raw-`&[u16]` surface (D-8).

  > **PULLED FORWARD out of dependency order, deliberately.** M8.1 is a *representation* change to a type
  > every later milestone touches, so each milestone left ahead of it would be more code to migrate later.
  > Done after M2.3 rather than after M7; the rest of M8 stays here. Nothing in M8.1 depended on M3 through
  > M7 -- it only ever needed the wtf-string crate, which had already shipped.

- [ ] **M8.2** -- Integration test: after adoption, decode a real completion buffer and assert the relative
  name's raw `&[u16]` units, its lossless `OsString`/`Path` conversion (including an unpaired surrogate), and
  a direct wide-pointer (`as_ptr()`) hand-off to a Windows API, verifying the representation change preserves
  the public lossless-conversion contract (D-8). The unit tests added with M8.1 already cover the terminated
  pointer against `lstrlenW` on a synthetic buffer; this item is the same assertions against a buffer a real
  `ReadDirectoryChangesW` produced.

## M-inf -- Horizon (ungated, post-v1)

Parked, not pending. These are the deferred seams recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> D-19,
an explicit design decision that places them outside the v1 scope. That recorded decision -- not the
absence of a current consumer -- is why each is deferred, which is a legitimate deferral rationale (a
resolved, recorded scope decision), not a scope-boundary excuse. Each graduates to a numbered milestone
when a post-v1 line of work takes one up. None is an open obligation of any current milestone.

- [ ] **M-inf.1** -- `ReadDirectoryChangesExW` extended records (`FILE_NOTIFY_EXTENDED_INFORMATION`): surface the
  richer per-record fields on OS versions that support it, behind capability detection, without disturbing
  the basic `FILE_NOTIFY_INFORMATION` surface (D-18/D-19). **Availability is per-filesystem, not merely
  per-OS-version:** even on a build that exposes the API, some filesystems reject the extended structure --
  e.g. ReFS still does not support it (for no good reason) -- so detection must probe the actual target
  volume and fall back to `FILE_NOTIFY_INFORMATION`, never inferring support from the OS version alone.

- [ ] **M-inf.2** -- Digest-based change *verification*: an optional mode that confirms a reported change by
  hashing content, trading cost for fewer spurious notifications (D-19).

- [ ] **M-inf.3** -- Per-volume capability cache: remember detailed-vs-coarse (and extended-record) support per
  volume so establish/re-establish need not re-probe each time (D-17/D-19).
