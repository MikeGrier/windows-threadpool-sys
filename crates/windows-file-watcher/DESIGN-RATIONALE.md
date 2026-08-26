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


## Arming is gated by a lock, not a flag (D-23)

The first implementation used a `Mutex<bool>` checked *before* submitting a
read. That deadlocked under test: a completion callback passes the check,
teardown then cancels the outstanding read and begins waiting for rundown, and
only then does the callback finish submitting -- leaving a fresh pending read
that nothing will ever complete, because only a future directory change could.
The fix generalizes past "check a flag" to "hold the lock across the entire
submission": teardown's own acquisition then waits for any in-flight submission
to finish, and once it closes the gate, no new one can start. The `Weak`-upgrade
suppression in the completion callback looks like it should be enough on its
own -- it is not, because during `Drop` the strong count is still non-zero, so
the upgrade still succeeds and the callback still runs.

## Open failures are bad input, not faults (D-22)

The instinct on first classifying an open failure is to make everything
retryable, matching D-14's "no terminal state." That is wrong for exactly two
cases: a path that names something other than a directory, and a path Win32
cannot even receive (an interior NUL). Both are the caller naming something that
can never become a watched directory -- retrying spins forever against input
that will never change, which is a resource leak dressed up as patience, not
recovery. The permanent/retryable split is therefore about *whose problem it
is* -- caller input vs. environment -- not about severity. An unrecognised error
still classifies retryable, deliberately: a watcher that gives up on an error
code it has never seen is a watcher that silently stops working on some future
Windows release.

## The fault latch became a standing reservation, not resident state (D-28, D-55)

The first sketch of D-28 described a fault as *resident watcher state* -- one
error code plus one bit, allocated with the watcher, the same shape as a queue
depth counter. That framing quietly broke two promises the crate had already
made (D-12, D-26): a fault communicated out-of-band, rather than riding the
notification queue in order, destroys exactly the "a client seeing a `Desync`
knows everything before it is accounted for" guarantee. The corrected shape
treats a fault as a **message**, not a flag, delivered in-stream like anything
else -- and a message that must never be lost needs a *reservation*, not a
best-effort send. A reservation taken fresh per fault (the ordinary
`Sender::reserve` shape) would not do, because it can fail if the queue happens
to be full at exactly the wrong moment, and a fault report failing to enqueue is
the one loss this protocol cannot survive. So `StandingSlot` (D-55) generalizes
`Reservation` into a *permanent* carve-out, taken once at registration and never
released until the subscription ends. The proof this is sufficient, not merely
convenient, is D-28's own observation: a watcher cannot fault twice
concurrently, so at most one question is ever outstanding per subscription, and
one permanently-reserved slot is provably always enough.

## Fixed retry delays, not a policy-reduction engine (D-27, D-56)

An earlier design pass (recorded in the "Fault model" prose of D-14/D-15/D-16,
before D-27 replaced D-16) imagined a per-field *soonest-recovering reduction*
over several subscriptions' retry policies -- minimum initial delay, minimum
growth multiplier, minimum cap, minimum jitter, minimum per-error-kind override.
None of that survived contact with what `WatchOptions` actually carries:
`RetryMode` is a two-variant enum, `Defaults` or `Interactive`, with no field for
any of those knobs. Building the reduction machinery anyway -- against fields
that do not exist -- would have been speculative generality serving no caller.
D-27's literal text (`Azure/m`'s shipped 500ms default / 50ms floor) is exactly
what shipped, and the earlier language is left in the design notes as an
explicit, labelled "not implemented" marker rather than quietly deleted, so a
future contributor adding real per-subscription tuning knows where the seam
was always meant to go.

## Two independent retry loops, not one shared object (D-59)

It is tempting to unify "a still-`Pending` subscription retrying its own open"
and "a coalesced watcher retrying its arm" into one retry engine, since both
apply the identical ask/resolve/floor protocol (D-27). They stay separate
because their *ownership* genuinely differs: a `Pending` subscription has no
directory identity yet (D-6's coalescing only happens once a directory is
successfully opened), so its retry reduction is trivially over a set of exactly
one; a coalesced watcher's is a real reduction over however many routes
currently share it. Forcing them through one shared type would mean modelling
"sometimes exactly one, sometimes many" generically, for no payoff -- the
protocol logic (`resolve_and_schedule`, the earliest-answer accumulation) is
already factored out at the right level (D-27's rules), and each owner simply
applies it against its own `ThreadpoolTimer`.

## Cancel-and-resubmit does not widen a live read, measured directly (D-52)

The obvious way to widen a directory watcher's reach to recursive -- cancel the
outstanding `ReadDirectoryChangesW`, then resubmit with `bWatchSubtree = TRUE`
on the *same* handle -- was the original plan (it is what M4.4's checklist item
originally said). It was tried first and does not work: after the resubmit, the
kernel kept reporting only the directory's direct children, and a change nested
one level down was never reported, for as long as a test was willing to wait or
however many further changes it made. This was measured directly with debug
instrumentation before being accepted as fact. A fresh `CreateFileW` does not
have the problem -- the filesystem's recursive-watch attachment is evidently
tied to the *handle's creation*, not reconfigurable on a live one. This is why
`reopen` tears the endpoint down and rebuilds it from a new handle rather than
attempting any in-place reconfiguration, and it is also why M6's coarse handle
(whose `bWatchSubtree` is likewise fixed at `FindFirstChangeNotificationW`) reuses
the identical mechanism (D-62) rather than needing one of its own.

## One `WatcherInner`, two tiers, not two watcher types (D-60)

M6 could have added a wholly separate `CoarseWatcher` type, parallel to
`DirectoryWatcher`, with the monitor choosing which to construct and the
`Resident.directories` map holding an enum of the two. That would have
duplicated the entire coalescing, routing, and fault/retry machinery M4 and M5
had just finished building, for a difference that is genuinely narrow: which
Win32 API arms a read and what a completion looks like. So instead
`WatcherInner` gained one `Endpoint` field (`Detailed(ThreadpoolIo)` or
`Coarse(ThreadpoolWait)`), and only `arm_locked` (which API to call) and
completion handling (`on_completion` vs. the new `on_activation`) branch on it;
routes, the fault state machine, backpressure, and teardown are identical code
regardless of tier. The cost of this choice is that `reopen` has to know how to
build *either* tier and fall back between them, which it already needed to do
for the downgrade edge (D-17) in the first place.

## The M6.4 test seam is a private constructor, not a public feature flag (D-64)

M3.8 already retired `unstable-internals`, a feature-gated, `#[doc(hidden)]`
public surface that let the external `tests/` integration crate reach
crate-internal state for exactly this kind of forcing seam. Reintroducing that
shape for M6's "force coarse mode" need would have undone that decision for one
test's convenience. `DirectoryWatcher::start_forcing_coarse` is instead
`#[cfg(test)]` and reachable only from the crate's own unit-test tree -- which is
also where M6.5's test lives, alongside M4's and M5's own fault-machinery tests,
for the identical reason (D-65): the seam they all need is `pub(crate)`, and
`tests/` can only ever reach `pub` items.

## The change-type filter was withdrawn, not deferred (D-77)

M4 shipped `WatchOptions` as `#[non_exhaustive]` and said so in its own doc
comment: "Non-exhaustive because M4 adds the change-type filter here." The filter
never arrived, and the planned shape of it -- a `FILE_NOTIFY_CHANGE_*` mask (or an
equivalent set of change classes) per subscription -- turns out not to be a feature
that was merely unfinished. It is a feature that cannot be built honestly on this
API, and whose most plausible surviving fragment is worse than not having it.

### Why the obvious shape is not implementable

Win32 splits the concept in two and does not let you rejoin it. The *arm* side
takes classes:

    FILE_NOTIFY_CHANGE_FILE_NAME  DIR_NAME  ATTRIBUTES
    FILE_NOTIFY_CHANGE_SIZE  LAST_WRITE  LAST_ACCESS  SECURITY

The *delivery* side returns actions:

    FILE_ACTION_ADDED  REMOVED  MODIFIED  RENAMED_OLD_NAME  RENAMED_NEW_NAME

which this crate decodes into `ChangeKind`. The mapping is many-to-one and lossy
in the direction that matters: `SIZE`, `LAST_WRITE`, `ATTRIBUTES`, `LAST_ACCESS`,
and `SECURITY` all surface as a single `FILE_ACTION_MODIFIED`. A record does not
carry the class that caused it.

That is survivable for a lone subscription, because the mask you armed is the only
mask in play. It is not survivable under D-6: a directory has exactly one
`ReadDirectoryChangesW` outstanding, armed with the **union** of every
subscription's mask, and records are routed afterward. So a subscription that
asked for `SIZE` only is handed `Modified` records generated by some other
subscription's `LAST_ACCESS` interest, with no way to tell them apart. The router
must then pick a way to be wrong -- deliver them (the filter does nothing, which is
the honest failure) or drop them (the filter silently loses changes the client did
ask for, which is the dangerous one). Dropping the coalescing to make the filter
exact would mean one handle and one outstanding read per subscription rather than
per directory, which is precisely the cost D-6 exists to avoid.

### Why the surviving fragment is worse than nothing

`FILE_NAME` and `DIR_NAME` *are* recoverable from the action code, so a
namespace-only filter ("tell me about creates, deletes, and renames; suppress
content changes") is exactly implementable. It also looks like the feature people
actually want: the drop-directory pattern, where a producer writes a file into a
watched folder and a consumer picks it up.

It does not survive contact with that workload. A name appearing in a directory is
not a file being complete -- the content streams in afterward, and every byte of it
arrives as `Modified`. Windows offers no "writer finished" signal at all, so the
only test that works is a quiescence heuristic: the file opens, its content parses,
and then nothing further is heard about it for a settling window. Same-directory
temp staging does not rescue the namespace-only view either: a producer that writes
`x.tmp` beside the target and renames it emits its content churn inside the same
watched directory, and the sanitary replace-in-place sequence produces roughly five
namespace events for one logical publish. The client still has to reason about
which of them mattered.

The decisive point is what the filter costs. The quiescence heuristic runs *on* the
`Modified` traffic; the settling timer is reset by exactly the events a namespace
filter is defined to discard. Filtering is therefore not a reduction in noise, it
is the removal of the only signal that distinguishes "still being written" from
"finished." A namespace-only filter is most harmful precisely in the workload that
motivates it.

### What replaces it

The contract stated positively (and recorded in Tier 1 as "Completeness is the
contract"): a change notification is positive evidence that a file was **not**
finished, never evidence that it was. The crate's job is to deliver every change it
observes, so that a client can build the quiescence test it actually needs. This
also explains why D-12's `Desync` is unfilterable by construction rather than by
omission: a hole in the event set must invalidate any in-flight settling window,
and that is only sound while a client cannot opt out of hearing about the hole.

Consequences recorded so nothing is left implicit:

- **No work is scheduled.** There is deliberately no checklist item for a
  change-type filter in any milestone or in `M-inf`. The absence is the decision,
  not an oversight.
- **`WatchOptions` stays `#[non_exhaustive]`**, but for the ordinary reason (future
  additive options), not because a filter is coming. Its doc comment was corrected
  to stop claiming otherwise.
- **The only implementable shape, if a need ever appears**, is a client-side
  predicate over the already-decoded `ChangeKind`, applied at routing time. It is
  worth being clear about what that is and is not: it cannot narrow the kernel
  mask, so it buys no kernel-side or buffer-side efficiency, and it inherits the
  same hazard -- a client that predicates away `Modified` has disarmed its own
  quiescence detection. It is recorded here as the shape to reach for, and is
  unscheduled.

## The consumer test surface, and why it is a feature when D-64 was not (D-81, D-82, D-83)

The question this crate now answers for its *consumers* is: this is fiddly
concurrent code; how do I test my own code that reacts to it without standing up
a real filesystem and thread pool and paying for the flakiness that brings? The
answer is to let a consumer feed a real `Receiver` with synthetic notifications
and drive its own handler deterministically -- "going below" the `Monitor` to
substitute the OS ingest while keeping the delivery model the consumer is testing
against.

Three options were weighed for where a consumer intervenes. *Going above*
(wrapping the public API) is messy and discards the delivery model. *Replacing*
the crate discards it outright. *Going below* -- the chosen path -- keeps
`Notification`/`Receiver`/queue semantics and substitutes only the notification
source, the one option that preserves what the consumer is trying to test
against.

An audit found the seam was ~90% already public (`channel_with_bound`,
`Sender::send`, `WatchId::from_raw`, and every boundary enum). Blessing those
rather than re-gating them (D-81) was chosen because they shipped in 0.1 and are
harmless to expose: re-gating would be a breaking change bought for nothing. The
two gaps (`RelativeName`, `VolumeIdentity`) get builders behind an off-by-default
`test-util` feature (D-82).

That feature is *not* a reversal of D-64, and the distinction is audience. D-64
kept `DirectoryWatcher::start_forcing_coarse` and the other forcing seams
`#[cfg(test)]`/`pub(crate)` because they serve the crate's *own* test tree
reaching *internal* state, and it explicitly declined to reintroduce
`unstable-internals` -- a `#[doc(hidden)]`, feature-gated public window into
internals -- for one test's convenience. This seam is the opposite case on both
axes: its audience is a *downstream* consumer's own tests, which `#[cfg(test)]`
cannot reach (the cfg is unset when this crate is a dependency), so a feature is
the *only* mechanism that reaches them; and what it exposes is *public boundary
constructors* (valid-by-construction `RelativeName`/`VolumeIdentity`), not a
window into internal state. Feature-gating rather than making them unconditional
keeps production code from forging identities it should only ever receive from
the crate. So D-64 and D-82 follow one rule -- gate a seam to match its audience
-- rather than contradicting each other.

The surface's honest limit (D-83) is that it tests the consumer's reactions, not
whether this crate would ever emit the fed sequence. Valid-by-construction
builders stop a consumer minting an impossible *value*; an impossible *ordering*
is still theirs to avoid, exactly as with any hand-authored test double.
