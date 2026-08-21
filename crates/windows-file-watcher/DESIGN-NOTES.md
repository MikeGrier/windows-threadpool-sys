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
| D-2 | Queue-mediated architecture: a `Monitor` hands out `Session`s (each a request-submission handle **plus** a notification sink). The crate never invokes client code, with no exceptions -- the D-25 doorbell was originally a carve-out and is no longer one. See [Queue mediation](#queue-mediation). |
| D-3 | No owned threads; all work runs on `windows-threadpool-sys` (`ThreadpoolIo`/`Wait`/`Timer`/`Work`, `CleanupGroup`). |
| D-4 | Detailed reads are issued through `windows-overlapped-io-sys`; its generation-stamped `OperationId` prevents a stale completion being misattributed across a re-arm. |
| D-5 | Subscribing returns an affine, move-only `#[must_use]` `Watch`: `Drop` enqueues cancellation, `cancel()` is the explicit form, and a `Copy` `WatchId` tags every notification. |
| D-6 | Coalesce watchers **by directory**: one read per directory, the union of `FILE_NOTIFY_CHANGE_*` filters and the max subtree flag, de-multiplexed to subscriptions on decode. See [Coalescing](#coalescing-by-directory). |
| D-7 | A subscription targets a *path*. A file is watched via its parent directory (non-recursive) filtered on the leaf; a directory is watched directly, optionally recursively. |
| D-8 | Names are delivered raw and **relative to the directory opened for the read**: for a directory target that directory itself, and for a file target (D-7) its parent -- so a file watch reports the leaf name, not a name relative to the file. `OsString`/`Path` (lossless WTF-8) is primary; a raw `&[u16]` escape hatch is available. |
| D-9 | Raw `FILE_ACTION_*` kinds, `RenamedOldName`/`RenamedNewName` kept distinct; the crate never joins renames or joins across a buffer. |
| D-10 | Notifications are delivered as batches (one decoded `ReadDirectoryChangesW` completion = one batch). |
| D-11 | Delivery is a **crate-owned concrete queue sender** (`Send + Sync`, multi-producer), never a client-implemented trait: its `deliver` only enqueues onto a queue the crate owns, so no client *delivery* code runs on the cadence path (D-2). The client holds the matching receiver and drains on whatever thread it likes -- including one of its own thread-pool callbacks, woken by the D-25 doorbell; consumer cardinality is the client's business (MPSC is the floor). **Its overflow policy is superseded by [D-29](#d-29):** a full queue no longer drops the batch, it stops the producer, so `QueueFull` becomes rare rather than routine. What survives is the bound itself (>= 1; a zero bound is rejected) and the per-`WatchId` latched `Desync`, which D-28 generalises to every desync. See [Delivery and saturation](#delivery-and-saturation). |
| D-12 | `Desync { cause }` is the single "you missed changes -- re-scan" primitive. Kernel overflow, a full client queue, coarse-mode signals, and post-outage gaps all collapse to it. See [The Desync primitive](#the-desync-primitive). |
| D-13 | `Suspended`/`Resumed` liveness brackets and `Established { mode }` are opt-in per subscription. |
| D-14 | No terminal fault state -- only "not yet re-established." The monitor retries autonomously and indefinitely; the client may cancel from any state. See [Fault model](#fault-model). |
| D-15 | Recovery cannot self-fail: every error classifies into reopen-retry, rearm-retry, or downgrade-to-coarse. The only failure edges are retryable Windows syscalls. |
| D-16 | **Superseded by [D-27](#d-27).** Retry policy was resident data only, with no per-fault exchange. Overturned 2026-08-21: the monitor now *asks* on fault, per subscription. The soonest-recovering reduction survives into D-27; the prohibition on asking does not. |
| D-17 | Two-tier watcher: Detailed (`ReadDirectoryChangesW` + `ThreadpoolIo`) preferred, Coarse (`FindFirstChangeNotification` + `ThreadpoolWait`) fallback. Mode is a volume property resolved at establish/re-establish. See [Two-tier watching](#two-tier-watching). |
| D-18 | v1 delivers basic `FILE_NOTIFY_INFORMATION`. |
| D-19 | **Deferred to [CHECKLIST.md](CHECKLIST.md) -> M-inf (post-v1 horizon); not part of v1 scope.** This decision scopes these seams out of v1, so they are parked as horizon (M-inf) checklist items -- deferred by recorded scope decision, not for lack of a consumer -- and each is pulled into a numbered milestone post-v1 when a line of work takes it up: `ReadDirectoryChangesExW` extended records; digest-based change *verification*; an optional per-volume capability cache. |
| D-20 | `Monitor::Drop` blocks on full rundown (cancel + drain every read/wait, then free), inheriting the `windows-threadpool-sys` teardown discipline. |
| D-21 | **The decoder accepts only an exactly-described buffer.** A final record (`NextEntryOffset == 0`) must end the buffer either exactly at its name, or exactly at that name's DWORD-aligned end. A name is a whole number of UTF-16 units, so a record always ends on an even offset and its alignment padding is exactly 0 or 2 bytes -- never 1 or 3. Any other trailing remainder is undescribed data (a truncated or corrupt completion, or records whose link was zeroed), and is reported as a desync rather than silently discarded, since dropping it would understate the batch and lose changes. See [The Desync primitive](#the-desync-primitive) and [the exactly-described-buffer rationale (D-21)](DESIGN-RATIONALE.md#the-decoder-accepts-only-an-exactly-described-buffer-d-21). |
| D-22 | **Open failures split into retryable and permanent, and "permanent" means bad caller input, not a fault.** Opening a watched directory classifies into `NotFound`, `Unsupported`, `Retryable` (all retryable) and `NotADirectory`, `InvalidPath` (neither). This does not contradict D-14's "no terminal fault state": the permanent pair is not an environmental fault but the caller naming something that can never be a watched directory, so retrying would spin forever against input that will never become valid. An *unrecognised* error classifies as `Retryable`, so a watcher never silently stops watching because of an error code the crate has not seen. Two checks are ours rather than Win32's: a path with an interior NUL is rejected before the call, because Win32 would stop at the NUL and open a shorter path than the caller named; and a handle is verified to be a directory, because `FILE_LIST_DIRECTORY` and `FILE_READ_DATA` are the same bit, so a plain file opens successfully and would otherwise fail much later as a mis-classified read fault. |
| D-23 | **Arming is gated by a lock held across the submission, not by a flag checked before it.** Teardown must be able to establish that no further read can be submitted. A boolean checked before submitting leaves a window: a completion callback passes the check, teardown then cancels every outstanding read and begins waiting for rundown, and only then does the callback submit -- leaving a fresh pending read that rundown waits on forever, since only a future directory change could complete it. This deadlocked the first implementation and is covered by a regression test that drops a watcher while changes are actively arriving. The gate is therefore a `Mutex<bool>` held for the whole submission, so teardown's own acquisition waits for any in-flight submission and, once it closes the gate, no new one can start. Note the `Weak`-upgrade suppression in the completion callback is *not* sufficient on its own: during `Drop` the strong count is still non-zero, so the upgrade still succeeds. |
| D-24 | **The completion buffer is `u32`-backed and heap-indirected.** `ReadDirectoryChangesW` requires a DWORD-aligned buffer, which a `Box<[u8]>` does not provide, so it is allocated as `Box<[u32]>` and viewed as bytes. The `Box` indirection is separately required: the buffer travels as the operation's payload and the payload moves when the operation is boxed for submission, so an inline array would invalidate the address handed to the kernel. The address and length are read *before* `submit` consumes the operation, which is sound precisely because the bytes live in the `Box`'s allocation rather than in the payload itself. A reported fill length beyond the allocation is clamped rather than trusted, so a corrupt completion length cannot become an out-of-bounds read. |
| D-25 | **Both queues have a doorbell, and both are crate-owned; neither is a client callback.** A queue drainable only by a blocking `recv()` forces a thread-pool-driven client to dedicate a thread, contradicting this crate's premise -- so each direction has a wake. The mechanisms differ because the *waiter* differs. On the **CQ** the client owns its waiting strategy, so `Receiver::doorbell()` hands out a manual-reset event handle, created lazily so a `recv()`-only client allocates no kernel object; on Windows a HANDLE is the universal waitable currency (`WaitForSingleObject`, `WaitForMultipleObjects`, `MsgWaitForMultipleObjects`, `ThreadpoolWait`, alertable waits), so this is the native composition point rather than a compromise. On the **SQ** *we* are the waiter, and `ThreadpoolWork::submit()` is already the ring -- queuing work beats an event plus a waiter for it. A `Doorbell` **trait was considered and rejected**: it would have been a client callback on our cadence path (reintroducing exactly what D-2 forbids), made `Monitor`/`Session`/`Sender` generic, and needed its own must-not-block/must-not-panic contract -- all to reach a case (an async `Waker`) the client bridges in ten lines *on its own pool*, which is where that code belongs. Owning the doorbell also makes the reset discipline an internal invariant (reset under the lock on observing empty) rather than a client obligation, so lost wakeups are impossible by construction and only harmless spurious ones remain. |
| D-26 | **An empty completion is not a notification.** `ReadDirectoryChangesW` can complete carrying no records, and forwarding that as an empty batch would break the inference a client most naturally draws from being woken -- "I was signalled, so something changed". An empty decode is therefore dropped rather than enqueued. This is not the same as the zero-byte completion, which *is* meaningful: that is the kernel's overflow signal and becomes `Desync { Overflow }` (D-12). Both a `Batch` and a `Desync` carry the `WatchId` of the subscription they belong to, and both ride the same queue, so their order relative to one another is defined within a subscription -- a client seeing a `Desync` knows exactly which changes preceded it. |
| <a id="d-27"></a>D-27 | **On fault the monitor asks each subscription how long to wait, and takes the earliest answer.** Supersedes D-16, which forbade asking. D-16's two objections were a synchronous callback on the pool thread (does not apply to a queued exchange) and a race against an already-scheduled retry timer -- and the race only exists if a timer is scheduled *before* the answer arrives. It is not: on fault the watcher latches and schedules nothing, so there is nothing to race. Each subscription chooses its mode at registration: **defaults** (the monitor retries autonomously, preserving D-14 for anyone who does not opt in) or **interactive** (a control message carries the `WatchId`, the failing operation, and the error code, and its response supplies the next delay). Because a directory is one coalesced watcher over several subscriptions (D-6), every subscription is asked and the **earliest** answer wins, clamped to a floor so no client can drive a hot loop; a subscription that declines is counted at its default rather than cancelled. Values follow `Azure/m`'s shipped code: **500 ms default, 50 ms floor.** Where `m`'s documentation contradicts its code -- claiming a declined answer cancels the watch, and a floor of "typically 1000ms, not less than 500ms" -- the code is authoritative and the prose lags. Open and arm failures carry separate defaults, matching D-15's reopen-retry / rearm-retry split. |
| D-28 | **A fault needs a standing reservation, and the latch is the saturation fallback for observation.** A fault report is control data generated on the cadence, so it can neither be dropped (the watch would silently never recover) nor block (deadlock). Under [D-33](#d-33) it is reserved rather than latched: an interactive subscription takes **one** standing notification-queue slot at registration, which is sufficient because a watcher cannot be faulted twice concurrently -- a faulted watcher is not running. It is therefore a queued item in the ordinary stream, in order with the data that preceded it. **This corrects an earlier over-generalisation.** This decision originally made *every* `Desync` a latch, which silently contradicted D-12 and D-26: an out-of-band latch destroys exactly the in-stream ordering those promise ("a client seeing a `Desync` knows exactly which changes preceded it"). Desyncs are observation-tier, so they ride the queue in order like any other notification; the per-`WatchId` coalescing latch is the **fallback used only when the observation tier cannot enqueue**, which is also the only way to report `QueueFull` at all, since saying "the queue is full" cannot itself require a slot. At that point ordering is already compromised by the loss the latch is reporting, so nothing is given up that survived anyway. |
| <a id="d-29"></a>D-29 | **A full queue is survived by throttling each producer away from the enqueue, never by blocking there.** Blocking at a full queue is a deadlock rather than backpressure: the writer would be an I/O completion holding a pool thread, and the client's drain may itself be a pool callback (the D-25 doorbell integration), so the cadence can block pool threads waiting for a drain that needs one. **Control** needs no throttle at all under [D-33](#d-33) -- its capacity was reserved at submit, so a completion always fits and request draining can never be blocked by a full ring. **Observation** is unreserved, so it is throttled at the arm: the watcher does not re-arm the read while the queue is full, which propagates backpressure into the kernel's own change buffer -- a grace period rather than a loss, and if the client drains in time nothing is lost at all. Because observation holds no reservation, a batch can still arrive to a full ring (a control reservation may have taken the room since the read was armed); that batch is dropped and the loss reported by D-28's latch, which is the one path where a notification is discarded. If instead the kernel buffer overflows first, that is the already-specified `Desync { Overflow }`. A consequence worth stating: the loss a client most often sees is genuine kernel overflow -- "the OS dropped changes", which is true -- rather than "the crate dropped changes because you were slow", which was a choice. |
| D-30 | **Every request produces a completion, carried on the notification queue, and its slot is reserved at submit ([D-33](#d-33)).** The request queue previously had no completions at all, which made M3.5's requirement -- assert no delivery after cancellation completes -- unprovable, since a client could not observe that its cancellation had been processed. Completions ride the notification queue rather than a side channel so the ordering guarantee is **structural**: a `Cancelled { watch }` sitting in the same ordered stream means everything before it belongs to the live watch and nothing after it does, which is the same reason `Desync` rides the stream (D-12). Uniformly *every* request, not only those whose contract seems to need it: subscribing can fail **permanently** (D-22's `NotADirectory` and `InvalidPath` have no retry path), so a fire-and-forget subscribe would hand back a `Watch` that will never fire and never say so. |
| D-31 | **A stalled watcher is a reportable state, not a silent one.** Both D-28 and D-29 park a watcher in the same not-re-arming state -- faulted, or waiting for a client that is behind. That state is bounded and self-healing (drain, or answer, and it resumes), but it is indistinguishable from "nothing is changing" unless it is reported. It must therefore be observable, and diagnostics for it must exist; the transport for those diagnostics is deliberately left open here, since a library emitting output is a dependency decision (see [CHECKLIST.md](CHECKLIST.md) -> M5.6). It must not be a client-supplied sink, which would be a callback on our path and is what D-2 forbids. |
| D-32 | **Relative names are stored as `Wtf16String`, not `OsString` or a bare `Box<[u16]>`.** The kernel reports names as `u16` and wide (`*W`) Win32 APIs consume `u16`, so any WTF-8 intermediate is pure overhead in both directions -- which is exactly the case `wtf-string` exists for. `RelativeName` is a newtype over `Wtf16String` rather than an alias, keeping the D-8 contract (relative to the *opened* directory) attached to the type, and derefs to `Wtf16Str` so the counted FFI, length, and lossy-display surface come for free; `as_wtf16` reaches the owned form for the terminated `LPCWSTR` pointer, which only the owned string carries. The public lossless surface is unchanged: `as_wide`, `to_os_string`, and `to_path_buf` all still hold, now converting at the boundary rather than storing there. One behavioural improvement falls out -- `Debug` was rendering through `to_string_lossy`, so two names differing only in *which* unpaired surrogate they carried printed identically as U+FFFD; WTF-16's own `Debug` escapes them as `\u{d800}` and keeps them distinguishable. |
| <a id="d-33"></a>D-33 | **Reliability is a property of reserved capacity, not of message type.** A **control** message reserves its notification-queue slot *before* whatever produces it is allowed to proceed: a request reserves its completion slot at submit, and an interactive subscription reserves a standing fault slot at registration (D-28). Delivery then cannot fail -- the slot is already the sender's -- so reliability is structural rather than a check somebody must remember to perform, and backpressure lands on the client's **own thread at submit time** rather than being discovered later somewhere it cannot be handled. **Change notifications reserve nothing** and are best-effort by design: a lost batch is re-derivable by re-scanning, which is what `Desync` exists to say, whereas a lost completion is a liveness bug because the client waits forever for something that already happened. This is what makes a single ring carrying two reliability classes principled rather than ad hoc -- the rule is one line, *reserved is guaranteed, unreserved is best-effort*, rather than a per-message-type table a reader has to memorise. It is the same discipline `io_uring` follows when it sizes its completion ring against its submission ring: an in-flight submission must always have somewhere to land. Consequently the monitor never needs to test whether a completion will fit, and a full ring can never wedge request processing. |
| D-34 | **The arm gate names why it is closed, and teardown is one idempotent operation with two triggers.** "Not re-arming" is a state three decisions reach for different reasons -- teardown (permanent), a latched fault (D-28), and queue backpressure (D-29, transient) -- so the gate is an `ArmGate` enum rather than the boolean D-23 introduced. A boolean would force the later two to bolt on a parallel flag and would leave `stop_reason` guessing which condition stopped the watcher; naming it now means each adds a variant instead. Teardown itself lives in one idempotent `stop()` that closes the gate, cancels, and drains, with `Drop` as only the implicit trigger -- so an explicit stop followed by the implicit one is free, and there is one implementation to reason about rather than two orderings to keep in step. Releasing the queue sender is part of teardown, not an afterthought: `inner` drops after `stop()` returns, so the client's receiver observes a disconnect and its drain loop terminates rather than blocking forever on a queue nothing can fill again. |

## Detail

### Queue mediation

Every interaction with a client is a queued request (client -> monitor) or a
queued notification (monitor -> client). The monitor's servicing is driven by a
`ThreadpoolWork` that serializes all resident-state mutations, so there is a
single logical authority, and **the crate never transfers control into client
code** on that path -- there is no sink trait, no callback registration, and no
client-supplied closure, with the single bounded exception of the D-25 doorbell.
A `Session` binds a request-submission handle to a notification sink; every
`Watch` created through a session delivers to that session's sink. The sink is a
**crate-owned concrete queue sender**, not a client trait object: delivery is an
enqueue the crate performs, and the client observes notifications only by
draining the matching receiver -- so the guarantee holds by construction, not by
asking the client to keep a callback well-behaved. (D-2, D-11)

**This is a statement about the call graph, not about threads.** An earlier
phrasing claimed "no client code ever runs on a monitor/threadpool thread",
which is false and was never something this crate could promise: the process
thread pool is not ours, and a client that arms its own `ThreadpoolWait` on the
doorbell and drains the queue from that callback is running client code on a
pool thread -- legitimately, and by design. That is the client's pool object and
the client's cadence; stalling it stalls only the client. What the crate
guarantees is narrower and actually enforceable: nothing the client does --
blocking, panicking, being slow, re-entering -- can stall or unwind *our* cadence
path, because we never call into it.

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
