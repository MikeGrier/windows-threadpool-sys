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

## TW-D-7: a full SQ stops the poll; no locks around I/O

Waiting at a full queue was set aside in line with
[windows-file-watcher's D-29](../windows-file-watcher/DESIGN-NOTES.md#d-29) (throttle, never block
at a full queue). The engineer proposed the stop-and-resume shape and the no-locks-around-I/O rule,
with shared ownership of the handle as the means of probing outside a lock (first a bare `SharedHandle`,
later an observer; see TW-D-10).
[Q25](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q25-resume-by-roster-index-no-locks-around-io-a-working-position),
[Q29](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#q18-q31-and-q29-firmed-up)

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

## TW-D-10: one crate, shared handles

Three ways to reconcile the roster's `SharedHandle` with windows-overlapped-io-sys' exclusively
owned endpoint were laid out: share the endpoint behind an `Arc` (no change elsewhere), teach the
endpoint to hold a `SharedHandle` (a change to that crate), or open two handles per file. The
engineer chose the second and asked for the owned-versus-shared implications to be investigated up
front rather than when reads arrive.

That investigation (windows-overlapped-io-sys M15.1) found that exclusivity protects "one endpoint
per handle", not "one holder per handle", and that an endpoint generic over owned or shared storage
would need an `unsafe` constructor whose `SharedHandle` clones could become second endpoints. The
engineer asked for a structural remedy instead and chose *observers*: shared holders that can query
but implement no handle-lending trait, so they can never reach an endpoint constructor. A second,
non-overlapped handle per file was again set aside: "a small user mode allocation beats a second
kernel mode allocation". Using observers from the first probe made M15 a prerequisite of iteration 1.
[windows-overlapped-io-sys DESIGN-NOTES.md](../windows-overlapped-io-sys/DESIGN-NOTES.md)
[Q10](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q10-one-crate),
[Q30](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q30-the-endpoint-holds-a-sharedhandle)

## TW-D-11 and TW-D-12

The first sketch named layers (endpoint, cursor, registry, reader); the engineer pointed out that
the components and their names are an output of the design process, so the sketch's labels are
placeholders and the question asking for the full component graph was withdrawn. The experiment
harness was discarded because only Rust fixtures are kept.
[Q2](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q2-and-a-layer-sketch),
[Q8](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q8-the-harness-is-discarded)
