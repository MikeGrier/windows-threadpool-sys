# Design rationale: win-multi-file-tail-watcher (Tier 2)

_Written 2026-10-14 19:03 -04:00 - anchored to commit `57471c8f`._

How the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached and what was set aside. Tier 1
wins on any conflict. The full discussion is the
[inception design session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md);
each section below links the part of it that matters.

## TW-D-1: what the crate is for

The brief: a process launches many child workers whose stdout and stderr go to files, which proved
the most robust arrangement if the parent fails. The existing service polled each file on its own
timer, which does not scale. The engineer's analysis of where the crate wins: at large populations
one beat replaces many timer expirations; at small ones a loop over a few handles costs less than
the same number of dispatched callbacks; in between, the comparison depends on file count, beat
interval and change rate, and thread-pool timer coalescing (`SetThreadpoolTimerEx`'s window) makes
the per-file baseline a family of configurations. Producing that comparison was placed with the
component that chooses between the approaches.
[Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#where-the-crate-beats-per-file-timers-the-engineers-analysis)

**End of stream** was considered as an input (retire a file when its child exits) and rejected: the
crate knows registrations, not processes, and the client unregisters when it no longer wants
updates. A final drain is something the client composes by polling once more before unregistering.
[Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q6-the-source-of-length-changes-is-not-the-watchers-business)

## TW-D-2: a file's length is read through a held handle

The brief claimed that querying a file's length also triggers its change notification. Experiments
on NTFS and ReFS refined that: only the *path-based* query does, and only on NTFS; the held-handle
query never does. Since nothing depends on the notifications, the held handle was chosen: it is
current on both file systems, provokes nothing, and avoids an open and close per file per beat.
Whether "buffered" in the brief meant the child's own stdio buffering was asked and settled as the
operating system's view only.
[Experiments](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#experiments-re-verifying-the-premises),
[Q1](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q1-and-the-goal-behind-the-crate),
[Q16](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q16-buffered-means-the-os-view-only)

## TW-D-3: registration

Considered and set aside: adopting a caller-supplied handle (the brief registers by path);
accepting `Wtf8String` as well (one encoding, the one the open needs); registering a path before
it exists and retrying the open on later beats ("for now" the path must exist, which leaves that
possible later). A starting offset past the length was first thought to share an answer with
shrinkage, then given its own error, because at registration it is detectable immediately.
[Q4](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q4),
[Q5, Q13-Q15](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answers-to-q5-q13-q14-q15),
[Q19](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q19-registration-as-an-raii-value),
[Q23](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q23-a-caller-type-t-per-registration)

## TW-D-4: what an attention carries

An early sketch modelled each file as a chain of monotonic cursors (written, announced, consumed)
with a byte-range type; it was withdrawn once lengths were allowed to shrink, and the engineer's
"last known" and "current" replaced the range. Whether "last known" should mean last *reported* or
last *consumed* decided whether a path back from the caller was needed; last reported was chosen as
simpler, which removed the consumed report and partial consumption from the design. For `T`, a
borrow was ruled out once attentions could outlive registrations; between a clone and a shared
pointer, `Clone` was chosen because the lifetime management is otherwise too hard to work out.
[Ring analysis](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q3-and-whether-this-is-ring-shaped),
[last known and current](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#what-an-attention-carries),
[Q27](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q27-last-known-means-last-reported),
[Q24](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answers-to-q24-and-q31)

## TW-D-5: the beat

The engineer inverted the expected control: the caller, not the crate, runs the timer, because the
process-control parent must manage cadence and keep the crate idle between polls. "Idle" was first
read as zero resources of any kind and settled as no thread wakeups. A synchronous `poll` that fans
out and joins was weighed against one that initiates and signals later; the engineer is against
synchronous APIs, so `poll` initiates. Concurrent polls were rejected under the engineer's rule
"whichever is simpler". The thread count was left open deliberately: whether 20,000 queries per
beat need sharding cannot be known before code exists.
[Q2, Q3](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#elaboration-of-q2-and-q3-iterations-and-who-owns-the-beat),
[Q7](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q7-about-20000-files-at-the-top-end),
[Q17](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q17-the-measure-is-thread-wakeups),
[Q18](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q18-leaning-asynchronous),
[Q28](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q28-one-poll-at-a-time)

## TW-D-6: delivery

The engineer wondered whether the mechanism was ring-shaped; the underlying requirement turned out
to be dispatch of an unbounded, growing population of attentions. Candidates considered: a visitor
called during the poll, a returned list, an intrusive ready list (as epoll does), a completion
port, and a waitable queue that merges records by key. The engineer chose an IoRing-shaped
submission queue with a doorbell. That the caller would supply the SQ's storage was later reversed:
the crate creates the SQ and the events, because it must fix the events' reset mode and because the
SQ then lives as long as the watcher. The merge-by-key requirement and the CQ both fell away as
other answers settled. Poll completion as a distinct doorbell was rejected in favour of a poll-done
SQ entry; a reserved SQ slot for it was proposed and declined as unnecessary.
[Dispatch](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#correction-the-requirement-is-dispatch-not-a-ring),
[Q20](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q20-resolved-reading-b-with-a-doorbell),
[Q26](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#q26-completed-manual-reset-doorbells),
[creation](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#who-creates-the-queues-and-doorbells),
[Q12](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#q12-superseded),
[Q32](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q32-there-is-no-cq),
[Q35](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q35-no-special-mechanism)

**The SQ's storage (TW-2.2, 2026-10-15).** The design said an SQ and a capacity and nothing about
its shape. A mutex around a deque would have met it, and was set aside because the poll is meant to
stay shardable across threads ([TW-D-5](DESIGN-NOTES.md#tw-d-5)) and a lock on every write is what
[TW-D-7](DESIGN-NOTES.md#tw-d-7) is written against. The repository already has a bounded
many-writer one-reader queue, windows-waitable-queues' `slotwise_mpsc`, so the SQ is built on it,
and its capacity rule (a power of two, at least two) became the SQ's. That queue's other shapes
were not compared; the choice is the first that fit, and the SQ's interface hides it. Its own
doorbell is unused, since the two doorbells are the crate's own events
([TW-D-6](DESIGN-NOTES.md#tw-d-6)).

**The doorbells' reset (TW-2.3, 2026-10-15).** Manual-reset was settled in TW-D-6; what it left open
is who resets and when. Resetting inside `drain` is the one place a caller cannot get the order wrong,
and was not taken because a drain dropped early would leave the doorbell clear over entries still
queued. Rather than add a re-ring on drop to repair that, TW-2.3 leaves the reset to the caller with
the order stated, and records `drain` as open. The sibling queue crate's doorbell, which this is
modelled on, is not exported, so the wrapper is rewritten here and not shared.

## TW-D-7: a full SQ stops the poll; no locks around I/O

Waiting at a full queue was set aside in line with
[windows-file-watcher's D-29](../windows-file-watcher/DESIGN-NOTES.md#d-29) (throttle, never block
at a full queue). The engineer proposed the stop-and-resume shape and the no-locks-around-I/O rule,
with shared ownership of the handle as the means of probing outside a lock (first a bare `SharedHandle`,
later an observer; see TW-D-10).
[Q25](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q25-resume-by-roster-index-no-locks-around-io-a-working-position),
[Q29](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#q18-q31-and-q29-firmed-up)

**The three-pass poll (2026-10-15, after TW-2.3).** The engineer proposed it while the roster's
storage and the poll's enumeration were being questioned, and it replaced a poll that kept an index
into the roster: copy each entry's id, last known length and observer under the lock; probe with the
lock released; then, for the changes, take the lock again and write the attentions, stopping when
the SQ fills and continuing from the last one written. Full discussion in the
[design session](design-sessions/DESIGN-SESSION-2026-10-15-poll-passes.md). What was weighed:

- **A cursor into the roster** (the earlier working position) was set aside. It needs the probe and
  the report interleaved per entry, which keeps the roster lock's scope tied to the file system; the
  passes separate them, so only pass 2 touches the file system and it holds no lock.
- **Restarting at pass 1 on resume** would find only the unreported changes, because reporting
  updates the last known length, with no kept position at all. It was set aside for its cost: it
  probes every file again on every resume, which is about `n` probes per resume for an SQ that
  holds fewer entries than there are changes. Continuing from a kept index reads a list that is
  already built; its price is that an attention can report a length that is a few resumes old.
- **A `BTreeMap` for the copy** would also keep every allocation small, but the copy is only
  appended to and read in order, so it needs no lookup, and clearing it frees every node on every
  poll. A chunked sequence of at most 64 KiB per chunk, held between polls, was chosen.
- **`Arc<T>` inside the roster**, so that `T::clone` could run outside the lock, was rejected by the
  engineer: `T` may be an integer or a boolean and should not be wrapped. The lock does not depend on
  `T`; a `Clone` that re-enters the watcher is the one hazard, and the documentation says so.
- **Asking the SQ for its free space** before pushing was considered so that no `T` is cloned for a
  refused write. The queue's own free-space count is a snapshot and documented as unsuitable for
  control flow, so a push can still be refused; the cost it would avoid is one clone per stop.
  Reserve-then-fill is how the sibling queue crate's other shape avoids it, and was not adopted.
- **The lock rule's scope.** The engineer's original wording said the file system, the SQ write and
  the doorbell should be outside any lock. With an SQ that never waits, the SQ write is not that
  kind of operation, and "no lock" cannot literally hold in a process whose allocator and kernel
  take locks. It now says the roster lock, and the operations that can wait on the outside world.

## TW-D-8: registration and unregistration during a poll

Making `Drop` wait for the poll, or deferring removal to the poll's end, was considered; the shared
handle made waiting unnecessary, and the engineer judged that suppressing late attentions was moot
since one may already be in the SQ.
[Q22](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q22-unregistration-during-a-poll-is-legal)

## TW-D-9: cancellation is shutdown

The question began as whether a thread-pool wait on the resume doorbell can be cancelled. It can,
through `windows-threadpool-sys`' `ThreadpoolWait`, whose `try_cancel_pending` is documented as able
to stall the whole pool, so draining was chosen. The engineer could find no use for cancelling one
poll and restarting, so cancellation became shutdown. Considered and set aside: `Drop` as the only
shutdown (it blocks); the caller's callback returning a stop value (with no CQ there is no path for
it to reach the crate); a reserved slot or separate event for the final signal (unnecessary).
[Cancellation](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#cancellation-means-shutdown),
[Q34](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q34-shutdown-is-requested-signalled-then-dropped),
[stop value](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#a-stop-result-returned-from-the-callers-callback-set-aside),
[Q35](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q35-no-special-mechanism)

**Corrected 2026-10-15**, after a review of the branch: the decision had the request stop and drain
the resume wait, and promised `Drop` would then have nothing to wait for. Draining waits for a
callback inside a probe, so a request made while a poll was probing would block, against its own
promise; and the Q35 answer lets a caller drop without reading the poll-done entry, so `Drop` could
always find a callback to wait for. The session's own analysis of Q35 had already put the wait at
"at most a running poll reaching its next roster entry"; the decision text had not followed it. The
drain moved from the request into `Drop`. Whether to add a signal that would let `Drop` promise no
wait at all was put as Q36; the engineer answered that `Drop` has to be allowed to block, so none is
added -- the bounded wait in `Drop` is the design, not a gap in it.

## TW-D-10: one crate, shared handles

Three ways to reconcile the roster's `SharedHandle` with windows-overlapped-io-sys' exclusively
owned endpoint were laid out: share the endpoint behind an `Arc` (no change elsewhere), teach the
endpoint to hold a `SharedHandle` (a change to that crate), or open two handles per file. The
engineer chose the second and asked for the owned-versus-shared implications to be investigated up
front rather than when reads arrive.

That investigation (windows-overlapped-io-sys M16.1) found that exclusivity protects "one endpoint
per handle", not "one holder per handle", and that an endpoint generic over owned or shared storage
would need an `unsafe` constructor whose `SharedHandle` clones could become second endpoints. The
engineer asked for a structural remedy instead and chose *observers*: shared holders that can query
but implement no handle-lending trait, so they can never reach an endpoint constructor. A second,
non-overlapped handle per file was again set aside: "a small user mode allocation beats a second
kernel mode allocation". Using observers from the first probe made M16 a prerequisite of iteration 1.
[windows-overlapped-io-sys DESIGN-NOTES.md](../windows-overlapped-io-sys/DESIGN-NOTES.md)
[Q10](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q10-one-crate),
[Q30](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q30-the-endpoint-holds-a-sharedhandle)

## TW-D-13: a failed probe

Raised by the replan of 2026-10-15, which left "what a probe that fails does" as a question for the
engineer. The answer was the two classes and the single predicate, with `ERROR_INVALID_HANDLE` as the
one example of terminal. Recording it raised three points, which the engineer then settled:

- **A diagnostic sink.** This crate had none; it was first written down here as there being none
  anywhere in the repository, which was wrong: durable-ioring has a `DiagnosticSink` (DI-D-51). Its
  events and kinds are its own, so this crate mirrors the shape instead of using it. Putting a
  watcher event in durable-ioring was the alternative, and the wrong layering; factoring the
  machinery into a crate both use was the other, and was set aside for now because it would change
  two crates for one event. The merge-or-delete decision is tracked as TW-2+.1.
- **Telling the caller of a terminal entry.** Left alone, a file dropped from probing looks to the
  caller like a file that stopped changing, and the Win32 code would be lost. An accessor on
  `Registration` would keep the code but only for a caller that polls it. The engineer chose a
  dedicated SQ entry. Only terminal failures are sent that way; transient ones are diagnostics.
- **Log volume.** A failed volume gives one transient message per entry per beat. The engineer's
  answer was not to aggregate; with the default sink wanting nothing, the cost when nobody listens is
  a branch.

Defaulting an unlisted code to transient was chosen as the failure that keeps watching.
`ERROR_FILE_INVALID` was added to the terminal set by the assistant and accepted.

## TW-D-14

- **Why the choices are recorded without a table.** The figures live in the two captures, with host
  and commit; copying them here would make a second copy that nobody re-checks. The decision names
  the captures and what each measured.
- **Why sharding is a remedy and not a plan.** The engineer: with very little effort the probe of a
  20,000-file roster could be brought much lower by threading it, so no normal update frequency
  should miss its deadline, and it need not be built now, only known to be available. What it
  would take (a changed-list buffer per thread, a merge in roster order, a fan-out in the poll on
  the thread pool) is why it is not a constant to change.
- **Why the earlier checklist item to choose them was closed rather than kept.** It was queued by the
  assistant so that the decision would not live only in prose, and the decision was made when the
  engineer read the captures.

## TW-D-11 and TW-D-12

The first sketch named layers (endpoint, cursor, registry, reader); the engineer pointed out that
the components and their names are an output of the design process, so the sketch's labels are
placeholders and the question asking for the full component graph was withdrawn. The experiment
harness was discarded because only Rust fixtures are kept.
[Q2](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q2-and-a-layer-sketch),
[Q8](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q8-the-harness-is-discarded)

## TW-D-15 and the 2026-10-16 amendments to TW-D-10 and TW-D-11

The engineer proposed the shape on 2026-10-16, replacing the reads through `ThreadpoolIo` planned as
TW-4+.1 and TW-4+.2: an I/O ring, buffers from a pool associated with it, a layer above the watcher,
and an SQ of data chunks for the client, with a CQ only if one is needed. The assistant laid out what
the repository already had -- reads in windows-ioring-sys, the same `SharedHandle` type on both
sides, thread-pool completion delivery, and no buffer pool anywhere -- and put three questions.

- **Who issues the reads.** The engineer left it to the assistant ("you decide!"). Four ways were
  considered:
  - *Hold the endpoint as an `EndpointHandle`, address the handle raw, keep an observer in each
    read's sidecar* -- chosen. It uses only what both lower crates already publish: the
    `EndpointHandle` is the existing route from an endpoint into a backend, and the ring is that
    backend. The observer keeps the handle open until the completion pops, which is the raw
    address's whole contract, so the `unsafe` is local and its argument is one sentence.
  - *A `FileTarget` for `EndpointHandle` in windows-ioring-sys.* Safe at the call site, but it makes
    the ring crate depend on windows-overlapped-io-sys, two crates kept apart on purpose
    (duplicate-then-decide in the ring crate's notes). Set aside as re-coupling them for one consumer.
  - *Expose the endpoint's `SharedHandle` from windows-overlapped-io-sys.* Its clones could become
    second endpoints, which is exactly what that crate's observer decision made structural to avoid.
  - *A second handle per file for the ring.* Twenty thousand more kernel objects at the top end, and
    the engineer had already ruled the same trade the other way for probes: "a small user mode
    allocation beats a second kernel mode allocation".
- **Owned or registered buffers.** The engineer chose owned. Registered buffers were the more
  natural reading of "associated with the ring", but lending one registered buffer to the client
  while the ring fills others is something windows-ioring-sys does not offer, so they would have
  started with a change to that crate.
- **Order of chunks within a file.** In order would hold completed later chunks, and their buffers,
  until earlier ones arrive. The engineer chose delivery as reads complete and left reordering to the
  consumer, which the range on every chunk allows.
- **Whether a CQ is needed.** Only the end of a chunk's use flows back, and ownership already says it:
  dropping the chunk returns the buffer. A CQ would have made the client post what the drop does.

### How the engine was built (TW-5.5, decided by the assistant while implementing)

Choices the engineer was not asked about, because the plan did not reach them. Each is a working
choice open to the engineer's correction; the decision text they sit under is
[TW-D-15](DESIGN-NOTES.md#tw-d-15).

- **How a read finds its file.** The planner is given an attention, which carries the caller's `T`
  and two lengths and nothing that names a file. Considered:
  - *Add the registration's id to the attention and look the file up in the roster when the read is
    planned.* Rejected: it changes a public type the watcher's own clients read, touches every test
    that compares an attention whole, and takes the roster lock once per attention. It also loses the
    file when the registration is dropped between the write and the lookup, which TW-D-10's amendment
    says must not stop a read.
  - *A value the layer builds before the file is open, completed afterwards.* Rejected: an attention
    can be written between the two, so a read could find the value empty.
  - *Build the value from the open file, under the watcher's roster lock, as the registration is
    made* -- chosen. It needed one crate-internal registration that hands the file and the id to the
    caller's closure, and nothing else outside the layer. The cost is that the layer is generic over a
    wrapped value and its clients register through it.
- **The window on a lent buffer.** A ring read fills the whole buffer. Reading a full buffer and
  keeping the planned part was considered and rejected: it reads bytes the next poll announces again,
  and it reads them for nothing.
- **Poll-done ordering.** A counter per poll was chosen over one global count of reads in flight: a
  global count makes poll-done wait for the reads of a poll that started after it.
- **A full client SQ stops popping.** Popping completions into a queue the client has not drained
  would hold every buffer there and leave the pool empty for nothing; the ring holds them as well and
  its queue size already bounds how many.
- **Zero bytes delivers nothing.** An empty chunk says nothing a poll-done does not, and every client
  would have to skip it.
- **A failed read is classified as a failed probe is.** The engineer decided it, 2026-10-16:
  terminal ones appear in the SQ and transient ones go to the diagnostic channel only. The assistant's
  first reading, a diagnostic for every failure, left a client waiting on bytes from a handle that
  would never answer.

### The layer on the thread pool (TW-6.1, decided by the assistant while implementing)

- **One wait per event, not one for all.** A thread-pool wait watches one handle, and the four
  events are four handles; waiting on any of several would need a thread of the layer's own, which
  is what the pool is here to avoid.
- **Waits stay armed.** The checklist said "nothing armed between polls". The ring's completion
  event cannot be detached from the ring, so one wait is armed for the ring's life whatever is
  decided, and the other three follow it: arming one from another's callback, to arm only what a
  stop needs, is a cross-thread arm that draining does not exclude, so a drop racing a callback could
  leave a wait armed behind it. Each wait re-arming only itself has no such race, and an armed wait on
  an event nobody rings is exactly the idle state [TW-D-5](DESIGN-NOTES.md#tw-d-5) calls outside its
  requirement.
- **The run resets, the wait does not.** Putting the reset in the engine's run rather than in each
  callback gives every trigger the same discipline, and it is also how a synchronous driver gets it.

### Shutdown and drop with reads in flight (TW-7.1, decided by the assistant while implementing)

- **Complete, not cancel.** The checklist allowed either. Cancelling needs a raw cancel on the ring,
  which only an `Open` ring has, and the engine's ring is `Owned` so that every operation it holds
  has a buffer to hand back. Making it `Open` for a bound that a read of a file already has would
  trade that guarantee away.
- **A stop flag and a handle, not the lock.** A shutdown that took the engine's lock would wait for a
  callback's step, which TW-D-9 forbids of a request; the flag and the watcher's stop handle are set
  with no lock.
- **Poll-done survives shutdown.** Abandoning only reads, and letting every other entry through, is
  what keeps the group accounting (a poll's poll-done waits for its reads) true after a stop with no
  special case.

## TW-D-16

The engineer asked whether a client could choose the ring sizes, the buffer size and the buffer
count. The assistant found that the internal constructors take all of them, and that none of the
layer is public, so no client outside the crate could. The engineer then said the public layer is not
a new crate: "I would put this in a distinct new module within the crate".

- **A new crate was not considered further.** The layer owns a `Watcher` and uses crate-internal
  parts of it (`register_with`, `mark_terminal`, the stop flag). A second crate would need each of
  those made public for one consumer, which is a larger public surface than the layer itself.
- **Defaults.** The assistant first planned none, because a number nothing had chosen is why
  `Watcher::new` takes its capacity, and parked them behind a measurement. The engineer replied that
  measurement would not be very useful since the best sizes are workload dependent, and gave a
  hedged pick: 64 buffers of 64 bytes and a queue of 64. That is recorded as a working position and
  scheduled as part of TW-9.2, with no measurement gating it.
- **Refusal before allocation.** The pool allocates and zeroes every buffer when it is made, so an
  invalid size found late would have paid for the allocation. The ring's own rules for its queue
  sizes are not known from reading the code, which is why the first item measures them.
- **Names left open.** The assistant proposed `TailReader` and `TailReaderOptions` for the types and
  `reader` for the module, and asked whether "the layer's name" meant the types. It does: the module
  and type names, since no crate name is in question.
- **The worst-case load.** The assistant measured three raw runner logs of this repository when
  the engineer said to plan for output with the cadence of a GitHub runner's job log, and the engineer
  confirmed the literal reading: the real work is virtual machine deployment, the log is a good
  benchmark of it, and it is the worst case load. The figures are in the capture, not here. What they change in the plan is TW-9.5's
  workload, and they bear on the buffer-size default without settling it: the 64-byte default was the
  engineer's hedged pick for a lighter typical load, so a worst case that stresses it is a test of
  correctness, not a ruling against it.
- **The near-term load and the design center.** The assistant read three Microsoft Learn pages and
  measured the sample blocks they show, as the engineer had suggested. The engineer then said not to
  fixate: the real figure is 0 to about 100 KB over 0 seconds to about ten minutes, with long
  operations returning a token that is polled, and a build service is a design center that sets a
  floor. The capture was kept, marked as documentation and not a run. The Azure CLI is installed on
  the development machine, but a deployment would create cloud resources, so none was run.
- **The ring's queue sizes were measured, not reasoned.** The assistant had inferred from two
  observations that the kernel doubles the completion queue it is asked for, and wrote that down as a
  working position with a note that TW-9.1 would confirm or replace it. A probe over a grid of sizes
  replaced it: the completion queue is the larger of twice the submission queue and the request
  rounded up. A request of zero is the finding that matters to the options type, because the ring
  accepts it and picks a default, so a caller who passes zero gets a size they did not choose.
- **Sizes the ring would raise are refused too.** The assistant put the choice after TW-9.1 showed
  the ring rounds a request up: accept it and let the ring round, or refuse it. It noted that
  accepting is harmless, since the caller gets at least what they asked for, but hides the real
  size. The engineer chose to refuse non-powers of two and asked that it be documented. The same
  measurement showed the rule alone does not make the caller's number the ring's: a power of two
  below the ring's floor, or a completion queue below twice the submission queue, is still raised,
  and the assistant's own first default of 64 and 64 was one such case. The engineer said to refuse
  those too, so the default became 64 and 128.
- **A queue the allocator refuses.** TW-9.5 found that the options accept a client queue capacity the
  queue crate also accepts and the process aborts allocating it. The engineer asked whether the fix
  belonged in `windows-waitable-queues` and, once it was released, chose a new `CreateError` variant
  from three offered: a variant, a cause field on `SqCapacityError`, or an `Io` mapping with
  `OutOfMemory`. The assistant had recommended the variant. The attribute `#[non_exhaustive]` on
  `CreateError` was added by the assistant with the change and flagged in the same message.
- **The error carries its cause.** The engineer asked that the error enum have a cause. The assistant
  read that as each variant naming the rule it broke and a lower layer's error being its `source`,
  and said so in the notes in case another reading was meant.