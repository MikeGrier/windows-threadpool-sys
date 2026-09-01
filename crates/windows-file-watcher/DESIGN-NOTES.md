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
| D-30 | **Every lifecycle request produces a completion, carried on the notification queue, and its slot is reserved at submit ([D-33](#d-33)).** The request queue previously had no completions at all, which made M3.5's requirement -- assert no delivery after cancellation completes -- unprovable, since a client could not observe that its cancellation had been processed. Completions ride the notification queue rather than a side channel so the ordering guarantee is **structural**: a `Cancelled { watch }` sitting in the same ordered stream means everything before it belongs to the live watch and nothing after it does, which is the same reason `Desync` rides the stream (D-12). Uniformly every **lifecycle** request -- one that creates or ends a subscription -- not only those whose contract seems to need it: subscribing can fail **permanently** (D-22's `NotADirectory` and `InvalidPath` have no retry path), so a fire-and-forget subscribe would hand back a `Watch` that will never fire and never say so. **`Answer` and `AnswerVolumeChange` are excluded and carry no completion**: they are *responses* to questions this crate posed, not requests with a lifecycle of their own, so there is nothing for a completion to report and nothing a client is left waiting on. An answer for a watch that is not currently being asked is silently discarded (already resolved, already cancelled, or never asked). |
| D-31 | **A stalled watcher is a reportable state, not a silent one.** Both D-28 and D-29 park a watcher in the same not-re-arming state -- faulted, or waiting for a client that is behind. That state is bounded and self-healing (drain, or answer, and it resumes), but it is indistinguishable from "nothing is changing" unless it is reported. It must therefore be observable, and diagnostics for it must exist; the transport for those diagnostics is deliberately left open here, since a library emitting output is a dependency decision (see [CHECKLIST.md](CHECKLIST.md) -> M5.6). It must not be a client-supplied sink, which would be a callback on our path and is what D-2 forbids. |
| D-32 | **Relative names are stored as `Wtf16String`, not `OsString` or a bare `Box<[u16]>`.** The kernel reports names as `u16` and wide (`*W`) Win32 APIs consume `u16`, so any WTF-8 intermediate is pure overhead in both directions -- which is exactly the case `wtf-string` exists for. `RelativeName` is a newtype over `Wtf16String` rather than an alias, keeping the D-8 contract (relative to the *opened* directory) attached to the type, and derefs to `Wtf16Str` so the counted FFI, length, and lossy-display surface come for free; `as_wtf16` reaches the owned form for the terminated `LPCWSTR` pointer, which only the owned string carries. The public lossless surface is unchanged: `as_wide`, `to_os_string`, and `to_path_buf` all still hold, now converting at the boundary rather than storing there. One behavioural improvement falls out -- `Debug` was rendering through `to_string_lossy`, so two names differing only in *which* unpaired surrogate they carried printed identically as U+FFFD; WTF-16's own `Debug` escapes them as `\u{d800}` and keeps them distinguishable. |
| <a id="d-33"></a>D-33 | **Reliability is a property of reserved capacity, not of message type.** A **control** message reserves its notification-queue slot *before* whatever produces it is allowed to proceed: a request reserves its completion slot at submit, and an interactive subscription reserves a standing fault slot at registration (D-28). Delivery then cannot fail -- the slot is already the sender's -- so reliability is structural rather than a check somebody must remember to perform, and backpressure lands on the client's **own thread at submit time** rather than being discovered later somewhere it cannot be handled. **Change notifications reserve nothing** and are best-effort by design: a lost batch is re-derivable by re-scanning, which is what `Desync` exists to say, whereas a lost completion is a liveness bug because the client waits forever for something that already happened. This is what makes a single ring carrying two reliability classes principled rather than ad hoc -- the rule is one line, *reserved is guaranteed, unreserved is best-effort*, rather than a per-message-type table a reader has to memorise. It is the same discipline `io_uring` follows when it sizes its completion ring against its submission ring: an in-flight submission must always have somewhere to land. Consequently the monitor never needs to test whether a completion will fit, and a full ring can never wedge request processing. |
| D-34 | **The arm gate names why it is closed, and teardown is one idempotent operation with two triggers.** "Not re-arming" is a state three decisions reach for different reasons -- teardown (permanent), a latched fault (D-28), and queue backpressure (D-29, transient) -- so the gate is an `ArmGate` enum rather than the boolean D-23 introduced. A boolean would force the later two to bolt on a parallel flag and would leave `stop_reason` guessing which condition stopped the watcher; naming it now means each adds a variant instead. Teardown itself lives in one idempotent `stop()` that closes the gate, cancels, and drains, with `Drop` as only the implicit trigger -- so an explicit stop followed by the implicit one is free, and there is one implementation to reason about rather than two orderings to keep in step. Releasing the queue sender is part of teardown, not an afterthought: `inner` drops after `stop()` returns, so the client's receiver observes a disconnect and its drain loop terminates rather than blocking forever on a queue nothing can fill again. |
| D-35 | **The M2 loop is tested through a feature-gated `#[doc(hidden)]` module, not by widening the public surface, and its overflow is forced rather than raced.** The loop is `pub(crate)` and an integration test cannot reach a `pub(crate)` item, but the two honest alternatives were both worse: publishing the interim queue would commit the crate to a shape D-11 already schedules for replacement in M3, and moving the test into the unit tree would put a thousand-file burst into a lib test binary the repository asks to stay under a second. So `unstable-internals` exposes the interim items as `crate::unstable`, hidden from docs, excluded from default features, and deleted with the module in M3.8. Separately, the overflow assertion does **not** race a burst against the re-arm: the kernel only discards when records pile up in the window between a completion and the next arm, and nothing outside the crate can widen that window -- the raced form was measured between 1.5 s and 15 s for the same assertion, which is a flake waiting for a loaded runner. Undersizing the completion buffer below one record forces the identical kernel path in bounded time, and asserting a *second* overflow proves the completion path re-armed after the first. |
| D-36 | **The SQ doorbell's edge is "a drain is outstanding", not "the queue is empty".** [D-25](#d-25) named `ThreadpoolWork::submit()` as the SQ ring but not the condition for ringing it, and the obvious condition is wrong. Each `submit` queues an *independent* invocation and they do not coalesce, so ringing per enqueue would queue 500 drains to service 500 subscriptions; ringing only when the queue was empty fixes that, and still does not **serialise**, which is the part D-2 actually needs -- a drain that has emptied the queue but is still running its last handler leaves the queue observably empty, so the next enqueue rings and a second drain executes alongside the first. The flag is therefore whether a drain is queued or running: set under the queue lock by whoever rings, cleared only by a drain that finds nothing left to do, and the drain loops until empty rather than servicing one item per ring, which is what makes the two readings coincide in the steady state. Clearing it is deliberately the drain's **last** touch of shared state, since a producer may ring the instant it is cleared. Measured: 500 submissions behind one running handler queue exactly **one** drain, and that is exact rather than a bound -- the flag is set at submission time, so it does not depend on how the pool schedules anything. |
| D-37 | **The monitor's `Request` is uninhabited until M3.5 defines what travels the queue.** M3.1 builds the servicing path; the requests are M3.5's, along with the affine `Watch` that issues them. An empty enum states that honestly -- the queue is typed, the handler is exhaustive, and adding a variant is a pure extension -- where a placeholder variant would be scaffolding that has to be found and removed later. It is not a deferral for lack of a consumer (which the repository forbids): the machinery is built in full, and it is proven on its own terms because [`Servicer<T>`](src/servicing.rs) is generic over the request type. That genericity is the point rather than a convenience -- what the path guarantees (exactly-once, in-order, never-concurrent, bounded rundown) is a property of the machinery, so it is tested against a trivial request type instead of waiting for a real one. |
| D-38 | **A session is a handle on the monitor, not a co-owner of it, and the notification queue is the session's rather than the monitor's.** A session holds the servicing path alive so it can submit from any client thread, but that is an allocation, not a lifetime: `Monitor::Drop` shuts the path down and tears every watcher down whatever sessions still exist, after which a surviving session reports itself closed and hands back any request it is given. The alternative -- a strong co-ownership in which a forgotten session keeps a monitor and its watchers running -- would make D-20's blocking teardown conditional on the client having dropped everything in the right order, which is exactly the property D-20 exists to remove. The queue runs the other way: it belongs to the session and its receiver, so shutting the monitor down does **not** sever a stream the client is still draining, and notifications already enqueued remain readable. The two directions are asymmetric on purpose -- the crate owns when watching stops, the client owns when reading stops. |
| D-39 | **The latch is drained into the queue at the next enqueue, and a freed slot reports an owed loss before it carries new changes.** [D-11](#d-11) said a latched `QueueFull` is "surfaced ahead of the next successful batch", which read as *emit it immediately*; that is wrong, and would break the ordering [D-12](#d-12) promises. A client seeing a desync at position P concludes everything before P is accounted for, so emitting the report ahead of items already queued would claim the hole is older than it is. The queue was **full** when the loss happened, so everything still queued precedes it -- which fixes the position exactly: flush the latch into the queue at the next successful enqueue, where it lands after the changes that preceded the loss and before the one that followed. A receiver that drains to empty synthesises any remainder directly, so the report does not depend on future traffic ever arriving. When only one slot frees, it goes to the **report**, not to the new notification, which is then re-latched: carrying more changes across a hole the client has not been told about is the silent loss the whole design forbids. A report covers the losses since the previous report, so a subsequent loss latches again rather than being coalesced into a report already delivered. |
| D-40 | **The bound is a `NonZeroUsize`, not a checked `usize`.** A zero bound is not a runtime condition to report -- it is a queue that could not carry even the desync announcing its own saturation, making the crate's never-silently-lose guarantee vacuous. [D-11](#d-11) requires rejecting it "at construction"; making it unrepresentable does that in the type system, rejects a literal at compile time, and leaves no error path for a caller to handle or for this crate to test. The bound counts **notifications**, not changes: one decoded completion is one batch ([D-10](#d-10)) and a batch may carry hundreds of records, so the depth is far greater than the number suggests. The default is 1024. |
| D-41 | **The doorbell's contract is one predicate, re-established under the queue lock at the end of every mutation.** [D-25](#d-25) specified the mechanism (a lazily created manual-reset event) and the discipline ("the receiver resets under the queue lock on observing empty and the sender sets after enqueue") as two rules on two sides. Implemented that way they can drift, so it is instead a single invariant -- **the event is signalled exactly when the receiver has something to take** -- re-established by one function that every mutation path ends with, under the same lock a receiver holds while deciding there is nothing to take. There is therefore no window between those two decisions for a wakeup to fall into, and the promised "harmless spurious wakeups" do not arise either. "Something to take" is deliberately three things, not one: a queued notification, an **owed loss report** (a latched desync is not in the queue but is still something to collect, per [D-39](#d-39)), and the **end of the stream** -- a disconnected queue stays signalled permanently, because a client waiting on the handle must learn the stream has ended rather than wait for a notification nothing can send. |
| D-42 | **The doorbell is handed out both borrowed and owned, because Win32's waits disagree about ownership.** `Receiver::doorbell` returns a `BorrowedHandle` -- the event belongs to the queue and a caller must not close it -- which is what `WaitForSingleObject` / `WaitForMultipleObjects` / `MsgWaitForMultipleObjects` want. But the integration D-25 exists to enable, arming a `ThreadpoolWait`, *takes ownership* of its target, so a borrow-only API would have forced every such client to write `DuplicateHandle` boilerplate to reach the motivating use case. `Receiver::doorbell_owned` returns a duplicate of the same event, so signalling reaches both and the caller closes its copy whenever it likes. One event, created once and lazily; two handles onto it, because the platform's waits are split on this point. |
| D-43 | **A `WatchId` is minted per monitor, and registration options are `#[non_exhaustive]` because they are the one call that cannot be added to later.** Identifiers key the monitor's resident state as well as tagging notifications, so two sessions must never mint the same one; the sequence therefore lives in the monitor's shared core rather than in a session. That core deliberately holds only the servicing path and the counter -- **not** the resident state, which the handler captures directly, because a handler reaching back through the object that owns its own servicer is the cycle the watcher's completion callback already had to solve with a `Weak`. Registration options are a `#[non_exhaustive]` struct with setters rather than positional arguments, because [D-27](#d-27)'s retry mode and M4's change-type filter are both properties of a *subscription* that only the registration call can state; adding either to a positional signature would be a breaking change to the one call that has to carry them. The retry mode is stored with the subscription and readable from the monitor, so "registration carries it" is verified now rather than being a claim M5.3 discovers is false. |
| D-44 | **Registration is asynchronous, so its failure is not the caller's return value.** `Session::subscribe` fails only when the monitor has shut down -- everything else it could report, it does not yet know: whether a path can be watched is discovered when the servicing path opens it, on a pool thread, after the call has returned. Reporting the open failure synchronously would mean opening the directory on the client's thread, which puts an unbounded filesystem call (a dead network path can take tens of seconds) where the client did not ask for one, and would put watcher construction outside the single servicing authority [D-2](#d-2) establishes. So the call returns a `Watch` optimistically and the outcome arrives as a completion (M3.6/[D-30](#d-30)). Until that lands there is a real gap, stated rather than hidden: a subscription to an unwatchable path starts no watcher and says nothing, which is exactly the "holds a `Watch` that can never fire and never says so" case D-30 exists to close. |
| D-45 | **A subscription reserves *two* slots at registration, because `Drop` has nowhere to report a refused reservation.** [D-33](#d-33) says a request reserves its completion slot at submit, which works for subscribe -- a refusal becomes an error on the client's own thread at the call that asked for the work. Cancellation cannot do that: [D-5](#d-5) makes `Drop` a cancellation path, and a destructor has no way to report that the queue was too full to promise its completion. So the cancellation's slot is taken at **registration** and held for the whole life of the `Watch`, which is the same standing-reservation shape [D-28](#d-28) uses for an interactive subscription's fault report. The cost is stated plainly: a subscription occupies two slots of the bound for its lifetime, so a bound of two admits exactly one subscription. That is [D-29](#d-29)'s backpressure landing where it was designed to land -- on `subscribe`, on the client's thread -- rather than a limitation to work around. The taken-or-not state of that reservation also *is* the "already cancelled" flag, so an explicit `cancel` followed by `Drop` cannot enqueue twice. |
| D-46 | **A retryable open is `Establishing`, not a failure, and the completion is enqueued only after the watcher is fully stopped.** Splitting registration's outcome by [D-22](#d-22)'s retryable/permanent line is what keeps [D-14](#d-14)'s "no terminal fault state" true: a path that does not exist *yet* leaves the subscription registered with no watcher, to be established later, and reporting it as `Failed` would invite a client to give up on something the monitor intends to recover. Only the permanent pair registers nothing and reports `Failed`. This makes *registered* and *watching* genuinely different states, and the monitor exposes both. Separately, `Cancelled` is sent **after** the watcher has been torn down rather than before, which is what makes D-30's ordering claim structural rather than a race: nothing from that subscription can be enqueued past that point, so a client may treat the completion as a boundary instead of reasoning about timing. |
| D-47 | **Resuming a paused watcher needs a re-check under the gate lock, or the wake is lost.** [D-29](#d-29) says the watcher stops re-arming while the queue is full and resumes when the client drains, which needs a prod from the drain -- and the room check and the gate transition are under *different* locks, so the obvious implementation drops wakes. Concretely: the watcher finds no room, and before it can record why, a drain frees a slot and prods; the prod finds the gate still `Open` and does nothing; the watcher then parks. Room is available, nothing is coming to prod it again, and the watch is wedged with no error anywhere. The fix is to set [`ArmGate::Backpressured`](#d-34) and then **re-check for room while still holding the gate lock**: a drain that freed a slot before the re-check is seen by it, and one that freed after must take the gate lock to prod and therefore observes the parked state. The gate lock, not the queue lock, is the serialisation point. The prod itself is a crate-owned `Resume` -- not the client callback [D-25](#d-25) rejected, since every implementor is this crate's code -- rung on the queue's **full -> not-full edge** so a client does not pay a wake per notification drained, and run *outside* the queue lock, because a failed re-arm reports itself by sending, which would take that same lock. |
| D-48 | **The resume is queued onto this crate's pool, not performed on the thread that drained.** The prod arrives on whichever thread emptied the queue, which is usually the client's. Re-arming there would put this crate's critical section on a thread it does not control: arming holds the gate lock across a `ReadDirectoryChangesW` submission (D-23) and teardown waits on that same lock, so a client thread preempted mid-arm would stall teardown. The bounded-work argument does not rescue it either -- the work is ours and short, but the *scheduling* is not ours. So `Resume` does one non-blocking `SubmitThreadpoolWork` and the re-arm happens on a pool thread, which is the mirror image of [D-25](#d-25)'s doorbell: there the client rings and we service; here we ring and we service. The cost is one work object per watcher, which [D-6](#d-6)'s per-directory coalescing bounds by directory rather than by subscription. |
| D-49 | **The public surface is `Monitor` / `Session` / `Watch` / `Receiver` and the notification types -- nothing that could break a promise the crate makes.** Retiring [D-35](#d-35)'s scaffolding was the moment to decide what is actually public, and the rule that fell out is that an item is public only if a client can use it without being able to invalidate a guarantee. `Request` and `Session::submit` are therefore **crate-internal**: a publicly constructible request could not carry a completion slot reserved before submission, so [D-33](#d-33)'s "delivery cannot fail" would become "delivery cannot fail unless you built the request yourself". `Sender` is internal for the same reason as [D-11](#d-11) -- a client only ever receives -- and `DirectoryWatcher`, `Servicer`, `ArmGate` and the completion-buffer size are implementation. `OpenFailure` *is* public, because it appears in an `Outcome` a client must match on. Removing the three `#![allow(dead_code)]` suppressions then found six items the scaffolding had been hiding, all genuinely dead: four were deleted, one test-only constructor became `#[cfg(test)]`, and `stop_reason` was given the production caller [D-31](#d-31) had been asking for all along -- a stalled watcher must be observable, and it now is from the monitor. |
| D-50 | **Directory identity, for coalescing (D-6), is by file -- volume serial plus file index -- not by path string.** Two subscriptions can name the same directory through different spellings (a trailing separator, a different case, a symlink hop), and coalescing has to recognise that as one directory rather than compare strings. `GetFileInformationByHandle`'s `dwVolumeSerialNumber` plus `nFileIndexHigh`/`nFileIndexLow` is stable for as long as the file exists regardless of how it was reached, and the crate already called this API (`ensure_directory`'s `FILE_ATTRIBUTE_DIRECTORY` check) so computing the identity there costs nothing further. A consequence, not a defect: a subscription always incurs one open even when it turns out to coalesce with an existing watcher, because the identity can only be read from a live handle -- the same trade-off `m` and every other identity-based watcher makes. |
| D-51 | **Made permanent by D-77 -- the "future milestone" this decision anticipated will never arrive, so the constant is the final answer, not a placeholder.** **The filter union is a constant, not a computed union, because there is nothing to union.** M4.1's item asked to "union the `FILE_NOTIFY_CHANGE_*` filters... across a directory's subscriptions", but [`WatchOptions`](../windows-file-watcher/src/watch.rs) has no filter-selection field -- every subscription implicitly wants [`ALL_NOTIFY_FILTERS`](../windows-file-watcher/src/watcher.rs), so the union over any set of subscriptions is trivially that same constant. The *reach* union (subtree) is real and is computed fresh at every arm from the live route set; the filter union has nothing to compute, and D-77 settles that it never will: no subscription will ever be given a filter to select, so this stays a constant permanently rather than becoming a real reduction later. |
| <a id="d-52"></a>D-52 | **Widening a coalesced watcher's reach to recursive reopens the directory; it does not cancel and resubmit on the same handle.** The obvious implementation -- cancel the live read, then resubmit with `bWatchSubtree=TRUE` on the same handle -- was tried first, matching M4.4's original wording. It does not work: measured directly, the kernel kept reporting only the directory's direct children after the resubmit; nothing nested was ever reported, no matter how long a test waited or how many further changes it made. A fresh `CreateFileW` does not have this problem. So [`WatcherInner::io`](../windows-file-watcher/src/watcher.rs) moved from a set-once `OnceLock` to a replaceable `Mutex<Option<ThreadpoolIo>>`: reopening tears the old endpoint down completely (cancel, then `run_down` -- the same ordering [D-23](#d-23) already established), builds a fresh one bound to a newly opened handle, and arms it, all under a new transient [`ArmGate::Reopening`](#d-34) that resolves to `Open` or, if teardown wins the race, is simply abandoned. The handle a reopen needs is the one the caller already opened to discover the directory's identity for coalescing (D-50) -- so `DirectoryWatcher::add_route` takes it as a parameter and there is no second open. A failure to reopen stops the whole watcher (D-15's rearm-and-retry classification) and is reported to every route it currently serves, not only the one that triggered the widen, since they now share one endpoint. |
| D-53 | **The establish/re-establish state machine reuses `ArmGate` rather than introducing a separately named `Opening -> ArmingDetailed -> WatchingDetailed` machine.** `ArmGate` already answered exactly the question a fault needs answered -- "may this watcher submit a read right now, and if not, why" -- for backpressure (D-29/D-34) and widening (D-52); a fault is a third reason of the same shape, so it is a fourth variant, `ArmGate::Faulted`, rather than a parallel state type. A watcher's directory-level path (an outstanding read failing, or a widen/re-establish reopen failing) and a still-`Pending` subscription's own open-retry path (M5.1, monitor-level, before any `DirectoryWatcher` exists) are two different owners of the same protocol, not one shared object -- see D-59. |
| D-54 | **Superseded by [D-79](#d-79).** A fault's `FaultState` does not retain the triggering error, only which operation faulted. The raw `io::Error`/`OpenError` is surfaced once, at the moment the fault begins, through the `log` diagnostic (D-58), and is not needed again: every subsequent decision (who to ask, what the default is, whether an answer resolves it) depends only on [`FaultOperation`](../windows-file-watcher/src/retry.rs) and the accumulating earliest-answer reduction. Keeping the error alive past that point would be state with no reader. |
| D-55 | **The interactive fault question is delivered through a standing per-subscription reservation ([`StandingSlot`](../windows-file-watcher/src/queue.rs)), taken once at registration, not the resident coalescing latch D-28 first sketched.** D-28's "one error code plus one bit, allocated with the watcher" described resident *state*; what M5.3 actually needed to deliver reliably is a *message* -- the `WatchId`, the failing operation, D-27's negotiation -- which the ordinary reserve-then-send `Reservation` cannot do more than once. `StandingSlot` is the reusable generalisation: it carves out its capacity permanently rather than releasing it after one send, which is sound *only* because a watcher cannot fault twice concurrently (D-28's own justification) -- so at most one question per subscription is ever outstanding, and one slot is provably always enough regardless of how many faults a long-lived subscription lives through. |
| D-56 | **The retry protocol ships fixed per-operation defaults only -- no growth multiplier, cap, jitter, or per-error-kind override.** The "Fault model" prose below describes a hypothetical soonest-recovering *reduction* over several such knobs, written before D-27 replaced D-16; but `RetryMode` carries only `Defaults`/`Interactive`; `WatchOptions` has no field for any of those numbers. Implementing them now would be inventing behaviour nobody can configure. What ships is D-27's literal text: [`FaultOperation::default_delay`](../windows-file-watcher/src/retry.rs) (500 ms for both Open and Arm), asked of every interactive route, resolved to the earliest answer, clamped to the [`FLOOR`](../windows-file-watcher/src/retry.rs) (50 ms). Extending this is a scope decision for whoever gives `WatchOptions` those fields, not an omission here. |
| D-57 | **`Suspended`/`Resumed`/`Established` (D-13) ride the ordinary best-effort observation queue, like `Desync`; only `RetryQuestion` gets a standing reservation.** The distinction D-28/D-33 draws is about *guarantee*, not about which notifications are "fault-related": a lost liveness bracket is not a liveness bug (the client still eventually resumes seeing batches, or observes the disconnect), where a lost `RetryQuestion` on an interactive subscription would silently wedge that subscription's own recovery. So only the question needs D-55's permanent slot; the brackets share fate with every other observation-tier notification, including being subject to the same `Delivery::Latched` best-effort outcome under saturation. |
| D-58 | **The M5.6 diagnostic transport is the `log` facade, at exactly two call sites: entering a fault and resolving one.** Per the repository's architectural pre-step rule, the first output emission had to introduce an abstraction rather than a bare call; `log` is that abstraction here, chosen (over `eprintln!`, which is unfilterable and wrong for a library, and over a client-supplied sink, which D-2/D-31 forbid outright) because it costs nothing when no logger is installed and commits a client to nothing. `DirectoryWatcher::is_faulted` and `Monitor::is_faulted` are the corresponding state exposure D-31 asks for -- observable directly, independent of whether a logger happens to be listening. |
| D-59 | **A `Pending` subscription's open-retry (M5.1, monitor-owned, before any directory identity or coalesced watcher exists) and a coalesced watcher's arm-retry (watcher-owned) are two independent instances of the same protocol, not a shared object.** Each owns its own [`ThreadpoolTimer`](../windows-threadpool-sys/src/timer.rs) and applies D-27's ask/resolve/floor logic on its own terms -- a `Pending` subscription's reduction is trivial (it is never shared, since coalescing (D-6) only happens once a directory is actually opened), while a coalesced watcher's is a genuine reduction over however many routes it currently serves. `Monitor`'s retry timer callback resolves a `Weak<Core>` through a cell filled in immediately after `Core` is constructed (`Core` cannot exist before `Servicer::new` returns, since the servicer is one of its own fields, so the callback captured at `Servicer::new` time cannot yet hold a strong or weak reference to it). End-to-end recovery through both paths together is confirmed by a real-OS integration test (M5.7): deleting the watched directory while a read is outstanding does fail that read (an arm-class fault, confirming D-15's classification empirically rather than by assumption), and recreating it lets the coalesced watcher's own reopen loop re-establish, reporting `Resumed`/`Established`/`Desync { Reestablished }`. |
| D-60 | **One `WatcherInner`, one `Endpoint` field holding either tier, not a second parallel watcher type for coarse (M6).** `Endpoint::Detailed(ThreadpoolIo)` or `Endpoint::Coarse(ThreadpoolWait)` share every piece of machinery that has nothing to do with which API is doing the reading: routes, coalescing, the fault/retry protocol (D-27), backpressure (D-29), and the retry timer. Only `arm_locked` (dispatches the actual submission/arm call) and completion handling (`on_completion` for detailed, the new `on_activation` for coarse) differ per tier. This is what let M6 land without duplicating M4/M5's entire coalescing and fault-recovery machinery for a second, mostly-identical type. |
| D-61 | **Mode is re-resolved by `reopen` on every call, and `reopen` is now the *only* way any tier is ever established -- including the very first one.** The constructor previously hand-built a detailed `ThreadpoolIo` directly; it now calls `WatcherInner::reopen` like every later widen, re-establish, or downgrade does. `reopen` tries detailed first (unless `force_coarse`, D-63) and falls back to coarse only when arming it fails with `OpenFailure::Unsupported` (via `directory::classify`, reused verbatim from the open-failure classification, D-22) -- reusing the classification is what makes "detailed open succeeds, detailed arm fails as unsupported" and "the directory itself cannot be opened" collapse to one predicate rather than two. Every other arm failure is an ordinary rearm-and-retry fault (D-15) and is never mistaken for the downgrade edge. |
| D-62 | **A coarse handle is reopened to widen, exactly like a detailed one (D-52), because `bWatchSubtree` is fixed at `FindFirstChangeNotificationW`'s call, not reconfigurable afterward.** This was not a new mechanism to invent: `WatcherInner::reopen`'s tear-down-then-establish shape already existed for D-52's detailed-widen case, and a coarse handle's identical constraint (reach fixed at open) falls out of the same code path for free. `WatcherInner::teardown_endpoint` dispatches the correct teardown (`cancel_all`+`run_down` for detailed, `stop_and_drain` for coarse) so `reopen` and `DirectoryWatcher::stop` do not need to know which tier they are tearing down. |
| D-63 | **`Established { mode }` reports the tier actually settled on, and fires on every successful establishment, not only a later one after an earlier `Establishing`.** `WatcherInner::mode()` is read fresh from the live `Endpoint` rather than hardcoded, so a client sees `Coarse` when that is genuinely what is watching. This also corrects an M5-era gap found while wiring it: the notification's own doc promised "reported once at first establishment," but the implementation only sent it on a *later* success; `monitor::route_established` now always announces (subject to `report_liveness`) regardless of whether this is the first attempt or a retry. |
| D-64 | **The M6.4 test seam is a `#[cfg(test)]`-only constructor (`DirectoryWatcher::start_forcing_coarse`) backed by an always-present `force_coarse: AtomicBool`, not a feature-gated public API.** M3.8 already retired the `unstable-internals` pattern (D-35) of exposing crate-internals through a public, `#[doc(hidden)]`, feature-gated surface reachable from the external `tests/` integration crate; reintroducing that shape for one test seam would undo that decision. The field always exists (one bool costs nothing and is never read outside `reopen`) rather than being `#[cfg(test)]`-gated itself, which would otherwise force conditional-compilation branches through the tier-decision logic. |
| D-65 | **M6.5's test lives in `src/watcher/tests.rs`, not the external `tests/` integration crate, because the seam it needs (`start_forcing_coarse`) is `pub(crate)`.** The checklist item's word "Integration" describes the test's *character* -- real OS behaviour, end-to-end, not a unit of pure logic -- not its literal location; `tests/` can only reach `pub` items, and the crate-internal unit tree is exactly where M4's and M5's own fault-machinery tests already live for the identical reason (D-6, D-27's earlier tests). |
| D-66 | **M9's data-driven scenario stress suite draws wait durations and choice points from a small hand-rolled seeded PRNG (splitmix64), defaulting to a fixed seed, rather than from an external `rand` dependency or unseeded `SystemTime`-derived randomness.** A fixed default seed keeps the suite reproducible per the repo's no-random-sampling-without-approval rule, while an env-var override (`WINDOWS_FILE_WATCHER_STRESS_SEED`) lets a developer explore other seeds on demand without touching code. The generator is intentionally tiny (one `u64` state, splitmix64's step function) rather than a dependency: this is test-only code with no need for a general-purpose RNG's statistical guarantees, only for a deterministic, easily-reproduced sequence. |
| D-67 | **M9.2's scenario harness is built to describe hundreds of thousands of operations without either the scenario value or the harness's bookkeeping scaling linearly in memory.** `Operation::Repeat { count, pattern }` lets a scenario stay a handful of bytes regardless of how many times `pattern` actually runs, instead of unrolling every repetition into the `Vec`. `run_scenario` tracks only bounded per-kind tallies (`HarnessOutcome`), not a growing `Vec<Notification>`, and drains the queue with non-blocking `try_recv` after every operation so a long operation loop never lets the crate's own bounded queue back up (D-11) between checks. |
| D-68 | **M9.3's churn scenario found that the harness's fixed 120s timeout is a false wedge-positive at real stress scale, not evidence of a watcher fault.** Applying 250,000 `CreateFile` operations one at a time (each a real `std::fs::write` syscall) measured at roughly 1,800 ops/sec on development hardware, so the fixed default timeout expires on operation-application throughput alone, well before the watcher does anything wrong. `HarnessParams::for_operation_count` scales the timeout from the scenario's own `operation_count()` (a conservative 500 ops/sec floor plus a flat settle allowance), so only a genuine stall trips the assertion at any scale; the plain 120s default remains for scenarios that do not describe hundreds of thousands of operations. |
| D-69 | **M9.4's `Fleet` tracks sessions and watches by scenario-given name in two `HashMap`s, not as a single fixed session/watch (M9.1-M9.3's model).** `Monitor::session` mints an independent channel per call (D-2), so a scenario that opens a second session needs its own receiver drained alongside every other open session's -- there is no single receiver to hand the harness once more than one session can be live. Targeting an unknown, already-open, or already-closed name is treated as a scenario-authoring bug (an `assert`/`panic`), not a fault the harness tolerates, mirroring D-15's reopen-retry/rearm-retry/downgrade classification: a lifecycle operation has no analogous "retry" outcome, so misuse fails loudly instead of being silently absorbed. |
| D-70 | **M9.4 gives session/watch lifecycle churn both a delayed and a back-to-back timing posture, as two separate scenarios sharing one generator function, rather than only the tightest possible loop.** Hitting a queue continuously as hard as possible finds throughput bugs, but a fault or a race is frequently a *timing-window* problem that only reproduces when a transition is spaced out enough to land while other activity is genuinely mid-flight -- so `session_watch_churn_with_delays_scenario`'s `(low, high)` wait bounds are also invoked at `(0, 1us)` for the tight-loop variant, rather than authoring the two postures as unrelated scenarios that could drift apart. |
| D-71 | **The `run-scenario` JSON schema (`Operation`/`Scenario`'s `Serialize`/`Deserialize` shape) is explicitly *not* covered by this crate's semver contract, by the engineer's own direction.** "Data-driven" was deliberately left underspecified until the model converged (M9.1-M9.4); once it had, the chosen realization is a persisted JSON file as the actual input to a stress tool, not merely an in-memory value built by Rust code. That schema is a testing/ops tool's input format, not a documented data contract this crate promises library consumers -- so a field rename, a new required field, or any other shape change to the JSON is not a breaking change requiring a major version bump, even though `Operation`/`Scenario` are `pub` Rust types (whose *Rust* API is, as always, covered by ordinary semver). This is recorded explicitly because the module lives in `src/` and is reachable via the public `windows_file_watcher::scenario` path (D-72), which would otherwise invite the assumption that everything reachable through it carries the usual stability promise. |
| D-72 | **`src/scenario.rs` and the `run-scenario` binary are gated behind an opt-in `scenario-tool` Cargo feature with `serde`/`serde_json` as *optional* dependencies, rather than living only behind `[dev-dependencies]` (M9.2-M9.4's original approach) or becoming unconditional dependencies of the published crate.** A `[[bin]]` target is built by plain `cargo build`, unlike `tests`/`examples`, and Cargo does not extend `[dev-dependencies]` to `[[bin]]` targets -- so putting the CLI in the crate at all (the user's chosen option over a separate internal workspace crate) means its dependencies must be real `[dependencies]` to build. Making them `optional = true` behind a default-off feature keeps an ordinary consumer of `windows-file-watcher` from linking `serde`/`serde_json` (or building `run-scenario`) unless they explicitly opt in with `--features scenario-tool`. |
| D-73 | **`WaitRandom`/`Wait` bounds below roughly 20-25ms are not meaningfully distinct on Windows, because `std::thread::sleep` cannot sleep for less than the OS scheduling quantum (Windows's default timer granularity is commonly cited as ~15.6ms, but the effective floor measured in this crate's own stress runs is closer to ~23ms) -- a requested 1ms and a requested 20ms sleep both round up to the same one tick. A scenario author who wants genuinely irregular timing (not just "the same minimum delay every time") must choose `(low, high)` bounds that span at least one multiple of that floor above it, e.g. `(25, 250)` rather than `(1, 20)`; a bound entirely below the floor silently degrades a "spaced out" scenario into the "back-to-back" posture (D-70) without the author noticing. The M9.3/M9.5 fixtures and the M9.4 lifecycle scenarios were corrected to bounds above this floor once the effect was noticed; `Operation::WaitRandom`'s own doc comment now calls this out. |
| D-74 | **M9+.1's `Operation::Concurrent { branches }` is the model's only concurrency primitive, implemented by moving `Fleet` behind a `std::sync::Mutex` and spawning one OS thread per branch with `std::thread::scope`, joining before the next top-level operation runs.** `std::thread::scope` was chosen over `Arc`-based thread spawning because every branch only ever needs to borrow the scenario's already-stack-local `root`/`fleet` for the duration of one fork-join step -- no thread outlives the `apply_operation` call that spawned it, so a scoped borrow is sufficient and avoids reference-counting entirely. Each branch draws its own PRNG seed from the parent `Rng` *before* spawning (never sharing one `Rng` across threads), so a scenario stays reproducible for a given top-level seed regardless of how the OS schedules the branches (D-66). Nesting (M9+.3) needed no separate mechanism: a branch is an ordinary `Vec<Operation>` that may itself contain `Concurrent`, `Repeat`, or any other variant, so composition is free once one concurrency primitive exists. |
| D-75 | **M9+.2's `Operation::HoldOpen` is a genuine Win32 spoiler, not a simulated one: it opens the target file with `share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)`, deliberately omitting `FILE_SHARE_DELETE`, via `std::os::windows::fs::OpenOptionsExt` (no new dependency; `std`'s own Windows extension trait already exposes it).** A concurrent `Rename`/`RemoveFile`/`RemoveDir` targeting the same path while the handle is held fails with a real sharing violation rather than a scenario merely hoping for a race window -- verified in the harness test (`a_held_open_file_blocks_a_concurrent_delete_with_a_real_sharing_violation`) by asserting the file is still present on disk afterward, not just by inspecting notification counts. The two flag values are named constants (`share_mode::READ_WRITE_NO_DELETE`), per the repo's no-bare-manifest-numeric-constants rule. |
| D-76 | **M9+.4's `Operation::OpenSessionBounded` reuses `Monitor::session_with_bound` (already part of the crate's public API) rather than adding a new queue-capacity knob anywhere.** A scenario that wants to deliberately overwhelm the crate's documented backpressure behavior (a full queue stops the producer rather than dropping data, D-11) opens a session with a small bound and then applies far more churn than that bound can hold before the harness ever drains it (draining only happens once per top-level operation, so a single large `Repeat` genuinely saturates a tiny bound). The check this buys is structural, not a specific assertion: the harness's own overall deadline (already asserted by `run_scenario`) is what would fail if backpressure ever became a wedge instead of a stall: `a_deliberately_tiny_queue_bound_never_wedges_under_overwhelming_load` deliberately does not assert on desync counts, only that the run completes and delivers at least one batch. |
| D-77 | **The per-subscription change-type filter that M4 reserved space for is *withdrawn*, not deferred: it is not faithfully implementable under D-6 coalescing, and the one shape of it that looks implementable (namespace-only) is actively harmful to the workload it appears to serve.** The kernel filter is expressed in *change classes* (`FILE_NOTIFY_CHANGE_SIZE`, `_LAST_WRITE`, `_ATTRIBUTES`, `_LAST_ACCESS`, `_SECURITY`, `_FILE_NAME`, `_DIR_NAME`) but records arrive as *action codes* (`FILE_ACTION_*`), and that mapping is lossy in exactly the wrong direction: all five non-namespace classes collapse into the single `ChangeKind::Modified` action. Because a directory has exactly one watcher armed with the *union* of its subscriptions' masks (D-6), a route asking for size-only changes receives `Modified` records it cannot attribute to a class, and must therefore either over-deliver (defeating the filter) or under-deliver (dropping changes it asked for). Restricting the feature to the namespace classes -- which *are* recoverable from the action code -- fails for a different reason: a name appearing is not a file being complete, its content streams in afterward as `Modified`, and the only workable completeness test on Windows is a quiescence heuristic (openable, parses, then quiet for N ms) built on precisely the `Modified` traffic such a filter discards. Filtering is therefore not a neutral reduction in volume; it destroys the crate's only evidence of ongoing work. The contract this crate keeps instead is *completeness*: a change notification is positive evidence that a file was **not** finished, never evidence that it was, and a client can only reason about quiescence if it sees every change. This also makes D-12's unfilterable `Desync` load-bearing rather than incidental -- a gap in the event set must invalidate any in-flight settling window, which is only sound while `Desync` cannot be filtered out. **This decision schedules no work**: there is deliberately no checklist item anywhere for a change-type filter, and the absence is intentional rather than an oversight. If a future need arises, the only implementable shape is a client-side predicate over the already-decoded `ChangeKind` (which cannot narrow the kernel mask and so buys no kernel-side efficiency), not a `FILE_NOTIFY_CHANGE_*` mask on `WatchOptions`; that shape is recorded here and remains unscheduled. Rationale and the full design discussion: [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md) -> D-77. |
| <a id="d-78"></a>D-78 | **A reopen that lands on a different volume than before is a per-subscription confirmation, not a silent continuation or a directory-wide veto.** `WatcherInner::reopen` reopens by path and never checked whether the result is still the same volume, so removable media swapped for different media at the same path (the classic case: NTFS media replaced by FAT32) was silently absorbed, with the client learning about it, if at all, only as an ordinary `Established { Coarse }` should the new volume happen to need the fallback tier. See [Volume identity confirmation on reopen](#volume-identity-confirmation-on-reopen). |
| <a id="d-79"></a>D-79 | **Supersedes [D-54](#d-54): every fault/failure message now carries a `FaultDetail` (this crate's `OpenFailure` classification plus a `FailureCode`), not just which operation faulted.** A client asked to choose a retry delay, or told a subscription failed permanently, previously had no way to know *why* -- `FaultOperation` says only `Open` or `Arm`, and the raw error was logged (D-58) and discarded. `FailureCode` is `Win32(u32)` or `HResult(i32)` rather than one currency: every source in this crate today is a classic last-error API, so `Win32` is the only variant anything currently produces, but a value is kept in the currency it actually arrived in rather than converted through `HRESULT_FROM_WIN32`/`HRESULT_CODE` to force a single shape. See [Failure detail on every fault report](#failure-detail-on-every-fault-report). |
| <a id="d-80"></a>D-80 | **M11.2's fast reopen path is disabled (returns `None` unconditionally), and reopens by `OpenFileById` (file reference), not `ReOpenFile` (handle), when re-enabled.** Both were measured against the actual OS (D-52's precedent) rather than assumed: `ReOpenFile` against a directory handle fails outright with `ERROR_ACCESS_DENIED` for an ordinary, unprivileged process (it needs `SeBackupPrivilege` *enabled*, which `FILE_FLAG_BACKUP_SEMANTICS` does not grant); `OpenFileById` reopens correctly by identity (confirmed delete-pending-safe and recreate-safe) but is path-independent, which is its own hazard (it would silently keep following a moved/renamed directory away from the path a client subscribed to -- caught by comparing `GetFinalPathNameByHandleW` before trusting it); and, independently of both, a handle obtained via `OpenFileById` hangs or (once) crashes with `STATUS_STACK_BUFFER_OVERRUN` once associated with the thread pool's `ThreadpoolIo`/IOCP and armed, for a reason not yet root-caused. See [Reopening by file reference, and why the fast path is off](#reopening-by-file-reference-and-why-the-fast-path-is-off). |
| <a id="d-81"></a>D-81 | **The consumer test surface reuses the delivery model rather than replacing it, and its already-reachable pieces -- `WatchId::from_raw` and every re-exported boundary type -- are blessed as-is, not re-gated.** A downstream consumer tests its own notification-handling code by feeding synthetic `Notification`s through a real `Receiver`: "go below" the `Monitor`, substituting the OS ingest while keeping the delivery model (`Notification`/`Receiver`/queue ordering/doorbell) intact. The reachable pieces shipped public in 0.1, so re-gating them would be a breaking change with no offsetting safety gain. The one further thing a consumer needs -- a `Receiver` it can feed -- was `pub` only inside a private module (hence unreachable), so it is *exposed*, not re-gated, under `test-util` (D-82). See [Consumer test surface](#consumer-test-surface). |
| <a id="d-82"></a>D-82 | **Everything a consumer needs but cannot otherwise reach is exposed behind an off-by-default `test-util` feature, not on the unconditional public surface: the feedable channel (`channel_with_bound` with `Sender`/`Delivery`/`Reservation`, previously `pub` only inside a private module) and valid-by-construction builders for the two unconstructible boundary types (`RelativeName`, `VolumeIdentity`).** This does not reverse [D-64](DESIGN-RATIONALE.md#the-m64-test-seam-is-a-private-constructor-not-a-public-feature-flag-d-64): D-64's seams serve the crate's own tests reaching internal state, for which `#[cfg(test)]`/`pub(crate)` is strictly better; this seam serves a downstream consumer's tests, which `#[cfg(test)]` cannot reach at all, and it exposes the delivery channel and public boundary constructors rather than internal state (so the retired `unstable-internals` objection does not apply). Feature-gating keeps the crate's internal queue sender, and identity/name construction, out of the production API. See [Consumer test surface](#consumer-test-surface). |
| <a id="d-83"></a>D-83 | **The consumer test surface tests the consumer's reactions, not whether this crate would ever emit a given sequence.** Builders are valid-by-construction in the type-safety sense (memory-safe, lossless), not production-domain-validating: a `RelativeName` can still carry a unit sequence the kernel itself never reports (an interior NUL, say), and an impossible ordering or an impossible relationship between two otherwise valid values (a `VolumeChanged` with equal `previous`/`current` serials, each individually a legal `VolumeIdentity`) both remain the consumer's responsibility, as with any hand-fed test double. This fidelity limit is documented on the surface so a passing handler test is not mistaken for confirmation that the crate produces that traffic. See [Consumer test surface](#consumer-test-surface). |
| <a id="d-84"></a>D-84 | **The delivery contract was under-specified, and a second implementation of it -- not a test suite -- is what proved that.** PR #42's example harness promised contract-legal schedules only (its own D-5), which made its generator a second implementation of *this* crate's contract. Converging it took **19 automated review rounds**: eight fixed generated sequences this crate could never emit, five corrected the contract prose itself, and one found a real shipped reliability defect ([`has_room`](#the-has_room-finding-in-this-crate)) on [D-29](#d-29)'s backpressure path. All 278 of this crate's own tests passed throughout and were never going to fail -- they assert what the watcher *does*, and every gap was in what the contract *permits*. The gap categories are workspace-wide and recorded once, in [the workspace design notes](../../DESIGN-NOTES.md#specifying-a-delivery-contract); the decisions amended in response were [D-9](#d-9) (renames never joined), [D-12](#d-12)/[D-30](#d-30) (branch and terminal paths), [D-17](#d-17) (per-tier emission legality), [D-27](#d-27)/[D-28](#d-28) (a fault question is unconditional, and enters as `Arm`), [D-50](#d-50)/[D-78](#d-78) (volume identity: distinct serials, and continuity across reopens), and [D-83](#d-83) (fidelity is type-safety, not production-domain). See [What the second implementation exposed](#what-the-second-implementation-exposed). |


### Queue mediation

Every interaction with a client is a queued request (client -> monitor) or a
queued notification (monitor -> client). The monitor's servicing is driven by a
`ThreadpoolWork` that serializes all resident-state mutations, so there is a
single logical authority, and **the crate never transfers control into client
code** on that path -- there is no sink trait, no callback registration, and no
client-supplied closure. The D-25 doorbell was originally considered a bounded
exception to that; it is not one (D-25): ringing an event is not a callback,
and no client code runs on the cadence path to ring it.
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
not stall the cadence (D-2). The core contract is that a client is *never silently*
left out of sync, and the shape that keeps it has three layers, not one.

**Reserved capacity, not message type, decides what can be lost** (D-33). A
completion's slot is taken at submit and an interactive subscription's fault slot
at registration, so control delivery cannot fail. Change notifications reserve
nothing and are best-effort, because a lost batch is re-derivable by re-scanning
where a lost completion would be a liveness bug.

**Saturation is survived at the arm, not at the enqueue** (D-29). A full queue does
not simply drop the batch: the watcher stops re-arming the read, which propagates
backpressure into the kernel's own change buffer -- a grace period rather than a
loss, and nothing is lost at all if the client drains in time. A batch can still
arrive to a full ring (a control reservation may have taken the room since the read
was armed), and that one is dropped; if the kernel buffer overflows first, that is
the already-specified `Desync { Overflow }`.

**The loss report is latched out of band, and lands where the loss actually
happened** (D-28/D-39). `Desync { QueueFull }` cannot be pushed onto the very queue
that is full, so the sender holds a latched set of `WatchId`s owed one, coalesced
(a second loss adds nothing, since the response to one is the response to ten). The
latch is flushed into the queue at the next successful enqueue -- **not** ahead of
the next batch, which would claim the hole is older than it is. The queue was full
when the loss occurred, so everything still queued precedes it; a freed slot goes to
the *report* and the new notification is re-latched. A receiver draining to empty
synthesizes any remainder directly, so the report never depends on future traffic
arriving. A zero-capacity bound is unrepresentable (D-40). (D-11, D-12, D-28, D-29,
D-33, D-39)

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
There is a **fifth cause, and it is not the same fact**: `Desync { Stopped }` is
terminal. It is published once by `record_stop`, reached only when a re-establish
attempt's own open fails in a way D-22 classifies as permanent -- the single edge
D-14's "no terminal fault state" does not cover, because retrying would spin
forever against a target that can never become watchable again. A re-scan does not
resynchronize it; nothing further will ever arrive for that watch, and the reason
is readable from `Monitor::stop_reason`. A client that treats the cause as purely
advisory and re-scans on every desync will therefore re-scan forever against a dead
watch, which is why the cause is advisory across the recoverable four and
match-worthy on the fifth. (D-12, D-22)

### Completeness is the contract (no change-type filter)

A client's real question is almost never "which class of change was this?" It is
"is this file finished?" Windows cannot answer that: `ReadDirectoryChangesW`
reports that something happened, never that nothing more will. So a change record
is **positive evidence of incompleteness only** -- it proves the file was not done
as of that moment, and proves nothing at all about whether it is done now. The
only workable completeness test is a quiescence heuristic layered on top: the file
opens, its content parses, and then no further change arrives for some settling
window.

That is why this crate delivers every change it observes and offers no
per-subscription change-type filter (D-77). A filter is not a neutral reduction in
volume; the `Modified` traffic it would discard is exactly the input the
quiescence heuristic runs on, so filtering removes the evidence the client needs
rather than merely the events it does not want. The same reasoning makes D-12's
`Desync` unfilterable by construction: a hole in the event set must invalidate any
in-flight settling window, and that is only sound while a client cannot opt out of
seeing the hole.

### Fault model

On any I/O error the monitor enters a re-establish loop that never terminates of
its own accord: there is no failure state, only "not yet re-established." Every
error classifies into *reopen-and-retry*, *rearm-and-retry*, or
*downgrade-to-coarse*; nothing throws or gives up. The client can cancel from any
intermediate state (D-14, D-15). Retry timing is D-27's ask/resolve/floor
protocol (superseding D-16's resident-data-only design): defaults or interactive,
chosen per subscription, with the earliest answer winning and a decliner counted
at the default.

A directory has exactly one coalesced watcher (D-6), so it runs exactly one
reopen/re-arm cadence even when several subscriptions with different retry modes
share it -- D-27's earliest-answer reduction is what reconciles them, not a
per-field policy merge. **What is not implemented:** the growth multiplier, cap,
jitter, and per-error-kind override this paragraph described before D-27 replaced
D-16 remain hypothetical -- `RetryMode` has no field for any of them (D-56), so
every retry uses the same fixed per-operation default (or interactive answer)
every time, with no backoff growth across repeated faults. Extending this is a
future `WatchOptions` addition, not a gap in what shipped.

### Two-tier watching

Detailed watching (`ReadDirectoryChangesW` on a `ThreadpoolIo`) is preferred, but
not every filesystem supports it. The universal floor is the coarse
`FindFirstChangeNotification` family, watched with a `ThreadpoolWait`; each coarse
activation carries no detail and so becomes `Desync { Coarse }`. Which tier a
directory uses is a property of its **volume**, resolved during establish and
re-establish (`WatcherInner::reopen`) by attempting the detailed arm first: an
unsupported-class error (`ERROR_INVALID_FUNCTION` / `ERROR_NOT_SUPPORTED`, via the
same `directory::classify` the open path uses) downgrades to coarse; a retryable
error uses the reopen loop instead (D-60/D-61). The coarse handle is closed with
`FindCloseChangeNotification` (not `CloseHandle`); because `ThreadpoolWait`'s
default `OwnedHandle` path would close it with `CloseHandle`, it reaches the pool
through the **custom-close waitable owner** `windows-threadpool-sys` provides
(M17), which drains the wait before invoking `FindCloseChangeNotification`. (D-17)

### Volume identity confirmation on reopen

A reopen (`WatcherInner::reopen`, driven by `retry_reestablish`, `add_route`'s
widen, and the very first establish) reopens by *path*, and Windows gives no
guarantee that the path still names the same volume it did before -- removable
media can be ejected and replaced with different media mounted at the same
drive letter or path, most commonly NTFS media swapped for something coarser
like FAT32. Before D-78 this was invisible: the watcher silently kept running
under the same `WatchId`s against what is, physically, a different filesystem.

The fix is a per-subscription confirmation, not a directory-wide gate: a
reopen that lands on a different `VolumeIdentity` (filesystem name and volume
label; the volume serial number is already tracked by `DirectoryId`, D-50)
asks only the routes on that directory whose `WatchOptions` opted into
`VolumeChangePolicy::Confirm` (default remains `AutoContinue`, matching every
prior release's behavior for a client that does not ask). Each route's answer
(`VolumeChangeDecision::Continue` or `::Stop`, an enum rather than a bool so a
future third option is additive) affects only that route -- `Stop` removes
just that subscription through the existing `remove_route` path, `Continue`
leaves it and updates its baseline `VolumeIdentity` to the one just
confirmed, so the *next* reopen compares against the volume the client just
accepted rather than the original one. `AutoContinue` routes are never asked.

Arming is a single, shared operation per coalesced directory (D-6), so it
cannot proceed until every asked route has answered: a new `ArmGate` variant,
`VolumeChangePending`, blocks the shared arm exactly the way `Faulted` does,
resolving back to `Open` once the awaiting set empties (decliners already
removed by then). If that leaves zero routes, the watcher tears down through
the pre-existing zero-routes path -- no special case is needed there.

`WatcherInner::on_path_based_reopen` detects the mismatch and, rather than
installing the already-open candidate handle immediately, parks it in a new
`VolumeChangeState { handle, awaiting, decisions }` held in
`WatcherInner::volume_change` and sends `Notification::VolumeChanged` only to
the awaiting routes' `fault_slot`s. This is the same standing-slot
reservation `RetryQuestion` uses (widened at `subscribe()` time to cover
`retry == Interactive || on_volume_change == Confirm`), which is sound
because the two questions are provably never concurrently outstanding for one
route: a volume-change question can only arise from inside
`retry_reestablish`, which only runs once any prior fault-question for that
route has already been fully answered. `Session::answer_volume_change`
submits `Request::AnswerVolumeChange`, resolved by
`WatcherInner::answer_volume_change`/`resolve_volume_change` -- deliberately
*not* performed synchronously inside the timer/IOCP callback that detected
the mismatch, because those run on a different OS thread than the monitor's
single servicer, and tearing down a now-empty watcher is only safe from the
servicer thread (the same self-deadlock hazard `Request::Cancel` already
avoids). `remove_route_from_volume_change` mirrors D-27's "leaving counts as
declining": a route removed while its question is outstanding resolves as
`Stop` for that route rather than wedging the awaiting set forever.

A reopen tries `OpenFileById` against the file reference `DirectoryId` already
computes, using whichever handle is currently installed only as the volume
hint (`hVolumeHint`) `OpenFileById` requires -- not itself the object being
reopened, so it stays valid even once that object is gone. Reopening by file
reference rather than by handle (`ReOpenFile`) or by path (`CreateFileW`) is
structurally incapable of landing on a different filesystem object than the
one the reference already names, so when it succeeds the volume is provably
unchanged and no `VolumeIdentity` comparison is needed for *that* purpose at
all. It fails only when the original object is genuinely gone (deleted, or its
media was ejected), which is exactly when the path-based fallback is needed --
and only that fallback path can legitimately land on a different `DirectoryId`,
so only it re-keys `Resident.directories` (previously fixed at first
insertion, never updated -- a second latent bug this closes: a stale key would
have made a later new subscription to the same path fail to coalesce onto the
existing watcher and spin up a redundant second one).

### Reopening by file reference, and why the fast path is off

D-80: two rounds of measurement (D-52's precedent -- verify empirically,
never assume) replaced the reopen mechanism above's original design and then
suspended it entirely.

**Round 1 -- `ReOpenFile` does not work here.** The original design tried
`ReOpenFile` against the watcher's still-live previous handle. Measured
against a real directory handle, this consistently failed with
`ERROR_ACCESS_DENIED`, independent of the requested access mask or whether
`FILE_FLAG_BACKUP_SEMANTICS` was included. This matches documented Windows
behavior: reopening a directory this way needs `SeBackupPrivilege` *enabled*
on the caller's token, a privilege `FILE_FLAG_BACKUP_SEMANTICS` only exempts
at a *fresh* `CreateFileW`, not at `ReOpenFile`. An ordinary process (not an
admin or backup-operator tool) does not have this privilege, so the mechanism
is not viable for this crate's general audience.

**Round 2 -- `OpenFileById` works for identity, but exposes a path hazard and
an unexplained IOCP defect.** `OpenFileById` opens by file reference number
(exactly what `DirectoryId` already carries) plus a volume-hint handle, and
needs no special privilege. Measured correct on every identity question: it
reopens the same object while delete-pending, ignores an unrelated object
later recreated at the same path, and (unlike `ReOpenFile`) *does* keep
following the original object if it is renamed or moved elsewhere in the
namespace -- confirmed by `directory::tests`. That last property is also
exactly the hazard it introduces: a client subscribed to a path expects to
watch *that path*, not wherever the object ends up, so a candidate obtained
this way must have its current path (`GetFinalPathNameByHandleW`) checked
against this watcher's own recorded canonical path before being trusted --
this is what `WatcherInner`'s `canonical_path` field and
`reopen_via_existing_handle`'s comparison exist for.

Even with that guarded, a further, independent defect was found: a handle
obtained via `OpenFileById`, once handed into this crate's ordinary
establish path (`UnassociatedEndpoint::assume_overlapped` ->
`ThreadpoolIo::new` -> armed with `ReadDirectoryChangesW`), reliably fails to
resolve the fault it was reopening for -- the read never completes -- and on
one run crashed the process with `STATUS_STACK_BUFFER_OVERRUN`. Bisection
(temporarily short-circuiting `reopen_via_existing_handle` to return early)
localized this to the `OpenFileById` handle's interaction with IOCP
association/arming specifically: `DirectoryHandle::reopen_by_id` and
`canonical_path` are each independently correct per their own unit tests, and
the defect reproduces with the path check never reached. The cause is not
yet understood.

**Current state:** `WatcherInner::reopen_via_existing_handle` returns `None`
unconditionally, so every reopen uses the path-based fallback -- the
DirectoryId/VolumeIdentity comparison and `Resident.directories` re-keying
described above, which do not depend on the fast path and are unaffected. The
`OpenFileById`/`canonical_path` machinery is kept, verified, and ready; only
the wiring that would hand its result into IOCP association is disabled,
pending whoever root-causes that interaction.

### Failure detail on every fault report

Before D-79, `Notification::RetryQuestion` carried only
[`FaultOperation`](../windows-file-watcher/src/retry.rs) (`Open` or `Arm`) and
`Outcome::Failed` carried only the coarse `OpenFailure` classification --
neither told a client *why*, only *what kind of thing failed*. D-54's
reasoning (no subsequent decision reads the raw error, so do not keep it) was
sound for the machinery that existed then, but it also meant a client with no
way to explain a failure to a user, log it usefully, or decide anything more
specific than a wait duration. A client facing a fault it does not understand
-- for example a transient resource exhaustion that happens to classify as
`Unsupported` without any real filesystem change -- can now at least see the
real code and decide for itself; retrying with a client-chosen delay (D-27,
unchanged) remains the whole answer when nothing more specific can be done.

`FailureCode` is `Win32(u32)` or `HResult(i32)`, not a single converted
currency: every failure source in this crate (`CreateFileW`,
`ReadDirectoryChangesW`, `FindFirstChangeNotificationW`,
`GetVolumeInformationByHandleW`, `ReOpenFile`) is a classic last-error API, so
only `Win32` is produced today, but a code is kept in whichever currency it
actually arrived in rather than run through `HRESULT_FROM_WIN32`/
`HRESULT_CODE` to force one shape -- either direction of that conversion is
lossy and this crate has no present need to blend the two. `FaultDetail`
bundles a `FailureCode` with the existing `OpenFailure` classification (D-22,
already reused for both Open- and Arm-class faults per D-61), and both
`Notification::RetryQuestion` and `Outcome::Failed` carry one.

Getting the raw code into `FaultDetail` fixed a second bug along the way:
`WatcherInner::retry_reestablish`'s open-class fault path already had a fully
classified `OpenError` (real classification, real code) but reached
`enter_fault` via `io::Error::other(open_error)` -- wrapping a classified
error in an opaque boxed one erases both the classification and
`raw_os_error()` before `enter_fault` ever saw them. `enter_fault` now takes
`(OpenFailure, FailureCode)` directly, so every call site must classify at the
source instead of laundering a real error through a generic one.

`OpenFailure::NotADirectory` turns out to be unreachable through `subscribe`
in current practice, discovered while writing M10.5's integration test:
`open_target` redirects any top-level `NotADirectory` into `open_file_target`
(D-7's file-target fallback), and that fallback's own `DirectoryHandle::open`
call is against the target's *real, already-existing* parent directory --
which, whenever the top-level classification was genuinely `NotADirectory`
(the target exists and is a file), necessarily succeeds. The permanent-failure
integration test uses `InvalidPath` (an interior NUL) instead, which `wide_path`
rejects before any syscall and so is reachable deterministically. This is a
pre-existing property of D-7's design, not something M10 changed; `stopped`'s
own permanent-stop path (`WatcherInner::record_stop`) can still classify a
later re-establish's `NotADirectory` the same way `subscribe` does, and is
equally hard to hit for the same structural reason.

### Consumer test surface

A downstream consumer of this crate reacts to `Notification`s drained from a
`Receiver`. To let that consumer test its *own* reaction logic cheaply and
deterministically -- with no real filesystem and no thread pool -- the crate lets
the consumer feed a real `Receiver` itself, rather than only receiving one from
`Monitor::session`. This is the "go below" seam: the consumer substitutes the OS
ingest (the source of notifications) while keeping the crate's delivery model --
`Notification`, `Receiver`, queue ordering, the doorbell -- intact. Substituting
*above* the delivery model, or replacing it wholesale, would discard exactly the
behavior the consumer is trying to test against (D-81).

Because the consumer becomes the driver -- it decides what to push and when --
its test is deterministic without this crate shipping any scheduler or virtual
clock. Reproducibility falls out of removing the crate's own concurrency from the
consumer's test, not out of modeling it.

The reachable part of the seam was already public: `WatchId::from_raw` and every
boundary type (`Notification`, `Change`, `DesyncCause`, `Outcome`,
`FaultDetail`/`OpenFailure`/`FailureCode`, `WatchMode`, `FaultOperation`,
`ChangeKind`) are re-exported at the crate root, so a consumer can already tag and
match notifications. These are blessed as-is rather than re-gated (D-81). Two
things a consumer additionally needs were not reachable, and are exposed behind
the off-by-default `test-util` feature (D-82): the feedable channel --
`channel_with_bound`, with `Sender`, `Delivery`, and `Reservation` -- which was
`pub` only inside the private `queue` module; and valid-by-construction `for_test`
builders for the two boundary types with no consumer-reachable constructor,
`RelativeName` (inside a `Change`) and `VolumeIdentity` (inside a `VolumeChanged`).

The feature gate is an audience distinction, not a reversal of D-64. D-64's
`#[cfg(test)]`/`pub(crate)` seams exist for the crate's own tests to reach
internal state, and a public feature there would leak internal state for no gain.
This seam exists for a *downstream* consumer's tests, which `#[cfg(test)]` cannot
reach at all (the cfg is not set when the crate is compiled as a dependency), so
a feature is the only mechanism that reaches them -- and it exposes public
boundary constructors, not internal state, so the anti-pattern that retired
`unstable-internals` (a `#[doc(hidden)]` window into internals) is not what this
is.

The surface tests a consumer's reactions, not the crate's production of a given
sequence (D-83): the builders are valid-by-construction only in the type-safety
sense (memory-safe, lossless) -- a `RelativeName` can still carry a unit
sequence the kernel never reports (an interior NUL, say). An impossible
*ordering*, or an impossible *relationship between two otherwise valid values*
(a `VolumeChanged` with equal `previous`/`current` serials, say), is likewise
the consumer's responsibility, as with any hand-fed test double.

### What the second implementation exposed

D-84. The example harness published alongside this crate promises to generate
only schedules this crate could actually emit. That promise is what made it a
**second implementation of this crate's delivery contract**, and converging it
took 19 automated review rounds -- more review-response commits than the
original implementation had.

The value is not the harness. It is that a contract stated as prose can only be
*read*, never *executed*, so nothing forces its gaps into the open. A second
implementation has to make a decision at every point the prose is silent, and
each decision it gets wrong is a place the contract failed to say something.
This crate's own test suite could not have found any of it: 278 tests passed
throughout, because a test asserts one point in the set of legal sequences and
every gap was in the *boundary* of that set.

The ten categories those findings fall into are workspace-wide and recorded in
[the workspace design notes](../../DESIGN-NOTES.md#specifying-a-delivery-contract),
since `windows-overlapped-io-sys` and `windows-ioring-sys` publish contracts of
the same shape. What is specific to this crate is which of its decisions turned
out to be stated incompletely, and how:

- **[D-27](#d-27)/[D-28](#d-28) said the monitor "asks" on fault.** It did not
  say *always*: `enter_fault` puts every interactive route in the awaiting set
  and asks on every fault, with no probability anywhere. Nor did it say which
  operation a *live* watch's fault enters as -- always `Arm`, with `Open`
  reachable only as a re-entry into an already-unresolved bracket.
- **[D-17](#d-17) named the two tiers but not what each can emit.** A Coarse
  endpoint's `on_activation` publishes `Desync { Coarse }` for activity, and
  `Batch` and `Overflow` are Detailed-only -- `Overflow` because it is the
  *kernel change buffer's* own overflow, which only a detailed read observes.
  `QueueFull`, `Reestablished` and `Stopped` are **not** restricted by tier: a
  coarse activation rides the same best-effort queue, so saturation latches a
  `QueueFull` against it exactly as against a `Batch`. The tier decision stated
  none of this.
- **[D-50](#d-50)/[D-78](#d-78) specified volume identity per message, not
  across messages.** `install` stores the confirmed identity, so a watch's next
  `VolumeChanged.previous` must equal its own prior `.current` -- a continuity
  rule that only exists *between* two notifications and so had no natural home
  in either one's description.
- **[D-12](#d-12)/[D-30](#d-30) enumerated the success path exhaustively and
  the branches by omission.** `Completion { Failed }` is not only an initial
  outcome (`rekey` emits it for an already-routed watch), and a fault question
  is not always followed by `Desync { Reestablished }` -- a pending retry can
  ask again, and `record_stop` can end the watch with `Desync { Stopped }`.
- **[D-9](#d-9) said renames are never joined; it did not say what that
  permits.** A legal batch may carry a lone half, both halves, or halves with
  unrelated records between them.

Each of those is now stated. [M14.1](CHECKLIST.md) then ran the converse pass over
the sequencing decisions a consumer builds recovery on; see
[The M14 audit](#the-m14-audit) below. The remaining decisions are M14.2's.

### <a id="the-m14-audit"></a>The M14 audit: the sequencing decisions against all ten categories

Reactive fixes only ever reach the categories a reviewer happened to probe, so
M14.1 asked the question deliberately, for each of
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract), of the
three decisions carrying the rules a consumer's recovery logic rests on. "Not
applicable" below means the category has no instance here, and is distinguished
throughout from "unspecified", which means the contract deliberately declines to
say.

It found three shipped defects, corrected in the same change, and two rules that
were true of the code but stated nowhere.

**[D-12](#d-12), the `Desync` primitive.**

| # | Answer |
|---|---|
| 1 | No `WatchOptions` field suppresses any desync. Unfilterability is load-bearing, not incidental (D-77): a hole must invalidate an in-flight settling window. |
| 2 | Unconditional. `Desync { Reestablished }` is published on **every** successful re-establishment and is *not* gated on `report_liveness`, unlike the `Resumed`/`Established` that follow it. |
| 3 | Per tier: `Overflow` is Detailed-only, `Coarse` is Coarse-only, and `QueueFull`/`Reestablished`/`Stopped` are tier-independent. **Was unstated; now stated.** |
| 4 | `Desync { Reestablished }` is always published *before* the `Resumed`/`Established` for the same recovery, so a client is told to re-scan the gap before being told it can trust incremental changes again. |
| 5 | Not every `(watch, cause)` pair is reachable -- the tier restriction in 3 constrains it, and nothing follows a `Stopped` for that watch. |
| 6 | `Reestablished` only ever leaves an unresolved fault bracket; `Coarse` fires on every coarse activation including the first; `Stopped` is reachable only from a re-establish attempt, never from initial registration (that path reports `Completion { Failed }`). |
| 7 | **`Stopped` is terminal and was documented as though it were not** -- see the defects below. |
| 8 | A desync is never correlated with what was lost. The crate says a hole exists, never what fell in it; there is no partial-recovery path. |
| 9 | Not applicable -- a desync carries no name or boundary-typed value. |
| 10 | `test-util` can build tier-impossible pairs (a Coarse watch's `Overflow`); D-83's fidelity limit governs. |

**[D-27](#d-27)/[D-28](#d-28), the fault protocol.**

| # | Answer |
|---|---|
| 1 | **Three** independent `WatchOptions` fields, not two: `retry`, `on_volume_change`, and `report_liveness`. All eight combinations are legal. A standing slot is taken iff `Interactive \|\| Confirm`. |
| 2 | Unconditional -- every interactive route is asked on every fault, with no probability anywhere. The source reads conditionally (`retry == Interactive && fault_slot.is_some()`), but the second conjunct is *implied* by the first: `subscribe` fails with `WouldBlock` when the reservation cannot be carved out, so an interactive route always has its slot. |
| 3 | A `RetryQuestion` reaches only an `Interactive` subscription, a `VolumeChanged` only a `Confirm` one -- both over the same standing slot. |
| 4 | **The shared slot rests on a mutual-exclusion invariant** -- a `RetryQuestion` and a `VolumeChanged` are never outstanding at once for one subscription, because a volume-change question is only raised by a reopen that already succeeded, and the gate blocks arming while one is pending. This was a source comment, load-bearing for a reservation's soundness, and stated nowhere in the contract. **Now stated.** |
| 5 | A live watch's fault always enters as `Arm`; `Open` reaches a route only by re-entering an already-unresolved bracket. |
| 6 | As 5 -- entry state, not just transition label, is what distinguishes the two. |
| 7 | A question does not always resolve with `Desync { Reestablished }`: a retry loop can ask again, and `record_stop` can terminate the watch with `Desync { Stopped }` instead. |
| 8 | Answers are keyed by `WatchId` alone, never to a question instance, so a late answer to an already-resolved fault is silently discarded rather than misapplied. Deliberate, and deliberately not a correlation the client can rely on. |
| 9 | `FailureCode` keeps a code in the currency it arrived in (`Win32` or `HResult`) rather than forcing one shape through `HRESULT_FROM_WIN32` (D-79). |
| 10 | As D-12's row 10. |

**[D-30](#d-30), request completions.**

| # | Answer |
|---|---|
| 1 | No option suppresses a completion; reliability is reservation, not opt-in (D-33). |
| 2 | **"Every request produces a completion" is true of every *lifecycle* request, not of every `Request` variant** -- `Answer` and `AnswerVolumeChange` are responses to questions the crate posed and deliberately carry none. D-30's blanket wording did not admit the exception. **Now stated.** |
| 3 | `Subscribed`/`Establishing`/`Failed` arise only from a subscribe; `Cancelled` only from a cancellation. |
| 4 | A subscription yields exactly one registration outcome and at most one `Cancelled`. |
| 5 | `Establishing` is not a failure and carries no terminal meaning (D-46); only D-22's permanent pair reaches `Failed`. |
| 6 | `Failed` is not only an initial-registration outcome -- `rekey` emits it for an already-routed watch. |
| 7 | `Cancelled` is the stream boundary and is sent *after* teardown, which is what makes D-30's ordering claim structural rather than a race. |
| 8 | Completions are keyed by `WatchId`, never by a request identity; the crate issues none, so two requests for one watch are distinguished by their `Outcome` alone. |
| 9 | `Failed { detail }` carries the full `FaultDetail` rather than a reduced code (D-79). |
| 10 | As D-12's row 10. |

**The three defects this pass found**, all in prose or rustdoc rather than behaviour,
and all corrected here:

1. **`DesyncCause`'s own doc contradicted its own variant.** The enum said "the
   cause is advisory: the client's response is the same in every case (a re-scan)"
   while `Stopped` said "unlike every other cause, a re-scan will not resynchronize
   this watch". A consumer reading the type-level doc -- the one a reader reaches
   first -- would re-scan forever against a dead watch. `Notification::Desync`'s
   `cause` field carried the same claim.
2. **[The Desync primitive](#the-desync-primitive) enumerated four causes.** There
   are five; `Stopped` was absent, so the section's "all four are the same fact to
   a client" was the exact over-generalisation defect 1 shipped.
3. **[Delivery and saturation](#delivery-and-saturation) still described the
   pre-D-29 design.** It said a full queue "drops the batch" (D-29 replaced that
   with throttling at the arm) and that the latch is "surfaced ahead of the next
   successful batch" -- the precise phrasing [D-39](#d-39) identifies as wrong,
   because it would claim the hole is older than it is.

All three are the same shape: a decision was superseded or extended, the index row
was updated, and the prose section a reader actually reads was not. That the
per-decision rows were correct throughout is what let it go unnoticed.

**[D-10](#d-10)/[D-13](#d-13)/[D-17](#d-17)/[D-26](#d-26)/[D-57](#d-57), the
notification-shaping decisions**, audited by M14.2 the same way. Only the rows that
were not already answered above are listed.

| # | Answer |
|---|---|
| 1 | `report_liveness` is the third independent option (M14.2 confirms D-27/D-28's row 1 from the other side): it gates `Suspended`/`Resumed`/`Established` and nothing else, and never creates a standing slot (D-57). |
| 2 | `Established` is *conditional in a second way* beyond `report_liveness`: a route coalescing onto an already-faulted watcher is deliberately not told a tier, since there is no settled one to name. |
| 3 | One decoded completion yields **zero, one, or many** `Batch` notifications, not one. `publish` de-multiplexes across every route (D-6) and a route whose filtered subset is empty is skipped, on the same reasoning D-26 applies to a wholly empty completion. **D-10's "one completion = one batch" is per-subscription, and only when that subscription's filtered subset is non-empty.** |
| 4 | **The tier is not sticky.** `reopen` re-resolves it on every call (D-61), so `Established { Detailed }` then `Established { Coarse }` for one watch is legal, as is the reverse -- detailed is retried first every time, so a downgrade is not permanent. A client caching "this watch is detailed" is caching something the contract never promised. |
| 5 | `Suspended`/`Resumed` carry only a `WatchId`; `Established` pairs it with a tier, both of whose values are always reachable. |
| 6 | **A `Resumed` does not imply this subscription saw the matching `Suspended`.** A route that coalesces onto a faulted watcher joins `routes` after `enter_fault` has sent its `Suspended`s, so it receives `Resumed` (and its first `Established`) out of a bracket it never saw open. |
| 7 | **A `Suspended` is not always followed by `Resumed`.** If the re-establish attempt's own open fails permanently, `record_stop` publishes `Desync { Stopped }` and sends no `Resumed` and no `Established` -- the bracket is closed by a terminator instead. A client balancing brackets must treat `Stopped` as closing every open one. |
| 8 | An `Established` is never correlated with the `Completion` for the same registration; their relative order is fixed only in the immediate-success case. |
| 9 | Not applicable -- none of these five carry a boundary-typed value. |
| 10 | As D-12's row 10. |

**What M14.2 found**, all in the contract's description of sequences rather than in
behaviour:

1. **A liveness bracket can open with `Resumed`.** Rows 6 and 7 above are the two
   halves of the same omission: the contract described `Suspended`/`Resumed` as a
   pair, and both a mid-fault join (opens without `Suspended`) and a permanent stop
   (closes without `Resumed`) break the pairing. A consumer that counts brackets --
   the obvious way to track "is my watch live?" -- is wrong in both directions.
2. **`Established` is not the first notification a liveness subscription sees.**
   For a route joining a faulted watcher the order is `Completion { Subscribed }`,
   then later `Desync { Reestablished }`, `Resumed`, `Established`.
3. **The tier can change between establishments**, which "reported once at first
   establishment and again after every re-establishment" does not imply.

One correction to M14.1's own table falls out of this pass: its D-27/D-28 row 2 says
the `fault_slot.is_some()` conjunct is *implied* by `retry == Interactive`. That
holds for every route `subscribe` builds, which is every route in production, but
the crate's own unit tests construct `Route` directly and do pair `Interactive` with
`fault_slot: None`. The guard is therefore live code, not redundant, and the honest
statement is that the pairing is an invariant of the public registration path rather
than of the type.

### <a id="the-has_room-finding-in-this-crate"></a>The `has_room` finding

One round found a genuine defect in shipped 0.1 code rather than in the harness.
`Sender::has_room` reported `free() > 0`, but `Sender::send` flushes every owed
latched desync into the queue *before* considering the caller's notification --
so a freed slot with a latch outstanding was never available to a new
notification, and `has_room` returned `true` for a slot the next `send` would
consume for the flush.

`WatcherInner::arm_locked` gates re-arming on `any_route_has_room()`, so this
sat directly on [D-29](#d-29)'s backpressure path. The intended behaviour is to
leave changes in the kernel's buffer as a grace period rather than let a batch
complete with nowhere to go; the bug produced exactly the outcome D-29 exists to
prevent -- arm, complete, flush takes the slot, batch dropped and re-latched.
Fixed with a regression test that had no predecessor: no existing test called
`has_room` with a latch outstanding.

The transferable rule is recorded workspace-wide: an advisory predicate another
subsystem's reliability gate depends on is not advisory, and must be tested in
the condition that gate uses it under.

### <a id="the-m143-predicate-sweep"></a>The M14.3 predicate sweep

`has_room` was found by review rather than by looking, so nothing established it
was the only one. M14.3 enumerated every predicate this crate exposes or consumes
and asked the same question of each: **does its stated contract hold under the
condition its caller actually uses it in?**

| Predicate | Used by | On a reliability gate? | Verdict |
|---|---|---|---|
| `Sender::has_room` | `any_route_has_room` -> `arm_locked` | Yes -- [D-29](#d-29) backpressure | The original finding; accounts for the latch since `700e0eb` |
| `Receiver::is_empty` / `len` | a client's own drain loop | Yes -- a drain loop is the client's liveness gate | **Defect. See below.** |
| `Receiver::is_disconnected` | end-of-stream detection | Yes | Honest: `senders == 0` is exactly what it claims |
| `Receiver::latched` / `capacity` | diagnostics | No | Informational, and correct |
| `DirectoryWatcher::stop_reason().is_none()` | `route_established`, to discard a permanently-stopped watcher before coalescing onto it | Yes | Sound: read under the same `resident` lock as the insert that acts on it, so there is no window |
| `DirectoryWatcher::is_faulted` | `route_established`, to suppress a tier report with no settled tier | Yes | Sound, same critical section |
| `Session::is_open` / `Servicer::is_open` | advisory pre-check | No -- `submit` returns `Rejected` regardless | The right shape: the fallible *operation* is authoritative, the predicate is a courtesy |
| `Monitor::is_registered` / `is_watching` / `is_running` | [D-31](#d-31) observability | No | Point-in-time snapshots by construction, and documented as such |
| `OpenFailure::is_retryable` | [D-15](#d-15) classification | Yes | Pure function of a value; no timing dimension, so the shape cannot arise |

**The defect: `Receiver::is_empty` is not "nothing to take".** It is `len() == 0`,
and `len` deliberately excludes latched losses -- so a drained queue that still
owes a `Desync { QueueFull }` reports itself empty while `recv` would still yield
that report. The caller's actual question is the drain loop's, and
`while !receiver.is_empty()` exits with a loss still owed.

It is worse than a silent miss, because of [D-41](#d-41): the doorbell is signalled
whenever the receiver has something to take, and "something to take" is
deliberately *three* things -- a queued notification, an owed latched loss, and the
end of the stream. A client that waits on the doorbell and then tests `is_empty`
therefore **spins**: the event stays signalled because a loss is owed, and the
predicate keeps saying there is nothing to collect. That is the `has_room` shape
exactly -- a predicate consulted at a gate whose condition it does not model.

The evidence it was already biting: **this crate's own tests never use `is_empty`
alone.** They write `!receiver.is_empty() || receiver.latched() > 0`, and
`receiver.is_disconnected() && receiver.is_empty() && receiver.latched() == 0`.
Every internal call site that means "is there anything for me" reconstructs the
conjunction by hand, which is the same signal `has_room` gave before it was fixed.

The fix is to publish the predicate D-41 already defines internally rather than to
redefine `is_empty` (whose `len() == 0` correspondence is a Rust-wide expectation
and is not itself wrong): `Receiver::has_pending` *is* `State::pending`, the exact
condition the doorbell is signalled on, so a client waiting on the doorbell now has
a predicate that matches it. `is_empty` and `len` keep their meaning and gained the
warning they lacked. Three regression tests cover the drained-with-a-loss-owed
state, agreement with the doorbell at every step of that transition, and the
disconnected-and-empty case -- none of which had a predecessor.

**The sequel: fixing a predicate without fixing its wake edge.** The review round
that followed M14.3 found the other half of the original `has_room` defect, and it
is the sharper lesson of the two.

`has_room` was corrected to `free() > latched.len()`. The prod that *wakes* a
parked producer was left at its original `free() == 1`. Those two expressions agree
only while nothing is latched -- which is exactly the state that never matters,
because a producer only parks when the queue is saturated, and saturation is what
creates latches. With one loss owed, draining a slot took `free()` to 1 and fired
the prod, but `has_room` was still `1 > 1` = false, so the producer re-checked and
stayed parked. The next drain took `free()` to 2, where `has_room` finally became
true -- and `free() != 1`, so **no prod was ever sent again**. The watcher remained
`Backpressured` with room available, permanently, and no error anywhere.

That is [D-47](#d-47)'s lost-wake failure returning through a door D-47 did not
guard. D-47 reasoned carefully about *when* the prod is rung relative to the gate
lock, and got that right; what it did not anticipate is the wake *threshold* and
the arm predicate being two separately-written expressions that a later fix to one
would silently desynchronise from the other.

So the fix is not to correct the second expression to match the first -- that just
restores the coupling and waits for the next edit to break it again. Both are now
derived from one quantity, `State::best_effort_room()`: `has_room` is `> 0` and the
wake edge is `== 1`. Every step that frees capacity raises it by exactly one, so the
edge cannot be stepped over. **A predicate and the edge that wakes it are one
decision, and must have one source of truth.**

Worth recording about the test, too. The first regression test written for this
passed against the buggy code, because it only asserted the prod count *after* the
transition -- where both versions reach 1, differing only in *when*. Asserting the
count at the intermediate step (still 0 while `has_room` is false) is what
distinguishes them. A regression test for a timing defect has to be run against the
unfixed code to be worth anything, which is now how both were confirmed.

**A third round: the audit was right and the generator was not.** The review after
the wake-edge fix found that the harness generator excluded `Desync { QueueFull }`
for a Coarse-tier watch, with a test codifying that exclusion -- while
[the M14 audit's own table](#the-m14-audit) had just recorded `QueueFull` as
tier-independent.

The audit's table is correct, and the source settles it: `on_activation` publishes
`Desync { Coarse }` through `publish` -> `route.sink.send`, which is the same
best-effort path a `Batch` takes, so a saturated client latches a `QueueFull`
against a coarse watch exactly as it would a detailed one. Only `Overflow` is
Detailed-only, because it is the *kernel change buffer's* overflow and a coarse
handle never reports one.

The generator's error was a plausible-sounding conflation -- "`Overflow` and
`QueueFull` are the two loss causes, and a coarse watcher's losses are `Coarse`"
-- which reads as one rule and is really two, one true and one false. Worth
recording because it is the mirror image of every other finding in this section:
here the *contract* was stated correctly and the second implementation still
diverged, so writing the rule down is necessary and not sufficient. Anything that
was built against the pre-audit understanding has to be re-checked against the
post-audit statement, and nothing does that automatically.

The corrected test also asserts that a coarse `QueueFull` is actually *generated*,
not merely permitted -- without that, the test would pass just as well against a
generator that still excluded it, which is the same weakness the wake-edge
regression test had in its first form.

### <a id="dead-code-that-could-not-have-run"></a>Dead code that could not have run: `StandingHold::drop`

Mutation testing left four survivors in `StandingHold::drop`, and the interesting
part was not the survivors but why they survived.

The reachability question settles from the source alone. Every `StandingHold` is
built in one place (`StandingSlot::send`) and moved straight into `state.queue`.
The only site anywhere in the crate that removes an entry from that queue is
`take`, which settles the reservation inline with the pop and sets `resolved`, so
`Drop` returns at its first line. The only other way a hold dies is `Shared` being
torn down, where the hold's `Weak` fails to upgrade and it returns at its second.
Nothing reaches the body in between -- confirmed by replacing it with
`unreachable!()` and passing the full suite.

The history says it was not always so. In `4198aa8` this `Drop` *was* the drain
path: it "unconditionally restored `reserved` on drain." `07d4b75` then found that
popping exposed the queue slot before the deferred `Drop` restored the
reservation, so `queue.len() + reserved` could exceed capacity; the fix moved the
release into `take`, inline with the pop, and left `Drop` as "the fallback for
every other discard." The body was live code whose only caller moved out from
under it.

**The finding that decided what to do about it: the body could not have run
safely.** `take` takes `&mut State`, so its caller holds the `items` guard -- and
any other way to remove an entry from `state.queue` needs that same guard. A hold
discarded on such a path is therefore dropped *inside* the lock, and the body's
first act was `lock(&shared.items)`, a plain non-reentrant `Mutex::lock`. So the
"fallback for every other discard" would have deadlocked in precisely the
situation it was written for. Measured, not reasoned: with a forced unwind out of
`take`, the body hung past 90s; the identical unwind with `Drop` short-circuited
failed immediately.

Building the missing discard path was never an option either, and the tests
already said so: `dropping_a_standing_slot_while_its_message_is_still_queued_releases_capacity_once`
asserts that a cancelled slot's queued question **still arrives**. There is no
discard to fall back from.

So the body is gone and an assertion stands in its place, phrased as
`debug_assert!(std::thread::panicking(), ...)` -- which is the true statement
rather than a bare `false`. The only way to reach it today is an unwind out of
`take` between the pop and `resolved` being set, and there the original panic is
the real diagnostic and must be left to propagate rather than turned into an abort
by a second one. Any *other* arrival is a new discard path that has not settled
its reservation, and it fires. **The contract it encodes: a discard must release
the reservation under the `items` lock it already holds, exactly as `take` does --
never by delegating to a hold's `Drop`.**

Two transferable points. First, "unreachable" and "harmless" are different
claims, and the second does not follow from the first: this body was unreachable
*and* was a deadlock waiting for its first caller. Second, the survivors were the
symptom of a doc that had gone false by vacuity -- `Entry`, `StandingHold`,
`StandingState`, and `take` all described this `Drop` as the live release
mechanism, so a reader would have trusted a fallback that could not work. Four
restatements of one fact, none of which moved when the fact did.