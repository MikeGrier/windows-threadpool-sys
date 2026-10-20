# win-multi-file-tail-watcher: open design questions

_Written 2026-10-13 22:58 -04:00 - anchored to commit `57471c8f`._

A worksheet for the questions raised in
[DESIGN-SESSION-2026-10-13-tail-watcher-inception.md](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md).
The session file holds the context and reasoning; this file holds the questions and your answers.
Numbering matches the session. "Too early to say" is a valid answer to any of them, and an answer
recorded here is a working position until it is promoted to a decision in a DESIGN-NOTES.md.

**Promoted 2026-10-14 to [DESIGN-NOTES.md](DESIGN-NOTES.md), which is now authoritative.** This
worksheet is kept as the record of how each answer was given.

Grouped so that earlier groups tend to unblock later ones.

The layer labels used below (L0 endpoint, L1 cursor, L2 registry and beat, L3 reader) are
placeholders from a first sketch in the session file, not chosen names. Naming is still open.

## Answered

- **Q2. Who reads the bytes?** Layered: each layer answers it for itself, and every layer is usable
  without the ones above it. The held handle is opened overlapped, so reads can complete on the
  thread pool. By iteration: in the first, only test code reads the bytes; in the next, probably
  asynchronous reads on the Windows thread pool; later iterations may build further.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q2-and-a-layer-sketch),
  [elaboration](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#elaboration-of-q2-and-q3-iterations-and-who-owns-the-beat)
- **Q3. Who advances the consumed offset?** Two signals: the watcher signals *available*, the
  consumer signals *consumed*. Per Q27, *consumed* stays on the caller's side; the watcher is not
  told it. The caller owns the beat: it runs the timer and decides when to call
  in to poll. The crate may use several threads to finish a poll quickly, but between polls its
  resource use must be zero.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q3-and-whether-this-is-ring-shaped),
  [elaboration](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#elaboration-of-q2-and-q3-iterations-and-who-owns-the-beat)
- **Q4. How are files registered?** By path, as `WtfString`, with a starting offset.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#engineers-answer-to-q4)
- **Q13. Path encoding.** `Wtf16String` only.
- **Q15. A path that does not exist yet.** The path must exist at registration, for now. A failed
  open surfaces the Win32 error code.
- **Q5. Shrink and replacement.** Nothing special. The crate watches the handle; when the length
  changes, in either direction, it reports the new length. The receiver decides what a change
  means. Replacement is not detected: the held handle keeps following the file it opened.
- **Q14. Starting offset past the current length.** Registration reads the length when it opens
  the file; a starting offset beyond it fails registration with a distinct `Err` value.

  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answers-to-q5-q13-q14-q15)
- **Q19. At most one outstanding attention per file?** Intended, but the design must not depend on
  it. Registration opens and validates the file, then adds it to the list. No uniqueness, no lookup
  by name. Registration returns an RAII value whose `Drop` unregisters.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q19-registration-as-an-raii-value)
- **Q23. How an attention names its file.** The watcher is generic over a caller type `T`. A `T` is
  moved into each registration, and an attention gives the caller access to it (possibly just the
  `T` itself) to identify the registration. Constraints on `T` are whatever the design needs.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q23-a-caller-type-t-per-registration)

## Shape of the layers

- **Q9. Withdrawn as a question.** It asked for the full set of components and the graph of what
  uses what. That is an output of this design process, not something available to answer now. The
  pieces get named and placed as the other answers arrive.

- **Q10. Where the layers live.** Modules of one crate, or does some layer belong in an existing
  crate (L0 in windows-overlapped-io-sys most obviously) or in a crate of its own?

  Answer: one crate. Changes this drives in other crates are acceptable; changes to
  windows-overlapped-io-sys were not anticipated but are open to discussion (see Q30).
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q10-one-crate)

- **Q30. The held handle once reads arrive.** Iteration 1 needs nothing from
  windows-overlapped-io-sys. In the reads iteration, its `UnassociatedEndpoint` owns its handle
  exclusively (an `OwnedHandle`, deliberately not `Clone`), while Q25 holds each handle as a
  `SharedHandle` so probes run outside any lock. Options: (a) the shared ownership wraps the
  endpoint (`Arc` of the endpoint) and the probe uses its borrowed handle, needing no change to
  windows-overlapped-io-sys; (b) the endpoint learns to hold a `SharedHandle`, a change to that
  crate; (c) two handles per file, one for probing and one for reads.

  Answer: (b). windows-overlapped-io-sys needs reworking around the shared handle type. Queued
  there as M16 in its [CHECKLIST.md](../windows-overlapped-io-sys/CHECKLIST.md), investigation first.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q30-the-endpoint-holds-a-sharedhandle)

- **Q11. Withdrawn.** It assumed lengths only grow; per Q5 they need not.

## Signals and lifetime

The intent behind the ring remark: a poll produces one *attention* per file whose length changed,
carrying its *last known* (last reported) and *current* lengths. What the consumer has consumed
stays with the consumer (Q27). The set of attended files has no fixed bound over
time, so the mechanism carrying it must not either. A ring is not required.
[Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#correction-the-requirement-is-dispatch-not-a-ring)

- **Q20. The dispatch mechanism.** A visitor called during the poll, a returned list, an
  intrusive ready list (epoll-style), a completion port, or several of these layered on one
  another? And separately, how the consumed report comes back: through the same mechanism, or a
  direct call naming the file and its new consumed mark. (The payload is settled: last known and
  current length. The consumed report does not come back at all; see Q27.)

  Answer: an IoRing-shaped submission queue, with no CQ (Q32). The crate creates the queue
  (superseding an earlier answer that the caller supplies its storage; the caller presumably sets
  its capacity) and the watcher writes attentions into it (its SQ). The watcher rings a doorbell when the queue is full
  and when it has a batch to hand over.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q20-resolved-reading-b-with-a-doorbell)

- **Q25. A full queue in the middle of a poll.** **Superseded in part by
  [TW-D-7](DESIGN-NOTES.md#tw-d-7), amended 2026-10-15: the kept roster position became an index
  into a saved list of changes, and the lock rule was scoped.** Does the poll wait for the caller to drain, or
  stop and resume on the next call from where it left off? (windows-file-watcher D-29: throttle,
  never block at a full queue.)

  Answer (working position, a suggestion rather than an edict): keep a position in the roster.
  When the SQ fills, record it, ring the doorbell and stop, without waiting. No lock is held across
  the probe, the SQ write or the doorbell; each entry's handle is a `SharedHandle`
  (win-shared-os-owned-handle) so the probe runs on its own reference. When the caller rings back
  that space is free, resume from the same position. Roster changes in between are deferred, with
  a preference to resume at the same index even if an entry is skipped or repeated that beat.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q25-resume-by-roster-index-no-locks-around-io-a-working-position)

- **Q26. What the doorbell is, and who owns it.** An event the caller waits on (the
  windows-waitable-queues shape), a thread-pool work item the watcher submits, or something the
  caller supplies with the queue? (Per Q25 there are two: watcher to caller when the SQ is full or
  a batch is ready, and caller to watcher when space is free.)

  Answer: doorbells are Windows events, created by the crate, manual-reset (less error-prone; an
  auto-reset event can lose a signal, as windows-waitable-queues found).
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q26-doorbells-are-windows-events),
  [creation](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#who-creates-the-queues-and-doorbells)

- **Q32. Is there a CQ?** Since Q27 nothing flows from caller to watcher except the resume
  doorbell. Does "CQ" name something new (for example the caller handing drained slots back), or
  is it a leftover from the earlier ring shape?

  Answer: a leftover. There is no CQ: one crate-created SQ, written by the watcher and drained by
  the caller, plus the two doorbells.

- **Q29. Who waits on the resume doorbell?** The watcher registers a thread-pool wait on it (no
  thread while idle, but a live registration between polls, and resuming runs on a pool thread), or
  the event is only a signal, and resuming still means the caller calling in?

  Answer (working assumption): the watcher registers a thread-pool wait on the resume doorbell and
  resumes the poll from it. Earlier recorded as tentative; now the assumption to build on, open to
  refinement during implementation.

- **Q27. The way back: does the watcher need to hear "consumed"?** Q3 named two signals,
  *available* (watcher to caller) and *consumed* (caller to watcher). Q20 settled how *available*
  travels; nothing yet carries *consumed*. Whether anything must depends on what an attention's
  "last known" length means:
  - **Last reported by the watcher.** The watcher needs nothing back. Consumption lives with the
    caller, who already holds the numbers needed to re-register after a restart. The only thing
    flowing back is the resume doorbell (SQ space is free), and Q21 dissolves.
  - **Last consumed by the caller.** Then the caller must report back per file ("consumed up to N"),
    for example through a second, caller-written queue the watcher drains on its next poll (the CQ
    of the IoRing shape). Q21 (partial consumption) is then real.

  Answer: the first. "Last known" is the last length the watcher reported; nothing flows back
  except the resume doorbell; Q21 dissolves.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q27-last-known-means-last-reported)

- **Q28. One poll at a time?** A single remembered roster position implies one poll in progress
  at a time, with any threads inside it dividing the roster. Is that the rule? It also keeps each
  entry's last known length written by one thread only, outside any lock.

  Answer: one poll at a time; no need for concurrent polls, and the engineer's rule is to take
  whichever is simpler. One at a time is: a single resume position, and each entry's last known
  length has one writer, so it needs no synchronisation. Concurrent polls would need both to be
  coordinated. Registration and unregistration during a poll are a separate matter (Q22).
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q28-one-poll-at-a-time)

- **Q22. Unregistering during a poll.** If the registration value is dropped while a poll (possibly
  on several threads) is visiting that file, does `Drop` wait for the poll to finish with it, or is
  the removal deferred to the poll's end? (Mostly settled by Q25: the poll holds its own
  `SharedHandle`, so `Drop` need not wait for the probe. What remains is the `T`; see Q24.)

  Answer: unregistering during a poll is legal and `Drop` does not wait. No promise is made that
  attentions stop at unregistration: one may already be in the SQ, so the client must tolerate
  attentions for a registration it has dropped. A registration made during a poll may or may not
  be picked up by that poll.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q22-unregistration-during-a-poll-is-legal)

- **Q24. How an attention exposes `T`.** A borrow (`&T`) lives only as long as the registration's
  storage is pinned for the consumer, which suits a visitor called during the poll and not a list
  that outlives it. A list or ready list needs `T: Clone` (a copy per attention) or `T` held behind
  a shared pointer. Which access, and with it which constraints on `T` (`Send`, `Sync` if a poll
  runs on several threads)? This is also what decides the rest of Q23: whether an attention can
  outlive its registration. (Narrowed by Q22: an attention can outlive its registration, so a
  borrow is out; the choice is between a clone and a shared pointer.)

  Answer: `T: Clone`; each attention carries its own clone. Anything else makes the lifetime
  management too hard to work out. (A caller wanting cheap clones can choose an `Arc` as its `T`.)
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answers-to-q24-and-q31)

- **Q21. Withdrawn.** Dissolved by Q27: the watcher is never told what was consumed, so partial
  consumption is the caller's own business.

- **Q12. Superseded by Q20 and Q26.** The signals travel through the caller-supplied queue (Q20)
  with Windows events as doorbells (Q26). The merge-by-key requirement it implied is gone: a full
  queue stops and resumes (Q25), and attentions carry absolute lengths, so a duplicate is harmless.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#q12-superseded)

- **Q6. End of stream.** Is a child's exit an input to the registry (final drain, then retire), or
  purely the caller's business?

  Answer: the caller's business. The watcher knows registrations, not processes; the client
  unregisters (drops the registration value) when it no longer wants size updates. What causes a
  length to change is not the watcher's concern.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q6-the-source-of-length-changes-is-not-the-watchers-business)

## The beat

- **Q17. What counts as resource use between polls.** No threads, timers or wakeups, certainly.
  What about the held handles and their bookkeeping (inherent to the design), memory such as
  read buffers, and, from the reader iteration on, overlapped reads still outstanding?

  Answer: thread wakeups. Between polls the process should quiesce as far as possible: the crate
  causes no thread to wake. Held handles, memory and other state that sits still are not the
  concern.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q17-the-measure-is-thread-wakeups)

- **Q18. Shape of a poll call.** Does `poll` return only when the poll is finished (fanning out
  internally, joining before it returns), or does it start the poll and signal completion later?
  The first makes "zero between polls" true by construction; the second lets the caller's thread
  go, at the cost of work outliving the call.

  Answer: the second, definitely. `poll` initiates the poll and returns; completion is notified
  later by a poll-done SQE (Q31). The engineer is against synchronous APIs. Only the finer details
  of the call are left to implementation.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q18-leaning-asynchronous)

- **Q31. How completion of a poll is signalled.** A distinct "poll complete" doorbell, the existing
  batch doorbell with a completion marker the caller reads from the SQ, or something else? Bound
  up with Q26 (who owns the events) and Q29 (who waits on what).

  Answer: a distinguished SQE says the poll is done, and the doorbell is rung so the caller attends
  to the SQ. No separate completion doorbell.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answers-to-q24-and-q31)

- **Q33. Can processing be cancelled, and what does cancelling mean?** Answer: yes; cancellation
  is shutdown of the watcher, not of one poll. No use was found for cancelling a single iteration
  and polling again, so it is not offered (not foreclosed). A parked poll is stopped by
  `stop_and_drain` on its resume wait with a stop flag set first; a running poll stops at its next
  roster entry.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#cancellation-means-shutdown)

- **Q34. The shape of shutdown.** Dropping the watcher must block until it is quiescent, since
  pool callbacks reference its state. Is `Drop` the only way to shut down, or is there also an
  explicit asynchronous shutdown that signals when quiescent, leaving `Drop` nothing to wait for?

  Answer: an explicit, non-blocking shutdown request, typically made from the caller's own
  notification callback. The watcher stops the poll and signals that polling has ended; the caller
  then drops the watcher, and `Drop` has nothing left to wait for. `Drop` without a prior shutdown
  remains safe by blocking, as a fallback. The request is idempotent; `poll` after it is refused.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q34-shutdown-is-requested-signalled-then-dropped)

  **Amended 2026-10-15** in [TW-D-9](DESIGN-NOTES.md#tw-d-9): the request sets the stop flag and
  returns without draining, and `Drop` drains, waiting at most for a running poll to reach its next
  roster entry. "`Drop` has nothing left to wait for" did not hold: draining inside the request
  could block, and the Q35 answer lets the caller drop without reading the poll-done entry.

- **Q35. Signalling "polling has ended" when the SQ is full.** Shutdown is often requested while
  the poll is parked on a full SQ, so a terminal SQ entry may not fit. A reserved slot only a
  terminal entry may use, a terminal state read beside the SQ when the doorbell rings, or a
  dedicated shutdown event?

  Answer: none; no special mechanism. The terminal entry goes at the end of the SQ like any other
  and waits for space like any other. A caller not interested in it need not drain for it: having
  requested shutdown, it may simply drop the watcher.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q35-no-special-mechanism)

- **Q36. Signalling quiescence after a shutdown request.** Raised 2026-10-15 by the review
  that corrected [TW-D-9](DESIGN-NOTES.md#tw-d-9). After the request, `Drop` waits at most for a
  running poll to reach its next roster entry and return; even after the poll-done entry is read,
  the callback that wrote it may not have returned. Should the crate offer a way to know it is
  quiescent -- so a caller can drop knowing `Drop` will not wait -- or is the bounded wait enough?

  > Drop has to be allowed to block.

  Answer: the bounded wait is enough. `Drop` may block; no quiescence signal is added. Recorded in
  [TW-D-9](DESIGN-NOTES.md#tw-d-9).

## Population

- **Q7. Scale of the beat.** Hundreds of files or tens of thousands? (Partly answered under Q3:
  the crate may use several threads within a poll. Still open: how many, and who supplies them --
  the thread pool, or threads the caller lends for the duration of the call.)

  Answer (file count, a guesstimate): about 20,000 registered files at the top end, i.e. some
  10,000 running jobs each with stdout and stderr, each watched by remote clients. Considered an
  extreme.

  Answer (threads, a principle with the specifics deferred): use the fewest threads that finish a
  poll within a reasonable beat interval. Too much is unknown to say more: querying 20,000 handles
  may be trivially cheap on one thread, or the querying itself may need sharding as the count
  grows. The design should leave sharding possible without committing to it.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q7-about-20000-files-at-the-top-end)

## Premises

- **Q1. The notification side effect.** On NTFS a path-based length query provokes a directory
  change notification; a held-handle query does not. Does anything depend on those notifications?

  Answer: not at this time; no use is foreseen. The consumers are off-machine listeners who want a
  reasonably current view of each child's stdout/stderr, and the aim is to serve them more
  efficiently than one timer per file. Provoking notifications gains nothing toward that.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q1-and-the-goal-behind-the-crate)

- **Q16. What "buffered" means in the brief.** The directory-entry lag only, or also output held in
  the child's own stdio buffer, which no reader-side technique can see?

  Answer: only from the operating system's point of view. The crate has no insight into or control
  over the child process; it sees exactly the bytes that have reached the file system.
  [Session](design-sessions/DESIGN-SESSION-2026-10-13-tail-watcher-inception.md#answer-to-q16-buffered-means-the-os-view-only)

- **Q8. Keep the experiment harness?** Turn it into a probe in windows-platform-probes so the
  findings can be re-run on other hosts and file systems? (A comparison against per-file timers is
  out of scope for this crate; that evidence belongs to the component that chooses between them.)

  Answer: no. The harness was PowerShell, and only Rust fixtures are kept; it has been deleted.
  The session record's tables remain the record of what it found.
