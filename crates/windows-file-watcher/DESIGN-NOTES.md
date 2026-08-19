# Design notes: windows-file-watcher (Tier 1)

Current, canonical decisions for the crate. This is the authoritative record; the
"why" and the alternatives considered live in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
(Tier 2), and the raw design discussion in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md)
(Tier 3). On any conflict, this file wins.

## Intent

A memory-safe watcher for changes to Windows paths, with full Windows fidelity
for path names and — just as important — for the platform's notification
*limitations*. It is deliberately Windows-only; platform independence is built at
a higher layer. It builds on [windows-overlapped-io-sys](../windows-overlapped-io-sys/README.md)
and [windows-threadpool-sys](../windows-threadpool-sys/README.md) and owns no
threads of its own.

## Decision index

| ID | Decision |
|---|---|
| D-1 | Windows-only watcher over `ReadDirectoryChangesW`, with a `FindFirstChangeNotification` coarse fallback. No cross-platform surface. |
| D-2 | Queue-mediated architecture: a `Monitor` hands out `Session`s (each a request-submission handle **plus** a notification sink). No client code ever runs on a monitor/threadpool thread. See [Queue mediation](#queue-mediation). |
| D-3 | No owned threads; all work runs on `windows-threadpool-sys` (`ThreadpoolIo`/`Wait`/`Timer`/`Work`, `CleanupGroup`). |
| D-4 | Detailed reads are issued through `windows-overlapped-io-sys`; its generation-stamped `OperationId` prevents a stale completion being misattributed across a re-arm. |
| D-5 | Subscribing returns an affine, move-only `#[must_use]` `Watch`: `Drop` enqueues cancellation, `cancel()` is the explicit form, and a `Copy` `WatchId` tags every notification. |
| D-6 | Coalesce watchers **by directory**: one read per directory, the union of `FILE_NOTIFY_CHANGE_*` filters and the max subtree flag, de-multiplexed to subscriptions on decode. See [Coalescing](#coalescing-by-directory). |
| D-7 | A subscription targets a *path*. A file is watched via its parent directory (non-recursive) filtered on the leaf; a directory is watched directly, optionally recursively. |
| D-8 | Names are delivered raw and **relative to the directory opened for the read**: for a directory target that directory itself, and for a file target (D-7) its parent — so a file watch reports the leaf name, not a name relative to the file. `OsString`/`Path` (lossless WTF-8) is primary; a raw `&[u16]` escape hatch is available. |
| D-9 | Raw `FILE_ACTION_*` kinds, `RenamedOldName`/`RenamedNewName` kept distinct; the crate never joins renames or joins across a buffer. |
| D-10 | Notifications are delivered as batches (one decoded `ReadDirectoryChangesW` completion = one batch). |
| D-11 | `NotificationSink: Send + Sync`, non-blocking + infallible `deliver`. The crate forces **multi-producer** safety (MPSC minimum); consumer cardinality is the client's business. |
| D-12 | `Desync { cause }` is the single "you missed changes — re-scan" primitive. Kernel overflow, a full client queue, coarse-mode signals, and post-outage gaps all collapse to it. See [The Desync primitive](#the-desync-primitive). |
| D-13 | `Suspended`/`Resumed` liveness brackets and `Established { mode }` are opt-in per subscription. |
| D-14 | No terminal fault state — only "not yet re-established." The monitor retries autonomously and indefinitely; the client may cancel from any state. See [Fault model](#fault-model). |
| D-15 | Recovery cannot self-fail: every error classifies into reopen-retry, rearm-retry, or downgrade-to-coarse. The only failure edges are retryable Windows syscalls. |
| D-16 | Retry policy is **resident data**, never a reactive callback: a backoff value mutated only through serialized request-queue items and read by the single serialized fault handler. Race-free; no client code on the cadence path. |
| D-17 | Two-tier watcher: Detailed (`ReadDirectoryChangesW` + `ThreadpoolIo`) preferred, Coarse (`FindFirstChangeNotification` + `ThreadpoolWait`) fallback. Mode is a volume property resolved at establish/re-establish. See [Two-tier watching](#two-tier-watching). |
| D-18 | v1 delivers basic `FILE_NOTIFY_INFORMATION`. |
| D-19 | Deferred seams, **reserved with no scheduled v1 work** (not gated on any blocker): `ReadDirectoryChangesExW` extended records; digest-based change *verification*; an optional per-volume capability cache. Revisit post-v1; no CHECKLIST item is queued for them, and M7.5 only reviews the public surface against this reservation. |
| D-20 | `Monitor::Drop` blocks on full rundown (cancel + drain every read/wait, then free), inheriting the `windows-threadpool-sys` teardown discipline. |

## Detail

### Queue mediation

Every interaction with a client is a queued request (client → monitor) or a
queued notification (monitor → client). The monitor's servicing is driven by a
`ThreadpoolWork` that serializes all resident-state mutations, so there is a
single logical authority and no client code executes on a monitor/threadpool
thread. A `Session` binds a request-submission handle to a notification sink;
every `Watch` created through a session delivers to that session's sink. (D-2)

### Coalescing by directory

Because there is one outstanding `ReadDirectoryChangesW` per directory handle,
all subscriptions whose targets live in a directory share **one** watcher and one
read. The read uses the union of the subscriptions' `FILE_NOTIFY_CHANGE_*`
filters and the maximum subtree flag; decoded records are routed to the subset of
subscriptions each matches. A single-file watch is just a non-recursive watch of
the parent directory filtered on the leaf name. (D-6, D-7)

### The Desync primitive

`ReadDirectoryChangesW` loses changes on buffer overflow (a zero-byte
completion / `ERROR_NOTIFY_ENUM_DIR`); a bounded client queue can fill; the coarse
fallback reports no detail at all; and any fault outage leaves a gap. All four are
the same fact to a client — "there is a hole in your event set" — so all four are
delivered as one cause-tagged `Desync { Overflow | QueueFull | Coarse |
Reestablished }`. Honest reporting of this limitation is a core requirement, not
an afterthought. (D-12)

### Fault model

On any I/O error the monitor enters a re-establish loop that never terminates of
its own accord: there is no failure state, only "not yet re-established." Every
error classifies into *reopen-and-retry*, *rearm-and-retry*, or
*downgrade-to-coarse*; nothing throws or gives up. Retry timing comes from
resident policy **data** — never a reactive per-fault callback and never a closure
on the cadence path — so a slow or absent client can neither stall recovery nor
create a race. The client can cancel from any intermediate state. (D-14, D-15,
D-16)

### Two-tier watching

Detailed watching (`ReadDirectoryChangesW` on a `ThreadpoolIo`) is preferred, but
not every filesystem supports it. The universal floor is the coarse
`FindFirstChangeNotification` family, watched with a `ThreadpoolWait`; each coarse
activation carries no detail and so becomes `Desync { Coarse }`. Which tier a
directory uses is a property of its **volume**, resolved during establish and
re-establish by attempting the detailed arm: an unsupported-class error
(`ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED`) downgrades to coarse; a
retryable error uses the reopen loop instead. The coarse handle is closed with
`FindCloseChangeNotification` (not `CloseHandle`) and reaches `ThreadpoolWait`
through `WaitableHandle::assume_waitable`. (D-17)
