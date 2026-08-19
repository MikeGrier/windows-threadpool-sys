# Design session — windows-file-watcher (2026-08-18)

A memory-safe Windows path-change watcher with full Windows fidelity for path
names and notification *limitations*. This is deliberately Windows-only;
platform independence is built at a higher layer, not here.

This session settled decisions **D-1 … D-20** below. It builds directly on the
two crates already in this workspace: [windows-overlapped-io-sys](../../windows-overlapped-io-sys/README.md)
(owned overlapped endpoints, IOCP, and generation-stamped operation identities)
and [windows-threadpool-sys](../../windows-threadpool-sys/README.md)
(`ThreadpoolIo` / `ThreadpoolWait` / `ThreadpoolTimer` / `ThreadpoolWork` /
`CleanupGroup`).

Reference studied: the C++ `Azure/m` filesystem monitor (the participant was its
author). Its state-machine shape is the good part; several of its behaviours are
explicitly *not* reproduced (see "Hazards from Azure/m").

---

## Decision index

- **D-1 — Scope.** Windows-only, memory-safe watcher over `ReadDirectoryChangesW`
  with a `FindFirstChangeNotification` coarse fallback. No cross-platform surface.
- **D-2 — Queue-mediated architecture.** A `Monitor` hands out `Session`s; a
  session bundles a request-submission handle **and** the notification sink.
  Every client interaction is a queued request or a queued notification. **No
  client code ever runs on a monitor / threadpool thread.**
- **D-3 — No owned threads.** All work runs on `windows-threadpool-sys`
  (`ThreadpoolIo` for detailed reads, `ThreadpoolWait` for coarse waits,
  `ThreadpoolTimer` for retry backoff, `ThreadpoolWork` to drain the request
  queue, `CleanupGroup` for teardown).
- **D-4 — Reuse the overlapped seam.** Detailed reads are issued through
  `windows-overlapped-io-sys`; the generation-stamped `OperationId` solves the
  re-arm aliasing hazard (a stale completion cannot be misattributed to the next
  read that reuses the same `OVERLAPPED`).
- **D-5 — Affine watch handle.** Subscribing returns an owned, move-only
  `#[must_use]` `Watch`. `Drop` enqueues cancellation; `watch.cancel()` is the
  explicit deterministic form. A `Copy` `WatchId` correlation token tags every
  notification so a client can route/aggregate without holding the lifecycle
  object. (Rust is affine by nature; true linearity is neither available nor
  needed.)
- **D-6 — Coalesce by directory.** One watcher per directory regardless of how
  many subscriptions target entries within it. It issues **one** read with the
  **union** of `FILE_NOTIFY_CHANGE_*` filters and the **max** subtree flag, and
  de-multiplexes decoded records to the matching subscriptions.
- **D-7 — Path targets.** A subscription targets a *path*. A **file** target is
  watched by opening its **parent** directory non-recursively and filtering the
  leaf name (the capability `Azure/m` lacked). A **directory** target is watched
  directly, optionally recursively.
- **D-8 — Names relative and lossless.** Names are delivered **raw and relative**
  to the watched target. `OsString` / `Path` is the primary surface (WTF-8
  round-trips arbitrary UTF-16 losslessly, including unpaired surrogates and
  `> MAX_PATH`); a raw `&[u16]` escape hatch is available.
- **D-9 — Raw actions, no rename joining.** `FILE_ACTION_*` map to distinct
  kinds including `RenamedOldName` / `RenamedNewName`; the crate never pairs them
  and never joins across a buffer boundary. Clients pair if they wish.
- **D-10 — Batch delivery.** One decoded `ReadDirectoryChangesW` completion is
  delivered as one batch of records.
- **D-11 — `NotificationSink` contract.** `trait NotificationSink: Send + Sync`
  with a non-blocking, infallible `deliver(&self, batch)`. The crate forces
  **multi-producer** safety (a session sink aggregates several subscriptions,
  whose completions run on different pool threads) — i.e. **MPSC minimum**.
  Consumer cardinality is entirely the client's business (single- or
  multi-drain; MPMC only if *they* choose). Delivery must never block the
  cadence.
- **D-12 — `Desync` is the fidelity primitive.** A cause-tagged
  `Desync { cause: Overflow | QueueFull | Coarse | Reestablished }` means "there
  is a hole in your event set — re-scan." Kernel-buffer overflow (0-byte
  completion / `ERROR_NOTIFY_ENUM_DIR`), a full client queue, coarse-mode
  activations, and the gap across a fault outage **all collapse to this one
  signal**.
- **D-13 — Liveness/observability is opt-in.** `Suspended` / `Resumed` brackets
  (watch went non-live / live again) and `Established { mode: Detailed | Coarse }`
  are opt-in per subscription. A client that ignores them still behaves
  correctly (it need only honour `Desync`).
- **D-14 — No terminal fault state.** There is only "not yet re-established." The
  monitor retries autonomously and indefinitely; the client may cancel from any
  state. A target that supports neither API simply stays in the establishing/
  retry state until cancelled (no special terminal case).
- **D-15 — Recovery cannot self-fail.** Every error classifies into
  *reopen-and-retry*, *rearm-and-retry*, or *downgrade-to-coarse*. Nothing throws
  or terminates the sequence; the only failure edges are retryable Windows
  syscalls (`CreateFileW`, `ReadDirectoryChangesW`, `FindFirstChangeNotification`).
- **D-16 — Retry policy is resident data, not a callback.** Backoff is a plain
  value (initial delay, multiplier, cap, optional jitter, optional per-error-kind
  overrides). A monitor-level default is overridable per subscription. It is set
  and updated **only** through serialized request-queue items, and read by the
  monitor's single serialized fault handler. There is **no reactive per-fault
  message and no closure on the cadence path**, so there is nothing to race and
  no client code can stall recovery.
- **D-17 — Two-tier watcher; downgrade is a volume property.** Detailed
  (`ReadDirectoryChangesW` + `ThreadpoolIo`) is preferred; Coarse
  (`FindFirstChangeNotification` + `ThreadpoolWait`) is the universal floor. The
  mode is resolved **at establish / re-establish** by attempting to arm the
  detailed read; an unsupported-class error (`ERROR_INVALID_FUNCTION` /
  `ERROR_NOT_SUPPORTED`) downgrades to coarse for that directory, a retryable
  error uses the reopen loop instead. Coarse activations carry no detail and thus
  emit `Desync { Coarse }`. The coarse handle is closed with
  `FindCloseChangeNotification` (not `CloseHandle`) and reaches `ThreadpoolWait`
  via `WaitableHandle::assume_waitable`.
- **D-18 — v1 record scope.** Basic `FILE_NOTIFY_INFORMATION` for v1.
- **D-19 — Deferred seams.** `ReadDirectoryChangesExW` /
  `FILE_NOTIFY_EXTENDED_INFORMATION` (file id, timestamps, size); digest/hash
  based change *verification* on top of coarse mode (left open for a future
  contributor — trivial for a single file, complex for recursive directories);
  and an optional per-volume capability cache to skip re-probing detailed on
  volumes already known coarse.
- **D-20 — Blocking teardown.** `Monitor::Drop` runs a full rundown — cancel and
  drain every outstanding read/wait, then free every context — inheriting the
  `windows-threadpool-sys` teardown discipline (cancel/suppress re-arm, then
  drain, then free).

---

## Object model

```
Monitor ──creates──> Session ──creates──> Watch (affine RAII, WatchId)
   │                    │  \
   │                    │   └─ notification sink (client-provided or default)
   │                    └─ request-submission handle (MPSC producers)
   │
   └─ per-directory watchers (coalesced): Detailed | Coarse
        Detailed: dir handle + ThreadpoolIo + ReadDirectoryChangesW
        Coarse:   FindFirstChangeNotification handle + ThreadpoolWait
```

- **`Monitor`** — top-level manager. Owns the per-directory watchers and the
  serialized fault/servicing state. Created once; `Drop` blocks on rundown.
- **`Session`** — obtained from the monitor (`monitor.session(sink)` or a
  default-sink variant). Bundles the request-submission handle and the sink. All
  `Watch`es made through a session deliver to that session's sink.
- **`Watch`** — affine handle for one subscription (D-5).
- **`WatchId`** — `Copy` correlation token on every notification.
- **`NotificationSink`** — client-providable delivery target (D-11), with a
  crate-provided bounded default that emits `Desync { QueueFull }` on overflow.

## Notification model

A batch carries records of `{ WatchId, kind, relative-name }` where `kind` is the
raw `FILE_ACTION_*` mapping (D-9). Control items — `Desync { cause }` (D-12) and
the opt-in `Suspended` / `Resumed` / `Established { mode }` (D-13) — ride the same
sink so ordering with data is well-defined per subscription. Ordering is
in-order **within** a subscription; **no** cross-subscription ordering when
several share a sink (concurrent producers).

## Fault / establish state machine (per coalesced directory)

```
        ┌─────────────────────────────────────────────┐
        v                                             │ (retryable error → backoff timer)
  Opening ──ok──> ArmingDetailed ──ok──────────> WatchingDetailed ──completion──> (decode → re-arm)
     │                  │                              │
     │(retryable)       │(unsupported-class)           │(retryable I/O error)
     │                  v                              v
     └──backoff──┐  ArmingCoarse ──ok──> WatchingCoarse ──signal──> Desync{Coarse} → re-arm
                 │       │(retryable)          │(retryable)
                 └───────┴─────────────────────┴──── backoff timer ────┐
                                                                        │
   Cancelling ──(from ANY state; client Drop/cancel)──> drain → Closed  │
                                                                        v
                                                          (retry re-enters Opening)
```

- **No terminal state** (D-14): a fault re-enters the establish path after a
  resident-policy backoff (D-16). "Unsupported" is not a fault — it is a
  downgrade edge to the coarse establish path (D-17).
- **Re-arm before processing** where possible, to minimise the inherent
  loss window between completions (which is real and surfaced honestly, not
  hidden).
- **Teardown** sets the equivalent of `Azure/m`'s `m_shutting_down` flag — but we
  get it from the `windows-threadpool-sys` primitives, whose `Drop`/`CleanupGroup`
  already suppress re-arm and drain before freeing, rather than hand-rolling it.

## Threadpool mapping (D-3)

| Concern | Primitive |
|---|---|
| Detailed read completion | `ThreadpoolIo` (balanced `StartThreadpoolIo`/cancel accounting is built in) |
| Coarse change wait | `ThreadpoolWait` over an `assume_waitable` FFCN handle, rearmed per activation |
| Retry backoff timer | `ThreadpoolTimer` |
| Draining the request queue | `ThreadpoolWork` (serialized servicing of resident state) |
| Bulk teardown | `CleanupGroup` / owned-object `Drop` rundown |

## Hazards from Azure/m (recorded so they are not re-introduced)

1. **It throws on unclassified errors** out of the completion path
   (`m::throw_win32_error_code`). We classify **every** error (D-15); nothing
   escapes as terminal.
2. **It silently drops overflow** ("the lost changes are simply not reported").
   We surface `Desync { Overflow }` (D-12) — the whole point of "fidelity with
   notification limitations".
3. **It collapses rename actions** (old→"deleted", new→"changed"). We keep them
   distinct and raw (D-9).
4. **Its teardown-race flag is the good part** — adopt the *intent* via the
   workspace's already-hardened threadpool teardown rather than re-implementing.
5. **Its per-directory coalescing** (one read, filter per watch) is correct and
   adopted (D-6).
6. **It has no path-based (single-file) watch** — we add it (D-7).

## Open / future

- Ex records, digest-based verification, per-volume capability cache (D-19).
- The neither-API-supported volume: stays in establishing/retry per D-14.
