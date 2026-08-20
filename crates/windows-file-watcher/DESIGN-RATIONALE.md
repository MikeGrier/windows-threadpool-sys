# Design rationale: windows-file-watcher (Tier 2)

*Why* the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached -- the
alternatives weighed, the prior art, and the reasoning. Keyed by decision ID.
The raw discussion is in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md).
This file is consulted for "why" questions; it is not authoritative for current
decisions (Tier 1 is).

## Prior art: the `Azure/m` filesystem monitor

The reference is the C++ `Azure/m` monitor (its author drove this design). Its
per-watch state machine -- open directory -> arm the change read -> wait -> decode ->
re-issue, with a "directory probe" retry timer -- is the shape we adopt. Several of
its behaviours are deliberately **not** reproduced:

- **It throws on unclassified errors** out of the completion path
  (`m::throw_win32_error_code`). We classify *every* error and never terminate the
  sequence (D-15).
- **It silently drops overflow** ("the lost changes are simply not reported"). We
  surface `Desync` (D-12) -- reporting the limitation is the point.
- **It collapses rename actions** (old -> "deleted", new -> "changed"). We keep the
  raw actions distinct (D-9).
- **It has no single-file (path) watch.** We add it via parent-directory
  filtering (D-7).
- Its **per-directory coalescing** and its **teardown re-arm-suppression flag**
  (`m_shutting_down`) are the good parts; we adopt the coalescing (D-6) and get
  the teardown discipline for free from `windows-threadpool-sys`.

## Queue mediation, and why no callbacks on the cadence path (D-2, D-11, D-16)

The recurring pain the author has hit is client code running on the I/O
provider's threads. So the design routes *all* client interaction through queues
in both directions and lets no client code execute on a monitor/threadpool
thread.

This forced the retry-control shape. An early proposal was a reactive per-fault
hook the client answers. Two constraints killed it:

1. **"The hook is in the queue."** A synchronous callback would run on the pool
   thread -- the exact thing we're avoiding.
2. **"There must be no race."** A reactive answer arriving on the request queue
   while the monitor's retry timer is already scheduled is, by construction, a
   race.

The only shape satisfying both is **resident policy data**: the client sets/updates
a backoff value through ordinary serialized request-queue items, and the single
serialized fault handler reads it as inert data. Nothing decides concurrently
(no race), and no client code runs on the cadence (no stall, no panic). A policy
update's effect relative to a fault is fixed by their order in the queue, not by
timing. (D-16)

### Per-subscription policy under directory coalescing (D-6, D-16)

Per-subscription retry overrides collide with one-watcher-per-directory (D-6):
several subscriptions can share a directory's single cadence yet ask for
different backoff. Leaving the winner unspecified would make recovery depend on
subscription order or add/remove timing. We instead define one deterministic
rule: the watcher recovers as fast as its *most eager* member wants, taking the
minimum of each policy field (initial delay, multiplier, cap, jitter, per-error
override) across the directory's subscriptions. This is a reduction over a set,
so it is order- and timing-independent (re-derived on membership change), and it
cannot starve one subscription behind another's slower policy. The alternative --
moving overrides to a directory granularity -- was rejected because the client's
unit of control is the subscription, not a directory it never named.

### The sink is a concrete sender, not a client trait (D-11)

Delivery could have been a client-implemented `NotificationSink` trait whose
`deliver` the monitor calls. It was rejected: a trait method invoked from an I/O
completion *is* client code running on a pool thread -- the precise thing D-2
exists to forbid -- and a `Send + Sync` bound cannot enforce the promised
non-blocking, infallible, panic-free behavior. A client `deliver` that blocks,
panics, or is slow would stall or unwind the cadence path. So the sink is instead
a **crate-owned concrete queue sender**: `deliver` is a crate-internal enqueue,
and the client only ever *receives*. The guarantee then holds structurally rather
than by trusting a callback.

### MPSC vs MPMC for the sink (D-11)

Delivery is serialized *per subscription* (one outstanding read per handle,
re-armed only after decode), so a single subscription is a single producer. But a
session's sink aggregates several subscriptions, whose completions run on
different pool threads concurrently -- so the sender must be **multi-producer**
(`Send + Sync`, concurrent enqueue): MPSC is the floor the crate imposes. It
never requires multi-*consumer*; draining from several threads is the client's
choice (MPMC only if they want it). Enqueue must be non-blocking and infallible
so it cannot stall the cadence, which is why a full bounded queue drops the batch
and latches a `Desync { QueueFull }` (see below) rather than blocking.

### Keeping QueueFull observable when the queue is full (D-11, D-12)

The obvious way to report a dropped batch -- enqueue a `Desync { QueueFull }` -- fails
exactly when it is needed, because the queue is full. Reserving a data slot for it
only defers the problem: a second overflow has nowhere to go. We instead keep the
overflow signal as latched control state *outside* the bounded queue: a set of
`WatchId`s with a pending `QueueFull`, coalesced (idempotent) and guaranteed to
reach the receiver before the next batch. This makes "never silently miss changes"
hold for any queue depth >= 1, including a client that has stopped draining. A
zero-capacity bound would make the guarantee vacuous, so it is rejected.

## The Desync unification (D-12)

Four distinct mechanisms -- kernel-buffer overflow, a full client queue, the
detail-free coarse fallback, and the gap across a fault outage -- are
indistinguishable to a client: each means "you may have missed changes." Rather
than invent four signals, we collapse them into one cause-tagged `Desync`. The
cause tag is advisory (it lets a client log/diagnose); the action is always the
same: re-scan. `Suspended`/`Resumed` and `Established { mode }` (D-13) are a
*different* axis -- liveness/observability, not "you missed changes" -- and are
therefore opt-in, so a minimal client honours exactly one signal.

## No terminal fault state (D-14)

Clients, in general, are not prepared to handle a failure that stops the
notification flow. So the monitor treats an I/O fault as "not yet re-established,"
not "failed," and retries autonomously and indefinitely. A target on a filesystem
supporting neither the detailed nor the coarse API simply stays in the
establishing/retry state until the client cancels -- there is no special terminal
case to reason about. This keeps the client's model trivial: a watch is either
delivering, or the monitor is working to make it deliver again, or the client
cancelled it.

## Two-tier watching, and *when* the mode is decided (D-17)

`ReadDirectoryChangesW` is not honoured on every filesystem/redirector; the
older `FindFirstChangeNotification` family is the broad floor but carries no
per-change detail. Detailed-vs-coarse is therefore a **volume** property, and the
natural place to resolve it is the establish/re-establish transition: attempt to
arm the detailed read on the freshly opened handle, and treat an
unsupported-class error as the downgrade edge (versus a retryable error, which
uses the reopen loop). Re-resolving on each re-establish is cheap and correct -- a
mount point's volume can change -- with a per-volume capability cache left as a
future optimization (D-19). Digest-based change *verification* on top of coarse
mode is likewise left open: trivial for a single file, genuinely complex for
recursive directories, so a good seam for a future contributor rather than v1
scope.

## Affine handle (D-5)

Rust is affine by nature -- a value can be dropped, and true linearity cannot be
enforced -- so an RAII `Watch` whose `Drop` enqueues cancellation is the idiomatic,
"easily managed" fit. A `Copy` `WatchId` correlation token lets a client route or
aggregate notifications without holding the lifecycle object.
