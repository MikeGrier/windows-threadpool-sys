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
in both directions, and the crate never transfers control into client code on its
cadence path.

An earlier phrasing of this said "no client code executes on a monitor/threadpool
thread". That is false, and was never promisable: the process thread pool is not
this crate's, and a client draining the queue from its own `ThreadpoolWait`
callback -- the integration the D-25 doorbell exists to enable -- *is* client code
on a pool thread. The enforceable property is about the call graph instead: we
never call into the client, so nothing the client does can stall or unwind our
cadence. See D-25 for the single, bounded exception.

This originally forced the retry-control shape into **resident policy data** with
no per-fault exchange at all (D-16). That was overturned on 2026-08-21 by D-27,
and the overturn is worth recording carefully, because D-16's reasoning was half
right.

Its two objections were:

1. **"The hook is in the queue."** A synchronous callback would run on the pool
   thread -- the exact thing we're avoiding.
2. **"There must be no race."** A reactive answer arriving on the request queue
   while the monitor's retry timer is already scheduled is, by construction, a
   race.

Objection 1 was written against a **synchronous callback** and never applied to a
*queued* exchange, where the client receives a fault notification and responds
through the ordinary request queue with no callback anywhere. The D-2 correction
above weakened it further.

Objection 2 is the load-bearing one, and it dissolves on inspection: the race
exists only if a timer is **already scheduled** when the answer arrives. It is
not. On fault the watcher latches and schedules *nothing*, so there is no timer
to race. The race was an artifact of wanting to keep retrying while asking; drop
that, and the objection goes with it.

What survives from D-16 is its reduction rule -- one coalesced watcher over
several subscriptions needs a deterministic winner -- and that carries into D-27
unchanged, now applied to answers rather than to resident values.

There is a real cost, stated plainly: interactive mode makes recovery depend on
the client answering, which D-14's "retries autonomously and indefinitely" does
not. That is why the mode is **per subscription and opt-in**: a client that says
nothing keeps the autonomous behaviour, so D-14 remains true by default rather
than being quietly weakened everywhere.

## Two rings would not have fixed it (D-29)

When the queue-fills problem arose, a two-ring split -- control and observation
-- was proposed, on the grounds that one ring with two different contracts is
confusing. It was rejected for a decisive reason: **two rings gives you two rings
that can both fill.** Ring count is a taxonomy question and the problem is
unchanged by it.

The actual constraint is that neither obvious response is available. *Blocking*
at a full ring is a deadlock rather than backpressure, because the writer is an
I/O completion holding a pool thread and the client's drain may itself be a pool
callback -- the very doorbell integration D-25 exists to enable -- so the cadence
can block pool threads waiting for a drain that needs one. *Dropping* is fine for
observation, whose loss `Desync` makes recoverable, and unavailable for control,
whose loss is a liveness bug because the client waits forever for something that
already happened.

So each producer is throttled somewhere other than at the enqueue, which is what
D-29 records. The observation half is the interesting one: refusing to re-arm the
read pushes backpressure into the *kernel's* change buffer, which turns what had
been an immediate loss into a grace period, and makes the loss that does
eventually occur an honest kernel overflow rather than a drop we chose.

## The doorbell should not have been a trait (D-25)

The first proposal was a client-implemented `Doorbell` trait, justified by
composition: an event handle reaches only Win32 waits, a method reaches a
semaphore, a completion-port post, or an async `Waker`.

That justification does not survive contact with the platform. On Windows the
HANDLE **is** the universal waitable currency, so an event is the native
composition point rather than a lowest common denominator. And the single case it
does not reach -- an async `Waker` -- is a short bridge the client writes on *its
own* pool. Abstracting that bridge into a trait would have moved the client's
wake onto our cadence path, importing precisely the problem D-2 exists to
prevent: the trait would have solved a composition problem by creating a
correctness one.

Owning the doorbell removed the D-2 exception outright, removed the generic
parameter that would have infected `Monitor`, `Session`, and `Sender`, removed
the must-not-block/must-not-panic contract, and moved the reset discipline from a
client obligation to an internal invariant -- so lost wakeups became impossible
by construction rather than by documentation.

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
completion puts arbitrary client code directly on the cadence path -- the precise
thing D-2 exists to forbid -- and a `Send + Sync` bound cannot enforce the
promised non-blocking, infallible, panic-free behavior. A client `deliver` that
blocks, panics, or is slow would stall or unwind the cadence path. So the sink is
instead a **crate-owned concrete queue sender**: `deliver` is a crate-internal
enqueue, and the client only ever *receives*. The guarantee then holds
structurally rather than by trusting a callback.

The D-25 doorbell is the deliberate counter-example, and the contrast is the
point: it is admitted precisely because it is *small enough to specify* -- ring a
bell, touch nothing, return -- where a full `deliver` carrying a batch is not.
The objection was never "a callback exists", it was "unbounded client work sits
on the cadence path".

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

## The decoder accepts only an exactly-described buffer (D-21)

The decoder's job at a completion is to account for *every* byte the kernel says
it returned. The failure mode this rule guards against is the quiet one: bytes
that the record chain does not describe get discarded, the batch is returned as
`Changes`, and the client is told everything is fine while changes have gone
missing. That is strictly worse than a `Desync`, whose only cost is a re-scan
(D-12).

The precise rule follows from the wire format rather than from a tolerance we
picked. `FILE_NOTIFY_INFORMATION` has a 12-byte fixed header, and `FileNameLength`
counts *bytes* of a UTF-16 name -- which the decoder separately rejects unless it
is a whole number of 2-byte code units. A record's content therefore always ends
at `12 + even`, an **even** offset. Records are DWORD-aligned, so the padding that
carries an even offset up to the next 4-byte boundary is exactly **0 or 2 bytes,
never 1 or 3**. A final record (`NextEntryOffset == 0`) may thus legitimately end
the buffer at exactly one of two lengths, and any other remainder -- a 1- or
3-byte tail, or a whole further record whose link was zeroed -- is undescribed
data and is reported as a desync.

The original check bounded the tail (`rec.len() > padded_end`) instead of
enumerating the two legal lengths. That is the intuitive spelling of "allow the
padding," but because it accepts everything *up to* the aligned end, it also
accepted a 1-byte tail at the alignments where padding is 2 -- silently dropping
a truncated completion. The lesson generalizes: when a format permits an exact
set of lengths, assert membership in that set, not a bound around it. Both the
padding case and the misaligned-tail case are pinned by tests
(`zero_offset_trailing_alignment_padding_decodes_cleanly`,
`zero_offset_misaligned_trailing_tail_is_desync`,
`zero_offset_with_trailing_record_is_desync`).
