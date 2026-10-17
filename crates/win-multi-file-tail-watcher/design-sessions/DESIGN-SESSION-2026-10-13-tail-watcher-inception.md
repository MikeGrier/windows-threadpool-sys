# Design session 2026-10-13: win-multi-file-tail-watcher inception

_Written 2026-10-13 22:43 -04:00 - anchored to commit `57471c8f`._

**Status: concluded 2026-10-14.** The answers were promoted to decisions TW-D-1 to TW-D-12 in
[DESIGN-NOTES.md](../DESIGN-NOTES.md), which is authoritative. What follows is the record as it
unfolded: the brief as the engineer stated it, the experiments run to re-verify its premises, and
the questions and answers in order.

## The brief

A parent process launches many child worker processes. Each child's stdout and stderr are
redirected to files (the arrangement found most robust if the parent dies; compare
[win-job-launcher](../../win-job-launcher/src/launch.rs), which already creates such files). The
parent reflects those streams by watching the files grow.

Directory change notifications give an unsatisfying cadence: they fire when the directory entry is
updated, which happens on a long interval rather than per write. The proposed remedy is to hold a
`GENERIC_READ` handle on each file and query its current length on a cadence.

The prior service implementation used one timer per tailed file, which does not scale. This crate
is to be a **registry** of tailed files, each with an open read handle, where the **caller owns the
beat**: on each beat the caller asks for a poll across the population and gets back the set of
files with new bytes and the byte region newly available in each.

## Experiments (re-verifying the premises)

Harness: a child `pwsh` writes a 69-byte line every 200 ms to stdout, redirected by `cmd /c ... >
file` (so the child holds the file handle; `Console.Out` autoflushes, so every line is one
`WriteFile`). The observer samples every 250 ms for ~26 s in one of three modes, and runs a
`FileSystemWatcher` (`Size | LastWrite`) on the directory throughout:

- **enum** - directory enumeration only: the size recorded in the directory entry.
- **path** - enumeration plus `FileInfo(path).Length`, i.e. `GetFileAttributesEx` by path, which
  opens, queries and closes the file each time.
- **handle** - enumeration plus `FileStream.Length` on a handle held open for the whole run
  (`GetFileInformationByHandleEx(FileStandardInfo)`), opened read-only with share
  read/write/delete.

Host: Windows build 26200. `Q:` is ReFS, `C:` is NTFS.

| Volume | Mode | Queried size tracks writes | Directory-entry size | Notifications |
|---|---|---|---|---|
| ReFS | enum | n/a | 2 updates mid-run, then at child exit | 4, bunched at ~12.8 s and exit |
| ReFS | path | yes, every sample | 2 updates mid-run, then at exit | 5, bunched at ~16.3 s and exit |
| ReFS | handle | yes, every sample | 2 updates mid-run, then at exit | 5, bunched at ~19.5 s and exit |
| NTFS | enum | n/a | 1 update mid-run, then at exit | 2 |
| NTFS | path | yes, every sample | **every sample** | **80, one per sample** |
| NTFS | handle | yes, every sample | 1 update mid-run, then at exit | 2 |

What this shows, against the brief:

1. **Directory-entry size and notifications lag by many seconds on both file systems.** Confirmed.
   Mid-run updates arrived 12-19 s in, with no regular period visible in one 20 s run each.
2. **A held handle reports the true current length on both file systems.** Confirmed.
3. **"Querying the length also triggers the change notification" is true only for the path-based
   query, and only on NTFS.** On NTFS each path-based query refreshed the directory entry and
   produced one notification. The held-handle query did neither, on either file system, and on ReFS
   the path-based query did not either. The likely mechanism is that NTFS refreshes the directory
   entry's duplicated size when a handle is closed, and the path-based query closes one; this is a
   hypothesis, not measured.
4. **Path-based queries also report the current length** on both file systems, without a held
   handle. They cost an open/close per file per beat, and on NTFS a notification storm with a
   directory-entry write per file per beat.

Not tested, and worth saying: data still sitting in the child's own user-mode buffer (for example
C runtime stdio, which is fully buffered when redirected to a file) never reaches the file
system, so no reader-side technique can see it. The harness child flushed every line on purpose.
Whether "buffered" in the brief means this, or only the directory-entry lag, is an open question.

The harness was scratch PowerShell and is not committed; see the questions below.

## Open questions

Too early to say is an acceptable answer to any of these.

- **Q1. Is the side effect in finding 3 wanted?** Something else may be relying on NTFS
  notifications that the path-based query provokes. A held handle does not provoke them.
- **Q2. Who reads the bytes?** The crate reports regions only, or it also reads them (and if so,
  synchronously, or through [windows-overlapped-io-sys](../../windows-overlapped-io-sys/) /
  [windows-ioring-sys](../../windows-ioring-sys/)).
- **Q3. Who advances the consumed offset?** The crate advances it on each report, so regions are
  disjoint and contiguous, or the caller commits consumption explicitly.
- **Q4. Registration source.** By path (the crate opens), by a handle the caller supplies (for
  example duplicated from the launcher), or both. Should registration accept a starting offset,
  so a restarted parent resumes where it left off?
- **Q5. Shrink and replacement.** What should a length below the consumed offset mean (truncation),
  and does the crate need to notice a path now naming a different file? A held handle keeps
  following the original file.
- **Q6. End of stream.** Is a child's exit an input to the registry (final drain, then retire), or
  purely the caller's business?
- **Q7. Scale of the beat.** A poll is one query per registered file. Is the population in the
  hundreds or the tens of thousands, and should the crate partition a poll across threads, or
  leave that to the caller?
- **Q8. Keep the harness?** It could become a probe in
  [windows-platform-probes](../../windows-platform-probes/) so the findings above can be re-run on
  other hosts and file systems, rather than living only in this record.

## Engineer's answer to Q2, and a layer sketch

> The handle should be opened so that overlapped i/o can be queued to be completed via the
> threadpool. [...] the answer to these points come in layers, I'd like to make the layers
> available to construct more library concepts as we go forward

So Q2-Q6 are not answered once for the crate. Each is answered by the layer at which it arises,
and each layer is usable without the ones above it. One thing is settled by the answer: the held
handle is opened `FILE_FLAG_OVERLAPPED`, so it can be bound to the thread pool for reads.

### Working position on the layers (not a decision)

A first cut, bottom-up, for the engineer to correct. Names are placeholders.

- **L0 - tailed endpoint.** One file: open by path (read-only, share read/write/delete,
  overlapped) or adopt a caller-supplied handle; ask its current length through the handle. No
  offsets, no policy. Possibly not new code at all:
  [windows-overlapped-io-sys](../../windows-overlapped-io-sys/) already owns overlapped endpoints
  (`UnassociatedEndpoint::open`) and its `fs` adapter does file reads.
- **L1 - cursor.** Per-file position state: compare a length against a cursor and yield the new
  region, or a shrink. Q3 (who advances) and Q5 (truncation) live here. Pure state, so it can be
  tested without a file system.
- **L2 - registry and beat.** A population of keyed endpoint + cursor pairs; a caller-driven
  `poll` returns the keys with new regions. Reads nothing. Q4 (registration source, starting
  offset) and Q7 (partitioning a poll) live here.
- **L3 - reader.** Turns reported regions into overlapped reads completed on the thread pool
  ([windows-threadpool-sys](../../windows-threadpool-sys/) `ThreadpoolIo`) and delivers the
  bytes. Q2's "who reads" is this layer existing.
- **Above L3, unscheduled.** Candidates only: a beat source (a thread-pool timer driving `poll`),
  line framing, pairing a child's stdout/stderr with its lifetime (Q6, and the
  [win-job-launcher](../../win-job-launcher/) side).

Open, and the next things worth deciding:

- **Q9.** Is the bottom-up order above the right shape, or does a layer split or merge?
- **Q10.** Do the layers live as modules of one crate, or does some layer belong in an existing
  crate (L0 in windows-overlapped-io-sys most obviously) or in a crate of its own?

## Engineer's answer to Q3, and whether this is ring-shaped

> When the "new data available" signal becomes available, there should be able to be a "new data
> consumed" signal that is also made. I wonder if this is all ring-shaped.

So Q3 has two signals rather than one owner: the watcher signals *available*, the consumer
signals *consumed*. The ring question is recorded below as analysis, not as a decision.

### Where the shape is a ring, and where it is not

**Per file, it is a chain of monotonic cursors.** Each cursor is advanced by one party and read by
the next, and never passes the one before it:

    written (file length; advanced by the child, seen only by polling)
      >= announced  (the "available" signal)
      >= consumed   (the "consumed" signal)

With an L3 reader in between, `announced >= read issued >= read completed >= consumed`. A publish
index and a release index, each advanced by one side and read by the other, is the index protocol
of a ring. The two signals in the answer are that ring's publish and release.

**What is missing is capacity.** The file is an append-only log: nothing wraps, and the child never
waits for `consumed`, so no backpressure reaches the producer. Per file this is a ring without a
bound: two cursors over an unbounded log.

**The bound appears where memory is reused, which is L3.** Bytes read into a fixed set of buffers
form a true byte ring: `consumed` frees space, and free space limits how far reads may be issued
past `announced`. So `consumed` does real work at L3. Below L3 it is bookkeeping, plus one more
use: the consumed offset is the natural thing to persist so a restarted parent can resume (Q4).

**Across the population, the signals themselves are two queues.** `poll` produces available
records and the consumer returns consumed records: a completion-queue / submission-queue pair, the
same shape as the I/O ring in [windows-ioring-sys](../../windows-ioring-sys/). One property is
specific to this case. An available record carries a cursor value, and newer values supersede older
ones, so a full outbound queue can **merge** records for the same file (keep the larger) rather
than block or drop them. Bounded queues whose readiness is a waitable handle already exist in
[windows-waitable-queues](../../windows-waitable-queues/), but none of its shapes coalesces by key
today.

**One more ring sits upstream and stays out of reach:** the child's own stdio buffer, discussed
under the experiments.

Open:

- **Q11.** Is the monotonic-cursor chain a primitive the layers should share (one definition of
  "cursor N never passes cursor N-1", used by L1, L3 and the population queues), or is "ring-shaped"
  a description rather than a type?
- **Q12.** Should the available/consumed signals travel through waitable queues, which would mean a
  coalescing-by-key shape, or through a structure owned by the registry, with only its readiness
  exposed as a waitable handle?

## Engineer's answer to Q4

> how are files registered: by path. The files come in as WtfString strings and yes with a
> starting offset

Settled by the answer:

- **Registration is by path.** The crate opens the file itself, so the open policy is the crate's:
  read-only, share read/write/delete, overlapped. Adopting a caller's handle is not part of the
  brief.
- **Paths arrive as `WtfString`** from [wtf-string](../../wtf-string/), so a path that is not
  valid Unicode (an unpaired surrogate, which Windows file names permit) registers without loss.
- **Registration takes a starting offset.** Together with Q3, this completes the restart story: a
  parent that persists `(path, consumed)` can re-register after a crash and resume at `consumed`.

What the answer leaves open:

- **Q13. Which encoding.** `WtfString` is generic over its encoding (`Wtf8String`,
  `Wtf16String`). Opening needs UTF-16, so `Wtf16String` passes straight through; `Wtf8String`
  costs a conversion per registration. Accept one, or both?
- **Q14. A starting offset past the current length.** The file may have been truncated or replaced
  while the parent was down. This is Q5 (shrink) arising at registration, so it probably takes the
  same answer as Q5, but it is reachable on the very first poll.
- **Q15. A path that does not exist yet.** Can a file be registered before its child creates it,
  with the open retried on later beats, or is that an error at registration?

## Elaboration of Q2 and Q3: iterations, and who owns the beat

> In our first iteration, only test code may read the bytes. In our next iteration, we will
> probably write code that performs asynchronous reads using the windows threadpool [...]
> subsequent iterations may build further.

> In an inversion of the expected control mechanism [...] I want to ensure that our caller is the
> one managing the timer and when it chooses to call into us to initiate the polling. We may use
> multiple threads if necessary to get the work done rapidly but managing the cadence and ensuring
> that our resource utilization between polls is zero is critical.

Settled by the answer:

- **Iteration 1 has no reader.** L0-L2 ship with nothing above them; the bytes in a reported
  region are read only by tests. L3 (thread-pool reads) is the probable second iteration.
- **The caller owns the beat.** The crate runs no timer and never wakes on its own. A poll happens
  because the caller called in, which also means the *available* signal can simply be what that
  call yields. Whether it should be is Q12.
- **Zero resource use between polls is a requirement, not an optimization.** The crate may fan a
  poll out across threads, but nothing it starts may outlive the beat it belongs to. The Windows
  thread pool suits this: an idle pool holds no threads, as
  [windows-threadpool-sys](../../windows-threadpool-sys/) notes.

This retires the "beat source" candidate listed above L3 as something this crate provides. A
caller-side helper that drives `poll` from a thread-pool timer could still exist, but it would be
the caller's code by construction.

New questions:

- **Q17.** What counts as resource use between polls? Held handles and their bookkeeping are
  inherent to the design. Memory such as read buffers, and, from L3 on, overlapped reads still
  outstanding, are not obviously either way.
- **Q18.** Does `poll` return only when the poll is finished (fan out, join, return), or start it
  and signal completion? The first makes "zero between polls" true by construction; the second
  frees the caller's thread at the cost of work outliving the call.

## Clarification of the ring remark

> when I was mentioning the ring, it was more about how the found files that needed attention and
> how advancing their high water mark for reading might look

So the ring is at the population level, not inside one file. The section above reads it mainly
per file (the cursor chain); its "two queues" paragraph is the part that matches the intent.

### Working position: an attention ring (not a decision)

- **Outbound:** a poll finds the files whose length has passed their announced high-water mark
  and writes one *attention* record per file into a ring: which file, and the region from the
  announced mark to the length now observed. The announced mark moves up to that length.
- **Return:** the consumer drains records, reads (in iteration 1, only tests do), and reports
  how far it got. That report advances the file's consumed high-water mark. This is the *consumed*
  signal from Q3.

One property looks worth having, offered for judgement. **If a file may have at most one attention
record outstanding**, a ring with one slot per registered file can never overflow, with no
coalescing and no backpressure. A poll that finds new bytes for a file whose record has not yet
come back either skips it until the next beat, or widens the outstanding record in place. Either
way the region stays contiguous, and the consumer never sees one file twice in one drain.

This also fits the beat requirement: the ring is memory, so between polls it costs nothing to
run. It is the same pairing as the submission/completion queues in
[windows-ioring-sys](../../windows-ioring-sys/), with the direction reversed: here the *library*
fills the ring and the *caller* hands slots back.

## Correction: the requirement is dispatch, not a ring

> but don't use a ring if it doesn't make sense. it's just that the list could be arbitrarily
> sized over time so maybe there's some dispatch mechanism

The requirement underneath the ring remark: the set of files needing attention has no fixed bound
over the life of the process, so whatever carries it to the consumer must not have one either.

A ring is a poor fit for exactly that reason. The one-slot-per-file argument above gives a ring
whose capacity must track the number of registered files, so it must grow on registration, which
fixed-capacity rings do not do. The useful part of that argument is the **at most one outstanding
attention per file** property, and it does not need a ring at all. Any mechanism with it is bounded
at every instant by the population and grows exactly as the population grows.

### Candidate dispatch mechanisms (options, none chosen)

- **Visitor during the poll.** `poll` calls a caller-supplied function once per file with new
  bytes. Nothing is materialized and nothing outlives the call, so "zero between polls" holds by
  construction. The caller decides what dispatch means (handle it inline, queue thread-pool work).
- **Returned list.** `poll` fills a caller-owned growable buffer. Simple; costs one entry per
  attended file per beat; the buffer's lifetime is the caller's business.
- **Intrusive ready list.** Each registered file carries its own link; a poll links files with new
  bytes onto a ready list that the consumer drains. One outstanding attention per file is enforced
  by the link itself (a linked file cannot be linked again), there is no allocation per beat, and
  capacity equals the population by construction. This is the shape of the epoll ready list.
- **Completion port.** A poll posts one completion per attended file to an I/O completion port,
  which is an unbounded, kernel-owned queue that already dispatches to waiting threads and to the
  thread pool. It costs a system call per attended file, and the port's queue holds state between
  polls if the consumer is behind. Raw completion ports are already wrapped in
  [windows-overlapped-io-sys](../../windows-overlapped-io-sys/).

These are not exclusive; per the layering answer, a visitor could be the bottom layer, with a ready
list or a completion port built on it.

## Answers to Q5, Q13, Q14, Q15

> q13: just Wtf16String
>
> Q15: path must exist let's say for now, surface the win32 error code if failure
>
> q5: I'm going to be "close minded" and "dumb" here and say that we are watching the handle and if
> the file length changes, we report the new file length. The receiver of updates can decide what
> to do based on the updated new length. we will not do anything fancy based on the change.
>
> q14: when we open the file we should get its length and if this is the case, fail with a
> distinct Err() value

Settled by the answers:

- **Paths are `Wtf16String`**, passed straight to the open.
- **Registration opens immediately; the path must exist.** A failed open is an error that carries
  the Win32 error code. ("For now": registering ahead of the file remains possible later.)
- **Registration reads the length at open.** A starting offset beyond it is a registration error
  with its own `Err` value, distinct from an open failure.
- **A length change is reported as a length change, in either direction.** The crate attaches no
  meaning to a shrink and does not detect replacement; the held handle follows the file it opened,
  and the receiver interprets what it is told.

What the Q5 answer changes in the sketches above:

- **The observed length is not monotonic.** The cursor chain under "Where the shape is a ring"
  assumed `written` only grows; it does not. What is still monotonic is nothing the crate
  enforces. The crate's own state per file reduces to the last length it reported, and a poll
  attends a file when the observed length differs from it.
- **An attention carries a length, not necessarily a region.** For growth the receiver can derive
  the region from the previous and new lengths; for a shrink there is no region. Whether an
  attention carries the previous length as well as the new one, or the receiver tracks it, is a
  detail of Q20.

## What an attention carries

> yes I guess you were thinking of "byte range" as a formal type, I was just thinking of "last
> known" and "current".

Settled: an attention carries the file's **last known** length and its **current** length. No
range type; the receiver reads growth, shrink or anything else from the pair. Q11 (a shared
monotonic-cursor primitive) is withdrawn, since lengths need not grow.

## Answer to Q19: registration as an RAII value

> q19: that is the intention, but it should not be important. Registration should open the file
> with whatever validation we want to add (e.g. starting point vs. size), and then adding it to the
> registration list. I don't foresee any need for uniquification or lookups by name or anything.
> Registration must give back some RAII instance handle that should unregister on Drop.

Settled:

- **One outstanding attention per file is the intent, not a load-bearing invariant.** Nothing may
  be correct only because it holds.
- **Registration is open, validate, append.** Validation is the Q14 check and whatever joins it.
- **No uniqueness and no lookup by name.** Registering one path twice yields two independent
  registrations, each with its own handle.
- **Registration returns an RAII value; dropping it unregisters.** Presumably it also owns the held
  handle, so dropping it closes the file (on NTFS, the one directory-entry refresh found by the
  experiments).

New questions this raises, since the registration value's lifetime now meets the poll and the
attention:

- **Q22.** A drop during a poll, possibly a multi-threaded one: does `Drop` wait for the poll to be
  done with that file, or is removal deferred to the end of the poll? The answer interacts with
  Q18.
- **Q23.** With no lookup by name, how does an attention tell the consumer which file it concerns?
  A caller value attached at registration and returned in each attention, or a reference tied to
  the registration value? And what becomes of an attention still held after its registration is
  dropped?

## Answer to Q23: a caller type T per registration

> q23: the watcher needs to be generic over <T> where <T> is a consumer type with whatever
> constraints we need such a T is moved into the registration on registration of a file and when
> the attention is available for a file, some f(T) is available that is useful to the caller
> (possibly `f(T t) -> T { t }`) to give some identification of the registration entry

Settled: the watcher is generic over a caller type `T`. Each registration takes a `T` by move, and
an attention gives the caller access to that registration's `T`. The constraints on `T` are
whatever the design needs.

One consequence, recorded as analysis. `f(t) -> t` taken literally moves the `T` out, which can
happen only once, while a file is attended on many beats. So the access is one of:

- **`&T`**, a borrow. It lasts only while the registration's storage is held still for the
  consumer, which fits a visitor called during the poll and does not fit a list that outlives it.
- **A clone**, requiring `T: Clone`, one copy per attention.
- **A shared pointer** to `T`, so an attention can outlive the poll, and its registration, without
  copying.

Which of these applies follows from the dispatch mechanism (Q20), and it is also what settles the
rest of Q23: whether an attention can outlive its registration. A poll spread over several threads
adds `T: Sync` for a borrow, and `Send` wherever a `T` crosses threads. Recorded as Q24.

## Answer to Q20 (interpretation pending)

> q20: i think its a ioring shaped sq/cq mechanism where the caller provides the sq

Recorded as a hedged answer ("I think") and not yet promoted: "the caller provides the SQ" has two
readings, and they lead to different designs.

- **Reading A - the caller submits work.** Submission entries are requests the caller makes: "check
  these registrations" for a beat, and plausibly "consumed up to N" for a registration (the Q3
  consumed signal as a submission). The watcher answers with completion entries, the attentions,
  each carrying its `T` (Q23) and the last known and current lengths. The caller decides how many
  submissions are in flight, so the population can grow without bound while what is outstanding
  stays bounded by the caller's own sizing. That answers the concern the ring was first dropped
  over.
- **Reading B - the caller supplies the queue's storage.** The caller hands the watcher memory it
  owns, and the watcher writes attentions into it. That is closer to the "returned list" option,
  with the caller sizing the buffer.

Under A, the earlier objection to a ring (capacity would have to track the population) no longer
applies, because the ring bounds what is in flight, not what is registered.

## Answer to Q20, resolved: reading B, with a doorbell

**Partly superseded by [who creates the queues and doorbells](#who-creates-the-queues-and-doorbells): the crate, not the caller, creates the SQ.**

> b. and the watcher rings the doorbell when full or they want to submit

Settled: **the caller supplies the queue's storage, and the watcher writes attentions into it.**
From the watcher's side this is its submission queue: the watcher submits, the caller consumes.
The watcher **rings a doorbell** when the queue is full and when it has a batch to hand over
(read here as: at the end of a poll, or whenever it chooses to flush a partial batch).

"Doorbell" already means something specific in this repository, and the meanings are worth reusing
rather than redefining: in [windows-waitable-queues](../../windows-waitable-queues/src/doorbell.rs)
it is a lazily created manual-reset event, level-triggered ("there is something to take"); in
[windows-file-watcher's DESIGN-NOTES.md](../../windows-file-watcher/DESIGN-NOTES.md) (D-25, D-36,
D-41) both queues have crate-owned doorbells and the crate never invokes client code.

Open, raised by the answer:

- **Q25. A full queue in the middle of a poll.** Does the poll wait for the caller to drain, or
  stop and resume on the next call from where it left off in the population? windows-file-watcher
  settled the analogous case as "throttle, never block at a full queue"
  ([D-29](../../windows-file-watcher/DESIGN-NOTES.md#d-29)), because blocking there deadlocks when
  the consumer is the one blocked. Resuming instead means a position in the population survives
  between calls, which is state, though not activity.
- **Q26. What the doorbell is, and who owns it.** An event handle the caller waits on (the
  waitable-queues shape), a thread-pool work item the watcher submits, or something the caller
  supplies together with the queue? Is it one of the repository's existing doorbells?
- **Q27. The way back.** If the caller's queue is the watcher's SQ, is the *consumed* signal (Q3)
  the matching CQ, written by the caller and read by the watcher on the next poll?

## Answer to Q25: resume by roster index, no locks around I/O (a working position)

> First of all my write up here is a suggestion not an edict, but here's what I think. I think that
> you can maintain an index into the roster of registered files and once the SQ is full, keep track
> of the position in the roster and ring the doorbell, with no synchronizaton locked. In fact, the
> filesystem interactions should occur with no synchronization - use the win-shared-os-owned-handle
> to do lifetime management on the handle itself so that the probe can occur outside any lock and
> putting the SQE into the SQ and ringing the doorbell is outside of any lock. Then once the SQ is
> freed up and the doorbell to resume filling it is rung back to this component, resume polling from
> the same index. If there are registry changes in the interval between, we can decide later what to
> do. My choice is to ignore and pick up from the same index even if some updates are lost from
> indices during that beat.

Offered as a suggestion, so recorded as a working position rather than a decision:

- **The poll keeps a position in the roster.** When the SQ fills, the poll records the position,
  rings the doorbell, and stops. It does not wait.
- **No lock is held across the file system probe, the SQ write, or the doorbell.** Each roster
  entry's handle is a `SharedHandle` from
  [win-shared-os-owned-handle](../../win-shared-os-owned-handle/), so the poll takes its own
  reference and probes with no lock held; the registration being dropped meanwhile only means the
  handle closes when the poll lets go.
- **There are two doorbells, one each way.** The watcher rings the caller's when the SQ is full or a
  batch is ready; the caller rings one back to the watcher when SQ space is free, and the poll
  resumes from the recorded position.
- **Roster changes between stop and resume are not handled specially.** Deferred, with a stated
  preference: resume at the same index even if that skips or repeats an entry for that beat.

Why the preference is cheap, as analysis: an attention carries state (last known and current
length), not a delta. A file skipped on one beat is picked up on the next with its current length,
so a skip costs latency for that file and loses no data. A file visited twice in one beat finds its
length unchanged the second time and produces nothing.

How this bears on questions still open:

- **Q22** (drop during a poll) is mostly settled by the shared handle: `Drop` need not wait for the
  probe. The remaining part is the `T` (Q24): an attention produced for a just-dropped registration
  still needs its `T`, which points toward shared ownership of `T` as well.
- **Q26** now has two doorbells to define, not one.
- **Q28 (new).** A single remembered position implies one poll in progress at a time. Is that the
  rule, with threads within a poll dividing the roster between them? Each entry's last known length
  is then written only by the thread visiting it, which keeps that write out of any lock too.

## Answer to Q26: doorbells are Windows events

> q26: doorbells are generally windows events in this crate's nomenclature

Settled: both doorbells are Windows event objects, matching the repository's existing usage. An
event is a kernel object with no thread behind it, so it is consistent with zero activity between
polls.

Still open under Q26:

- **Ownership.** Does the caller supply each event (alongside the SQ storage it already
  supplies), or does the watcher create them and lend their handles?
- **Reset mode.** windows-waitable-queues chose manual-reset, level-triggered ("there is something
  to take") after its probe showed an auto-reset event can lose a signal: two rings and one wait
  leave the waiter blocked with work pending. The same hazard applies to both doorbells here.

New:

- **Q29. Who waits on the resume doorbell?** When the caller rings the watcher's doorbell, something
  has to notice. Either the watcher registers a thread-pool wait on it (no thread while idle, but a
  live registration between polls, and the resume then runs on a pool thread, not the caller's), or
  the event is only a signal the caller may use, and resuming still means the caller calling in.

## Q29: tentative answer, deferred to implementation

> q29: can we defer this until we get into the implementation? Easy quick answer is that the watcher
> has a registered threadpool wait on the event to resume processing, but I wonder if that's the
> right approach. So mark that as the tentative answer and we will refine probably.

Tentative, not a decision: the watcher holds a registered thread-pool wait on the resume doorbell
and resumes the poll from it. Deferred until implementation, where the trade-offs in Q29 (a live
registration between polls; resuming on a pool thread rather than the caller's) can be judged
against working code.

## Answer to Q27: "last known" means last reported

> simpler is better, I agree, do it.

Q27 asked whether the watcher must hear the *consumed* signal from Q3, which turns on what an
attention's "last known" length means. Settled:

- **"Last known" is the last length the watcher reported** for that registration. A poll attends a
  file when its current length differs from it, and the attention moves it to the current length.
- **The watcher is never told what was consumed.** The *consumed* signal from Q3 still exists, but
  it stays on the caller's side: the caller knows what it has consumed, and that is what it would
  persist to re-register with a starting offset after a restart.
- **The only signal from caller to watcher is the resume doorbell** (SQ space is free, Q25/Q26).
  There is no CQ back to the watcher.
- **Q21 (partial consumption) is withdrawn**: it only existed if the watcher tracked consumption.

## Q12 superseded

Q12 asked whether the signals travel through windows-waitable-queues, which would have needed a
shape that merges records by key, or through a structure the registry owns. Q20 and Q26 answer it:
a caller-supplied queue, with Windows events as doorbells.

The merge-by-key requirement came from assuming a fixed-capacity queue with no rule for being full,
where merging was the way to avoid both blocking and dropping. It no longer applies:

- **A full queue stops and resumes** (Q25), so nothing needs to be merged to survive.
- **Attentions carry absolute lengths** (Q27), so two queued for one file are redundant, not
  conflicting; the later one supersedes the earlier.
- **Merging would mean rewriting entries already in the caller's storage**, against the position
  that the watcher writes to the queue without locks (Q25).

## Answer to Q1, and the goal behind the crate

> q1: not at this time. [...] I don't forsee a particular use for it. The main issue here is that
> there will be some number of off-machine "listeners" who are "watching" the stdout/stderr from
> these processes who want to see fairly up-to-date versions of the output.
>
> If we can do this sufficiently efficiently via this polling-on-the-beat, then I don't have any
> additional need. I am trying to balance between having "n" timers firing at random intervals
> polling the files vs. this crate which does the polling in a hopefully more efficient fashion en
> masse.
>
> As far as I can tell at this point, there is nothing to be gained by provoking the file change
> notification events.

Settled: nothing depends on the change notifications that a path-based query provokes on NTFS, and
no use for them is foreseen. This favours the held-handle query from the experiments, which reports
the current length on both file systems without that side effect and without an open/close per file
per beat.

Context worth keeping, because it states what the crate is for:

- **The ultimate consumers are off-machine listeners** watching children's stdout/stderr, who want
  a reasonably current view of the output.
- **The comparison the crate has to win is against one timer per file**, each firing on its own
  schedule. The crate's case is doing the same polling en masse, on one beat the caller owns, more
  efficiently. "Sufficiently efficient" is the bar; there is no further requirement behind it.

## Where the crate beats per-file timers: the engineer's analysis

> well it will at some point have fewer timers going off in the process and therefore be superior.
> It will also be superior at the bottom end since iterating over an array of two handles will be
> faster than two disjoint events being dispatched. it's somewhere in the middle that things get
> muddy.

An analytic argument, which is the kind that settles option questions here without a measurement:

- **At the top end**, one beat replaces many timer expirations, so fewer timer callbacks are
  dispatched in the process.
- **At the bottom end**, a loop over a handful of handles costs less than the same number of
  separately dispatched timer callbacks.
- **In the middle the comparison is not settled by either argument.** It depends on at least the
  number of files, the beat interval against the per-file timer intervals, and how often files
  actually change.

One piece of prior art bears on the middle and should be part of any comparison: thread-pool timers
accept a window (`SetThreadpoolTimerEx`'s `msWindowLength`) within which the system may coalesce
expirations, so "n timers" need not mean n separate wakeups. The per-file-timer baseline is
therefore a family of configurations, not one point.

Nothing here makes per-file timers an option this crate must offer; they remain available to any
caller outside it.

> we do not need to produce experimental comparative evidence, that is a job for the other
> component.

So characterising the middle is out of scope for this crate. Comparative evidence belongs to the
component that chooses between the approaches, not to the one that implements one of them.

## Answer to Q16: "buffered" means the OS view only

> q16: buffered means only from the os point of view. We have zero insight or control into the
> process

Settled: "buffered" in the brief refers to the operating system's side, the lag between a write
reaching the file and the directory entry or change notifications reflecting it. The child process
is opaque: the crate has no insight into it and no control over it, so output still held in the
child's own buffers is outside the crate's scope by definition. What the crate reports is exactly
what has reached the file system.

## Answer to Q8: the harness is discarded

> get rid of it. we don't want to keep any fixtures that aren't in Rust.

Settled: the PowerShell experiment harness was deleted, along with its run directories, and was
never committed. Only Rust fixtures are kept. The tables under "Experiments" above are now the only
record of what it found, and nothing re-checks them.

## Answer to Q6: the source of length changes is not the watcher's business

> q6: the client will unregister streams when it wants to stop receieving size updates. That
> doesn't quite answer your question, but really the source of length changes isn't quite your
> business.

Settled: the watcher knows registrations, not processes. A child's exit is not an input to it;
the client ends a stream by dropping its registration value (Q19) when it no longer wants size
updates. What causes a length to change, and when no more changes will come, is outside the
watcher's concern.

As a consequence, a "final drain" is something the client composes rather than something the
watcher provides: since the client owns the beat, it can poll once more before it unregisters.

## Answer to Q7: about 20,000 files at the top end

> q7: for file count, I'm going to make a guesstimate that 20,000 is at the top end, since that
> doesn't just mean 10,000 running "jobs", it means 10,000 remote clients watching running jobs. So
> that's really extreme to imagine.

A guesstimate, recorded as one: the top end is about 20,000 registered files, i.e. some 10,000
running jobs, each with stdout and stderr, each with remote clients watching. The engineer regards
this as an extreme.

What follows structurally, without a measurement: at the top end a full beat is about 20,000 length
queries, one system call each through a held handle, and the roster holds about 20,000 entries. The
other half of Q7, how many threads a poll may use and who supplies them, stays open.

## Q7, the threads half: as few as will do, specifics deferred

> for threads, we need to use the least possible to poll the handles within whatever we feel a
> reasonable beat interval is. there are too many unknowns at this point. it could be that query 20k
> handles for their current positions will be trivial in cost and the result will fall out
> trivially. it could be that once the handle count grows, we will have to plan out sharding of even
> the handle querying.

A principle, not a number: a poll uses the fewest threads that complete it within a reasonable beat
interval. Neither the beat interval nor the cost of one length query is known yet, so the thread
count is deliberately left open. Both outcomes named are live: one thread may be ample at 20,000
handles, or the querying may need to be sharded.

What the design owes this now is only not to foreclose sharding. The pieces already settled are
consistent with it: probes run outside any lock (Q25), and a roster divided between threads keeps
each entry's last known length written by one thread (Q28). The cost of a full poll at a given
population is this crate's own behaviour, so it is the crate's to observe once code exists; that is
distinct from the comparison against per-file timers, which belongs elsewhere.

## Answer to Q10: one crate

> for q10, I wasn't anticipating more than one crate. If this drives additional changes in other
> crates that's fine. I was not anticipating changes to windows-overlapped-io-sys but we can
> discuss

Settled: the watcher is one crate, whatever its internal layers. Changes it drives in other crates
are acceptable, in line with the repository's fix-the-layer policy. Changes to
[windows-overlapped-io-sys](../../windows-overlapped-io-sys/) were not anticipated and are open to
discussion.

### Where windows-overlapped-io-sys meets this crate (analysis)

**Iteration 1 needs nothing from it.** With no reads, the watcher only opens each file (read-only,
overlapped, sharing read/write/delete) and queries its length through the handle. Its
`UnassociatedEndpoint::open` would serve for the open, since it opens with `FILE_FLAG_OVERLAPPED`
through std's `OpenOptions`, whose default share mode is read/write/delete; but iteration 1 has no
completion backend to associate with, so the endpoint adds nothing yet.

**The reads iteration is where the two meet, and they disagree on ownership.**
[endpoint.rs](../../windows-overlapped-io-sys/src/endpoint.rs) gives `UnassociatedEndpoint` an
exclusively owned `OwnedHandle`, deliberately not `Clone`, because a second owner could break the
single-association invariant the completion backends rely on. The Q25 working position holds each
roster entry's handle as a `SharedHandle` so a probe runs on its own reference with no lock held.
Both cannot own the same handle as things stand. Three ways through, recorded as Q30:

- **(a) Share the endpoint, not the handle.** The roster holds an `Arc` of the endpoint and probes
  through its borrowed handle. The lock-free probe property is kept and windows-overlapped-io-sys
  is unchanged; win-shared-os-owned-handle is then not used for this.
- **(b) Teach the endpoint to hold a `SharedHandle`.** A change to windows-overlapped-io-sys, which
  would have to keep its single-association guarantee with shared holders.
- **(c) Two handles per file**, one probed and one read. No change anywhere, at twice the handles.

## Answer to Q30: the endpoint holds a SharedHandle

> I assumed b. I think that windows-overlapped-io-sys needs some reworking with the shared handle
> type from what you're saying

Settled as the direction: option (b).
[windows-overlapped-io-sys](../../windows-overlapped-io-sys/) is reworked so an endpoint can hold
its handle through `SharedHandle` from
[win-shared-os-owned-handle](../../win-shared-os-owned-handle/), and the watcher's roster shares
that handle with the endpoint rather than wrapping the endpoint. This is a change to another crate,
which Q10 allows.

What the rework has to answer, as analysis for whoever does it, not a design for it:

- **What the exclusivity actually protects.** `UnassociatedEndpoint` is deliberately not `Clone`
  because a second owner "could route completions through a duplicate handle and break the
  single-association invariant." The hazard is completions: an overlapped operation issued through
  any holder of a handle associated with a completion port posts a packet that the backend will
  interpret as one of its own operations. A synchronous length query produces no completion. So the
  guarantee to keep is "only the endpoint submits and associates", not "only the endpoint holds".
- **Whether sharing opens a new path to that hazard.** Every holder of a `SharedHandle` can lend a
  `BorrowedHandle`, and the endpoint already lends one today through `handle()`. Issuing I/O
  through a borrowed handle needs `unsafe` FFI, or a safe duplicate (`try_clone_to_owned`) that the
  existing borrow already permits. If that holds, sharing adds holders but no new safe route to a
  foreign completion. This is reasoning to be verified in the rework, not a verified claim.
- **The safety contract of `assume_overlapped`**, which currently requires that no duplicate
  exists and that ownership transfers exclusively, would need restating for shared holders.

## Answer to Q28: one poll at a time

> Q28: I don't see any need to support more than one poll concurrently. If that makes things more
> complicated, don't do it. If that makes things less complicated, do it.

The engineer's rule is "whichever is simpler"; applying it: **one poll at a time** is simpler.

- **One resume position.** Q25's remembered roster index is a single value. Concurrent polls would
  each need their own, and would have to agree on which entries each had covered.
- **One writer per last known length.** With one poll, and the roster divided among that poll's
  threads, each entry's last known length (Q27) is written by exactly one thread and needs no
  synchronisation. Two polls could visit the same entry and race on it, producing duplicate or
  reordered attentions unless it were made atomic and compared on update.

How it is enforced is an implementation question, but the repository's ladder applies: an exclusive
borrow (`poll(&mut self)`) makes a second concurrent poll fail to compile, which is the strongest
rung. If Q29's tentative answer stands (resuming on a thread-pool wait), the watcher's state is
shared with a pool callback and the exclusive borrow is no longer available, so a runtime guard
would take its place.

This does not settle registration or unregistration concurrent with a poll, which remains Q22.

## Answer to Q17: the measure is thread wakeups

> q17: resource use between polls that I was referring to was really thread wakeups. the process
> should, to the greatest degree possible, quiesce.

Settled: "zero resource use between polls" means **no thread wakeups caused by the crate**. Between
polls the process should quiesce as far as possible. State that sits still -- held handles, the
roster, memory -- is not what the requirement is about.

How the other answers read against that measure:

- **Q29's tentative thread-pool wait on the resume doorbell** wakes nothing until the caller rings
  it, so it is consistent with quiescing. A registration that sits idle is state, not a wakeup.
- **Outstanding overlapped reads in a later iteration** wake a thread only when they complete,
  which happens because the file was written to; that is activity the caller asked to observe,
  not idle churn.
- **Q18** (return when finished, or start and signal later) is not decided by this measure either
  way, provided nothing the poll started is still waking threads once it has completed.

## Answer to Q18: leaning asynchronous

> q18: to be determined? I am against synchronous APIs so I think that the poll initiates the poll
> and there's some notification when complete. The final exact shape is to be determined, will take
> input.

A leaning, not a decision: **`poll` initiates a poll and returns; completion is notified later.**
The engineer is against synchronous APIs. The exact shape is to be determined.

What follows from the leaning, as analysis:

- **Quiescing (Q17) still holds** as long as the poll's work ends when its completion is signalled:
  work outliving the call is fine, work outliving the poll is not.
- **Q28's one-poll-at-a-time rule needs a runtime guard.** An asynchronous poll cannot hold an
  exclusive borrow across its lifetime, so a second `poll` while one is in flight must be refused
  or rejected at run time rather than at compile time. The same is true under Q29's thread-pool
  resume, so the two point the same way.
- **A third signal appears.** Besides "batch ready / SQ full" (watcher to caller) and "space free"
  (caller to watcher), the caller now needs to learn "this poll is complete". Whether that is its
  own doorbell or a marker in the SQ behind the existing one is Q31.

## Answer to Q22: unregistration during a poll is legal

> q22: I would like unregistration during a poll to be legal. I suppose it should prevent
> notifications after registration but they may already be in a SQ so perhaps the point is moot and
> the client just has to deal with it so this point is already resolved. registration during a poll
> does not need to be picked up, but it may be.

Settled:

- **Dropping a registration while a poll is in progress is legal**, and `Drop` does not wait for
  the poll.
- **No guarantee that attentions stop at unregistration.** An attention for the file may already
  be in the caller's SQ, and one may be written after the drop by a poll that had already taken
  its reference. The client must tolerate attentions for registrations it has dropped. Suppressing
  late ones is therefore not part of the contract.
- **A registration made during a poll may or may not be picked up by that poll.** Either is
  correct; the next poll will see it.

Consequence for Q24, as analysis: since an attention can outlive its registration, it cannot carry
a borrow of the registration's `T`. The remaining options are a clone (`T: Clone`, one copy per
attention) or a shared pointer to `T`.

## Answers to Q24 and Q31

> q24: I'm going to say that it has to be Clone. Otherwise the lifetime management is too hard to
> really work out.
>
> Q31: a SQE that says that the poll is done and the doorbell is rung for attention to the SQ.

Settled:

- **Q24: `T: Clone`.** Each attention carries its own clone of the registration's `T`, so an
  attention never depends on its registration still existing (which Q22 requires). Whether `T` must
  also be `Send` or `Sync` follows from where clones are taken and consumed, and is left to
  implementation. A caller that wants clones to be cheap can make its `T` an `Arc`; that is the
  caller's choice, not the crate's.
- **Q31: completion is an SQ entry.** When a poll finishes, the watcher writes a distinguished
  "poll done" entry into the caller's SQ and rings the same doorbell. There is no separate
  completion doorbell.

What follows, as analysis:

- **The SQ carries two kinds of entry**: an attention (`T` clone, last known length, current
  length) and a poll-done marker.
- **The done marker needs a slot like any other entry.** If the SQ is full when the poll's last
  attention is written, the marker waits for the resume doorbell (Q25) the same way an attention
  would; a poll is not complete until its marker is in the SQ.
- **Signals now number two**: the watcher-to-caller doorbell (batch ready, SQ full, or poll done,
  all read from the SQ) and the caller-to-watcher resume doorbell.

## Who creates the queues and doorbells

> The SQ, CQ and doorbells should be created by the crate unless you have a better jurisdiction for
> them

Settled, superseding part of the Q20 answer: **the crate creates the SQ and the doorbell events.**
The Q20 record said the caller supplies the SQ's storage; that is replaced. What survives from Q20
is the shape: the watcher writes entries into the SQ, the caller drains it, and doorbells signal in
each direction.

No better jurisdiction was found, and the crate owning them has concrete advantages, as analysis:

- **The crate fixes the events' reset mode**, which is a correctness property, not a preference:
  windows-waitable-queues found that an auto-reset event can lose a signal. A caller-supplied event
  of the wrong kind would break the protocol silently.
- **The SQ's lifetime matches the watcher's**, so no caller-owned memory has to outlive anything the
  crate does with it.
- **What the caller still controls is capacity**, presumably as a parameter when the watcher is
  created; that is the part of "the caller supplies the SQ" that was worth keeping.

One term needs checking: since Q27 there is no CQ. Nothing flows from caller to watcher except the
resume doorbell. Whether "CQ" here names something new, such as the caller handing drained slots
back, or is a leftover from the earlier ring shape, is not yet settled.

## Answer to Q32: there is no CQ

> you're right there's no CQ any more

Settled: the "CQ" in the creation answer was a leftover from the earlier ring shape. The mechanism
is **one SQ**, created by the crate, written by the watcher and drained by the caller, plus **two
doorbells**: watcher to caller (batch ready, SQ full, or poll done, all read from the SQ) and caller
to watcher (space free, resume).

## Q26 completed: manual-reset doorbells

> agreed manual reset is less error prone

Settled: both doorbells are **manual-reset** events, level-triggered, matching
[windows-waitable-queues](../../windows-waitable-queues/src/doorbell.rs), whose probe showed an
auto-reset event losing a signal (two rings, one wait, a waiter blocked with work pending).

With this, every question raised in the session has an answer, a withdrawal, or a supersession.
Two answers remain tentative and are deferred to implementation: the exact shape of `poll` and its
completion (Q18, leaning asynchronous, with completion as an SQ entry per Q31), and who waits on the
resume doorbell (Q29, tentatively a thread-pool wait).

## Q18, Q31 and Q29 firmed up

> q18,q31: poll definitely initiates polling. poll done is definitely a sqe.
>
> q29: who waits on the resume doorbell: assume it's a threadpool wait

Settled:

- **Q18: `poll` initiates a poll and returns.** No longer a leaning. Completion is the poll-done SQE
  of Q31, also definite.
- **Q29: the resume doorbell is waited on by a thread-pool wait the watcher registers.** Recorded as
  the working assumption to build on, firmer than the earlier "tentative", and still open to
  refinement in implementation.

## Cancellation means shutdown

Raised by the engineer: with the resume doorbell waited on by a thread-pool wait (Q29), can
processing of the SQ be cancelled?

Analysis given in answer, from
[windows-threadpool-sys](../../windows-threadpool-sys/src/wait.rs)'s `ThreadpoolWait`: yes. A
*parked* poll (waiting on the resume doorbell) is cancelled by stopping the wait; `stop_and_drain`
is preferred over `try_cancel_pending`, which that crate documents as able to stall the whole pool
(repaired under its `self-heal` feature), matching the repository's teardown-drains decision. Since
draining lets a queued callback run, a cancelled flag is set first and the callback returns at once
on seeing it. A *running* poll cannot be interrupted mid-call, so it stops cooperatively at the next
roster entry, each entry being one system call.

> Re: what the caller sees: this is interesting. I guess I think of this situation as the poller
> mostly wanting to completely stop operations now and into the future, not just this iteration. I
> am having a hard time coming up with an example of why a caller would want to cancel one iteration
> of a poll, only to start another one later. so this I believe subsumes the next question also.

Settled (Q33): **cancellation is shutdown.** Stopping means stopping the watcher for good, not one
poll. No use was found for cancelling a single iteration and polling again later, so that is not
offered; nothing in the design forecloses adding it if a use appears. This subsumes "where does the
next poll start after a cancel": there is no next poll.

What shutdown must do, as analysis: set the stop flag, `stop_and_drain` the resume wait, let a
running poll stop at its next entry, then release the doorbells and the SQ. Entries still in the SQ
are the caller's to drain before shutting down, or are discarded with it. The poll-done SQE is not
how the caller learns of shutdown, since a parked poll is usually parked because the SQ is full.

Open (Q34): **the shape of shutdown.** Dropping the watcher must block until it is quiescent,
because pool callbacks reference its state. Given the engineer's stance against synchronous APIs, is
`Drop` the only way to shut down, or is there also an explicit asynchronous shutdown that signals
when it is quiescent, so that the eventual `Drop` has nothing left to wait for?

## Answer to Q34: shutdown is requested, signalled, then dropped

> if we are handling the callback for a notification and we decide to shut down, we need to do
> something to instruct the watcher to cancel the iteration which would presumably set the event
> signifying the end of the polling cycle and then the caller could drop the watcher handle.

Settled: **shutdown is an explicit, non-blocking request**, typically made from inside the caller's
own callback for a watcher notification. The watcher stops the poll (Q33's mechanism), signals that
the polling cycle has ended, and only then does the caller drop the watcher, by which point `Drop`
has nothing left to wait for. `Drop` without a prior shutdown must still be safe, by blocking until
quiescent, but it is the fallback, not the intended path.

Two properties this gives, as analysis:

- **No blocking inside a callback.** A caller's callback that dropped the watcher directly would
  block a pool thread until the watcher was quiescent. The request-then-drop sequence keeps the
  callback short, consistent with the stance against synchronous APIs.
- **The request must be idempotent, and `poll` after shutdown must be refused** (Q28's runtime
  guard already exists to refuse a second concurrent poll; it extends naturally to "no polls after
  shutdown").

Open (Q35): **how "polling has ended" reaches the caller when the SQ is full.** Q31 makes poll-done
an SQ entry, but shutdown is often requested exactly when the SQ is full and the poll is parked.
Options, none chosen:

- **A reserved slot.** The SQ keeps one slot back that only a terminal entry (poll done, or
  shut down) may use, so the final entry always fits and everything still arrives through the SQ
  and its one doorbell.
- **A terminal state beside the SQ**, read by the caller whenever the doorbell rings.
- **A dedicated shutdown event.**

## A stop result returned from the caller's callback: set aside

Raised by the engineer: the caller's notification callback could return a value that includes an
option to cancel further processing. Analysis given in answer: a return value reaches whoever made
the call, and in the current design the caller's callback is the caller's own thread-pool wait on
the doorbell, so its return goes to the thread pool, not to the watcher. It would only work if the
watcher itself invoked a caller-supplied function, which would replace both doorbells and Q35 but
make the crate run client code on its threads.

> oh you're right the cq is gone

Read as setting the idea aside: with no CQ there is no path from the caller back to the watcher
other than the resume doorbell, so stopping stays a separate, non-blocking shutdown request (Q34 as
recorded). The doorbell model of Q20, Q26 and Q29 stands, and Q35 remains open. This reading is an
interpretation and is open to correction.

## Answer to Q35: no special mechanism

> q35: I don't think this has to be answered. It would come at the end of the entries in the SQ as
> normal. If the caller is not interested, so be it. They can drop the watcher if they want after
> they themselves cancel.

Settled: the terminal entry is an ordinary SQ entry, queued after the others and waiting for space
like them. No reserved slot, side state or extra event. A caller that has requested shutdown and
does not care to see the terminal entry may drop the watcher without draining for it.

The consequence for `Drop`, as analysis: it must not depend on the terminal entry having been
written or read. It waits only for the watcher's own callbacks to finish (Q34's fallback), which,
after a shutdown request, is at most a running poll reaching its next roster entry.
