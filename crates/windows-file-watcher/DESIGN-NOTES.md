# Design notes: windows-file-watcher (Tier 1)

Current, canonical decisions for the crate. This is the authoritative record; the
"why" and the alternatives considered live in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md)
(Tier 2), and the raw design discussion in
[design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md](design-sessions/DESIGN-SESSION-2026-08-18-windows-file-watcher.md)
(Tier 3). On any conflict, this file wins.

## Intent

A memory-safe watcher for changes to Windows paths, with full Windows fidelity
for path names and -- just as important -- for the platform's notification
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
| D-8 | Names are delivered raw and **relative to the directory opened for the read**: for a directory target that directory itself, and for a file target (D-7) its parent -- so a file watch reports the leaf name, not a name relative to the file. `OsString`/`Path` (lossless WTF-8) is primary; a raw `&[u16]` escape hatch is available. |
| D-9 | Raw `FILE_ACTION_*` kinds, `RenamedOldName`/`RenamedNewName` kept distinct; the crate never joins renames or joins across a buffer. |
| D-10 | Notifications are delivered as batches (one decoded `ReadDirectoryChangesW` completion = one batch). |
| D-11 | Delivery is a **crate-owned concrete queue sender** (`Send + Sync`, multi-producer), never a client-implemented trait: its `deliver` only enqueues onto a queue the crate owns, so no client code ever runs on a monitor/threadpool thread (D-2). The client holds the matching receiver and drains on its own thread(s); consumer cardinality is the client's business (MPSC is the floor). A full bounded queue drops the batch but keeps saturation observable via a latched, out-of-band `Desync { QueueFull }` per affected `WatchId` (bound >= 1; a zero bound is rejected). See [Delivery and saturation](#delivery-and-saturation). |
| D-12 | `Desync { cause }` is the single "you missed changes -- re-scan" primitive. Kernel overflow, a full client queue, coarse-mode signals, and post-outage gaps all collapse to it. See [The Desync primitive](#the-desync-primitive). |
| D-13 | `Suspended`/`Resumed` liveness brackets and `Established { mode }` are opt-in per subscription. |
| D-14 | No terminal fault state -- only "not yet re-established." The monitor retries autonomously and indefinitely; the client may cancel from any state. See [Fault model](#fault-model). |
| D-15 | Recovery cannot self-fail: every error classifies into reopen-retry, rearm-retry, or downgrade-to-coarse. The only failure edges are retryable Windows syscalls. |
| D-16 | Retry policy is **resident data**, never a reactive callback: a backoff value mutated only through serialized request-queue items and read by the single serialized fault handler. Race-free; no client code on the cadence path. Because a directory has one coalesced watcher (D-6) but several subscriptions may set different policies, the watcher's *effective* policy is a deterministic **soonest-recovering** reduction across its subscriptions (see [Fault model](#fault-model)). |
| D-17 | Two-tier watcher: Detailed (`ReadDirectoryChangesW` + `ThreadpoolIo`) preferred, Coarse (`FindFirstChangeNotification` + `ThreadpoolWait`) fallback. Mode is a volume property resolved at establish/re-establish. See [Two-tier watching](#two-tier-watching). |
| D-18 | v1 delivers basic `FILE_NOTIFY_INFORMATION`. |
| D-19 | **Deferred to [CHECKLIST.md](CHECKLIST.md) -> M-inf (post-v1 horizon); not part of v1 scope.** This decision scopes these seams out of v1, so they are parked as horizon (M-inf) checklist items -- deferred by recorded scope decision, not for lack of a consumer -- and each is pulled into a numbered milestone post-v1 when a line of work takes it up: `ReadDirectoryChangesExW` extended records; digest-based change *verification*; an optional per-volume capability cache. |
| D-20 | `Monitor::Drop` blocks on full rundown (cancel + drain every read/wait, then free), inheriting the `windows-threadpool-sys` teardown discipline. |
| D-21 | **The decoder accepts only an exactly-described buffer.** A final record (`NextEntryOffset == 0`) must end the buffer either exactly at its name, or exactly at that name's DWORD-aligned end. A name is a whole number of UTF-16 units, so a record always ends on an even offset and its alignment padding is exactly 0 or 2 bytes -- never 1 or 3. Any other trailing remainder is undescribed data (a truncated or corrupt completion, or records whose link was zeroed), and is reported as a desync rather than silently discarded, since dropping it would understate the batch and lose changes. See [The Desync primitive](#the-desync-primitive) and [the exactly-described-buffer rationale (D-21)](DESIGN-RATIONALE.md#the-decoder-accepts-only-an-exactly-described-buffer-d-21). |
| D-22 | **Open failures split into retryable and permanent, and "permanent" means bad caller input, not a fault.** Opening a watched directory classifies into `NotFound`, `Unsupported`, `Retryable` (all retryable) and `NotADirectory`, `InvalidPath` (neither). This does not contradict D-14's "no terminal fault state": the permanent pair is not an environmental fault but the caller naming something that can never be a watched directory, so retrying would spin forever against input that will never become valid. An *unrecognised* error classifies as `Retryable`, so a watcher never silently stops watching because of an error code the crate has not seen. Two checks are ours rather than Win32's: a path with an interior NUL is rejected before the call, because Win32 would stop at the NUL and open a shorter path than the caller named; and a handle is verified to be a directory, because `FILE_LIST_DIRECTORY` and `FILE_READ_DATA` are the same bit, so a plain file opens successfully and would otherwise fail much later as a mis-classified read fault. |

## Detail

### Queue mediation

Every interaction with a client is a queued request (client -> monitor) or a
queued notification (monitor -> client). The monitor's servicing is driven by a
`ThreadpoolWork` that serializes all resident-state mutations, so there is a
single logical authority and no client code executes on a monitor/threadpool
thread. A `Session` binds a request-submission handle to a notification sink;
every `Watch` created through a session delivers to that session's sink. The sink
is a **crate-owned concrete queue sender**, not a client trait object: delivery
is an enqueue the crate performs, and the client observes notifications only by
draining the matching receiver on its own thread -- so the "no client code on a
pool thread" guarantee holds by construction, not by asking the client to keep a
callback well-behaved. (D-2, D-11)

### Delivery and saturation

Delivery is a crate-owned bounded queue: the monitor enqueues decoded batches and
the client drains the matching receiver. Enqueue never blocks -- a slow client must
not stall the cadence (D-2) -- so a full queue drops the batch. The core contract is
that a client is *never silently* left out of sync, which a naive design breaks
here: the `Desync { QueueFull }` that reports the drop cannot be pushed onto the
very queue that is full (and reserving one data slot only defers the problem -- a
second overflow has nowhere to go).

The signal is therefore kept **out of band**. The sender holds a latched overflow
set -- the `WatchId`s with a pending `QueueFull` -- as control state separate from the
bounded data queue, so it never competes for data capacity. A failed enqueue drops
the batch and adds each affected `WatchId` to that set; repeats coalesce, which
loses nothing because `Desync` is idempotent (the response is always a re-scan).
The receiver is guaranteed to observe a synthesized `Desync { QueueFull }` for each
latched `WatchId`, surfaced ahead of the next successful batch and cleared once
observed -- so the dropped batch and its desync can never both vanish, at any queue
depth >= 1. A zero-capacity bound is rejected at construction. (D-11, D-12)

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
the same fact to a client -- "there is a hole in your event set" -- so all four are
delivered as one cause-tagged `Desync { Overflow | QueueFull | Coarse |
Reestablished }`. Honest reporting of this limitation is a core requirement, not
an afterthought. (D-12)

### Fault model

On any I/O error the monitor enters a re-establish loop that never terminates of
its own accord: there is no failure state, only "not yet re-established." Every
error classifies into *reopen-and-retry*, *rearm-and-retry*, or
*downgrade-to-coarse*; nothing throws or gives up. Retry timing comes from
resident policy **data** -- never a reactive per-fault callback and never a closure
on the cadence path -- so a slow or absent client can neither stall recovery nor
create a race. The client can cancel from any intermediate state. (D-14, D-15,
D-16)

A directory has exactly one coalesced watcher (D-6), so it runs exactly one
reopen/re-arm cadence even when several subscriptions with different retry
policies share it. The watcher's effective policy is a per-field **soonest-
recovering** reduction over those subscriptions: the minimum initial delay,
minimum growth multiplier, minimum cap, minimum jitter, and the shortest
per-error-kind override; a directory with no overriding subscription uses the
monitor default. Because this is a reduction over the *set* of current
subscriptions, it is independent of subscription order and of add/remove timing
(it is simply re-derived whenever the membership changes), and it can never
starve one subscription's recovery behind another's slower policy. (D-6, D-16)

### Two-tier watching

Detailed watching (`ReadDirectoryChangesW` on a `ThreadpoolIo`) is preferred, but
not every filesystem supports it. The universal floor is the coarse
`FindFirstChangeNotification` family, watched with a `ThreadpoolWait`; each coarse
activation carries no detail and so becomes `Desync { Coarse }`. Which tier a
directory uses is a property of its **volume**, resolved during establish and
re-establish by attempting the detailed arm: an unsupported-class error
(`ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED`) downgrades to coarse; a
retryable error uses the reopen loop instead. The coarse handle is closed with
`FindCloseChangeNotification` (not `CloseHandle`); because `ThreadpoolWait`'s
default `OwnedHandle` path would close it with `CloseHandle`, it reaches the pool
through a **custom-close waitable owner** that `windows-threadpool-sys` must
provide (its M17, a prerequisite for M6.1, covering both the direct and
`CleanupGroup` teardown paths) and that drains the wait before invoking
`FindCloseChangeNotification`. (D-17)
