# Design session 2026-10-24: prior art for durable-ioring's design

Component: [crates/durable-ioring](../crates/durable-ioring/COMPONENT.md). A research record, not a decision record: it
backs decisions already made, and decides nothing. The engineer asked for "as much backing as we can", framing the work as
"some of our work here is novel but ... largely we are putting pieces together to make a bunch of understood mechanics
usable." Decisions it bears on, all in [DESIGN-NOTES.md](../crates/durable-ioring/DESIGN-NOTES.md): the epoch and seal
model, gates across lineages, flush domains, failures and their resolution, and the no-`std` capacity work of
[DI-D-97](../crates/durable-ioring/DESIGN-NOTES.md#di-d-97) to
[DI-D-109](../crates/durable-ioring/DESIGN-NOTES.md#di-d-109).

**Method.** A research agent searched and opened sources; the assistant then downloaded the papers it could not parse and
read the cited passages from their text. **VERIFIED** means a passage in the source, read on 2026-10-24, says what is
attributed to it; **UNVERIFIED** means recalled or summarised and not read. A search engine's summary is never counted as
verified: one claimed io_uring never returns `EBUSY` on completion overflow, and the man page says it does.

## Summary

The engineer's framing holds. Each mechanism has close prior art, mostly from file systems and databases, where it lives
inside one engine. What durable-ioring adds is exposing them together to a consumer above an OS ring. The rule under the capacity work -- what
clears capacity is never refused, so its room is reserved by derivation -- has a direct precedent in ARIES' reserved
rollback log space. One piece has no comparable prior art in what was searched: an unresolved failure holding the submit
capacity of the writes it puts at risk (DI-D-100). Two findings bear on the design beyond confirming it, under "Findings
that bear on the design" below.

## A. Epochs and seals as durability intervals

- **Silo** (Tu et al., SOSP 2013) [1], VERIFIED: "we solve [correct recovery] using a form of epoch-based group commit.
  Time is divided into a series of short epochs"; "A thread periodically computes and publishes a global durable epoch
  D = min dl. All transactions with epochs <= D are known to have been durably logged." That is a durable high-water mark
  over epochs, as `Durable { through }` is. Silo's epoch is a global clock tick; durable-ioring's is the consumer's own
  tag, per lineage.
- **Barrier-enabled IO stack** (Won et al., FAST 2018) [2], VERIFIED: "A barrier write is used to delimit an epoch", and
  its scheduler "preserves the partial order between the epochs, (ii) the requests within an epoch can be freely scheduled
  with each other". Epochs as the unit within which writes reorder freely and between which order holds is the same idea
  as a seal's span.
- **Rethink the Sync** (Nightingale et al., OSDI 2006) [3], VERIFIED by the research agent (abstract and introduction):
  external synchrony, output held until the writes it depends on commit -- a durability interval drawn by causality.
- **io_uring** [4], VERIFIED: `IORING_OP_FSYNC` runs in parallel with earlier writes unless `IOSQE_IO_LINK` or
  `IOSQE_IO_DRAIN` orders it. The OS ring gives no durability interval; a library must build one.
- Verdict: **established pieces; the consumer-named, per-lineage interval above an OS ring is the new combination.**

## B. Gates across lineages, and refusing a cycle

- **ARIES** (Mohan et al., TODS 1992) [5], VERIFIED: "The WAL protocol asserts that the log records representing changes
  to some data must already be on stable storage before the changed data is" written, and the buffer manager "may have to
  force the log up to the LSN of the dirty page before writing the page". A gate on another lineage's epoch is the WAL rule
  made general.
- **Featherstitch** (Frost et al., SOSP 2007) [6], VERIFIED: dependencies among patches, exported to applications as
  "patchgroups"; "Circular dependencies among patches cannot be resolved and are therefore errors", and "The patch
  abstraction places only one substantive restriction on its users, namely, that circular dependency chains are errors."
  The closest prior art for consumer-declared ordering with cycles refused; Featherstitch's is inside a file system,
  durable-ioring's across a consumer's lineages.
- **io_uring links** [4], VERIFIED: a chain in one submission, which cannot express a cycle and does not cross streams.
- Verdict: **well established (WAL, Featherstitch); exporting it across independent lineages with cycles refused at push
  is the combination.**

## C. Flush domains

- **Linux block layer** [7], VERIFIED: `REQ_PREFLUSH` and `REQ_FUA` act on a device's volatile write cache, and remapping
  layers must pass them through.
- **NVMe Flush** [8], VERIFIED: commits data "associated with the specified namespace(s)" and "may also flush additional
  data" -- scoped, and allowed to over-flush.
- **`FlushFileBuffers`** [9], VERIFIED: per file handle, or per volume through a volume handle.
- No prior art was found for a consumer-supplied provider answering for domains the OS flush does not reach.
- Verdict: **the primitives are well established; a declared set of domains per target with a pluggable provider is the
  combination.**

## D. Failures, suspect sets, heal and abandon

- **`errseq_t`** [10] and LWN's "Improved block-layer error handling" [11], VERIFIED: one error and a sequence counter per
  mapping, each opener told once that an error happened since it last looked. It bounds its storage by keeping no detail of
  which writes were hit; durable-ioring keeps that detail (the suspect set) and bounds it by derivation instead (DI-D-109).
- **"PostgreSQL's fsync() surprise"** (LWN) [12], VERIFIED: a failed fsync's error reported once and the pages marked clean.
- **Rebello et al., "Can Applications Recover from fsync Failures?"** (ATC 2020) [13], VERIFIED: "all three file systems
  mark pages clean after fsync fails, rendering techniques such as application-level retry ineffective"; none of ext4, XFS
  and Btrfs "retry data or journal-block writes"; PostgreSQL responds "by crashing and restarting without retrying the
  fsync ... WiredTiger/MongoDB and MySQL followed suit". Studied applications "revert in-memory state and report the error"
  or crash, and "none of the applications retry fsync on failure".
- **Pillai et al.** (OSDI 2014) [14], VERIFIED: application update protocols depend on "persistence properties" that "vary
  widely among six popular Linux file systems", with crash vulnerabilities found across eleven systems.
- **Ganesan et al.** (FAST 2017) [15], VERIFIED: "a single fault can have disastrous cluster-wide effects" in replicated
  storage.
- **Windows** [16], VERIFIED by the research agent: Event ID 50/26, "Delayed Write Failed", data lost. The status code
  `STATUS_LOST_WRITEBEHIND_DATA` was not found in an opened Microsoft source (UNVERIFIED here).
- Verdict: **the problem is well documented and the "report once" half is established; a frozen suspect set the consumer
  must heal or abandon is the new structure.**

## E. Every allocation at construction; refusal at admission

- **"The Power of 10"** (Holzmann, IEEE Computer 2006) [17], VERIFIED: "Rule: Do not use dynamic memory allocation after
  initialization. Rationale: This rule is common for safety critical software".
- **TigerBeetle** [18], VERIFIED by the research agent: all memory allocated statically at startup, sized from
  configuration, none after.
- **Linux `mempool_t`** [19], VERIFIED by the research agent: reserves for "guaranteed, deadlock-free memory allocations
  during extreme VM load" -- the forward-progress reservation for writeback.
- **SPDK** (`-ENOMEM` when a queue pair's requests are exhausted): UNVERIFIED; the passage was not reached.
- **ARIES** [5], VERIFIED: because undo writes a bounded amount of log, "we can keep in reserve enough log space to be able
  to roll back all currently running transactions under critical conditions (e.g., log space shortage)". The action that
  frees resources is never refused because its room was reserved from a bound; DI-D-98's seal and DI-D-109's abandon are
  the same rule.
- Verdict: **well established, including the rule that a clearing action is never refused.**

## F. What arrives unasked, bounded by derivation

- **io_uring** [4][20], VERIFIED: completion-queue entries that overflow are dropped and counted without
  `IORING_FEAT_NODROP`; with it the kernel holds them in allocated storage, still drops them if that allocation fails
  (`EBADR`), and `io_uring_enter` returns `EBUSY` while they cannot be flushed. `IORING_SETUP_CQSIZE` sizes the completion
  queue against submissions. Linux did not make unasked arrivals both bounded and lossless without allocating; DI-D-109
  ties each to room reserved at admission instead.
- **ARIES' reserved rollback space** [5] (under E) is the same derivation: a bound on what must happen, reserved ahead.
- **NVMe queue-pair sizing and RDMA completion credits**: the same reserve-at-admission shape. UNVERIFIED (not opened).
- **An unresolved failure holding the capacity of the writes it puts at risk** (DI-D-100): nothing comparable found in
  kernel, NVMe, RDMA or database sources searched.
- Verdict: **ring backpressure is established; failure-as-capacity is, as far as was searched, novel.**

## G. Handles as slots

- **`slotmap`** [21] and Weissflog, "Handles are the better pointers" [22], VERIFIED by the research agent: an index with a
  per-slot version, so a reused slot never answers a stale key.
- Verdict: **well established.**

## H. Lending a failure's records by scoped borrow

- No named prior art found for lending a bounded record set through a closure in a no-allocation API, or for expanding one
  stored marking into many events at pop. Internal iteration and RCU read sections are the general ancestors; nothing
  specific was opened.
- Verdict: **weakly searched; no comparable prior art found.**

## I. Forgetting an abandoned epoch once the mark passes it

- **Raft** (Ongaro and Ousterhout) [23], VERIFIED: "Once a server completes writing a snapshot, it may delete all log entries
  up through the last included index" -- detail below a watermark discarded, its position kept.
- **ARIES** [5]: log truncation below a checkpoint; the passage was not located in this pass (UNVERIFIED).
- **Kafka** log start offset: UNVERIFIED (documentation body not retrieved).
- Verdict: **well established as a pattern.**

## Findings that bear on the design

These are findings, not decisions; no work is queued by this record.

1. **Ordering without durability is a recognised, separate primitive, and durable-ioring does not offer it.** OptFS [24],
   VERIFIED: "osync(), which ensures ordering between writes but only eventual durability, and dsync(), which ensures
   immediate durability as well as ordering". Won et al. [2], VERIFIED: `fbarrier()` and `fdatabarrier()` "separate the
   ordering guarantee from the durability guarantee", returning "without waiting for the associated blocks to become
   durable", and report SQLite throughput gains from it. durable-ioring's gate holds an operation until the gate's epoch is
   durable, the stronger and costlier guarantee. Both papers build ordering into the storage stack itself (an
   order-preserving block layer, or OptFS's checksums and delayed writes); whether Windows documents anything an
   ordering-only gate could bind to was not examined.
2. **Recovery from a failed flush cannot be a retried flush.** Rebello et al. [13] found file systems mark pages clean after
   a failed fsync, so the same data is not written again. This backs DI-D-12 (c): a heal is the consumer saying it has
   re-issued the writes, and only a later seal covering the re-issue releases it. A design that healed by flushing again
   would have reported durability for data that was never rewritten.

## Sources

Read 2026-10-24.

1. Tu, Zheng, Kohler, Liskov, Madden, "Speedy Transactions in Multicore In-Memory Databases", SOSP 2013.
   https://people.csail.mit.edu/stephentu/papers/silo.pdf
2. Won et al., "Barrier-Enabled IO Stack for Flash Storage", FAST 2018.
   https://www.usenix.org/system/files/conference/fast18/fast18-won.pdf
3. Nightingale, Veeraraghavan, Chen, Flinn, "Rethink the Sync", OSDI 2006.
   https://www.usenix.org/legacy/events/osdi06/tech/nightingale/nightingale_html/index.html
4. io_uring_enter(2). https://man7.org/linux/man-pages/man2/io_uring_enter.2.html
5. Mohan, Haderle, Lindsay, Pirahesh, Schwarz, "ARIES: A Transaction Recovery Method Supporting Fine-Granularity Locking and
   Partial Rollbacks Using Write-Ahead Logging", ACM TODS 1992. https://web.stanford.edu/class/cs345d-01/rl/aries.pdf
6. Frost, Mammarella, Kohler, de los Reyes, Hovsepian, Matsuoka, Zhang, "Generalized File System Dependencies", SOSP 2007.
   http://read.seas.harvard.edu/~kohler/pubs/frost07generalized.pdf
7. Linux kernel, "Explicit volatile write back cache control".
   https://www.kernel.org/doc/Documentation/block/writeback_cache_control.txt
8. nvme-flush(1). https://www.mankier.com/1/nvme-flush
9. FlushFileBuffers. https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers
10. Linux kernel, "The errseq_t datatype". https://docs.kernel.org/core-api/errseq.html
11. LWN, "Improved block-layer error handling". https://lwn.net/Articles/724307/
12. LWN, "PostgreSQL's fsync() surprise". https://lwn.net/Articles/752063/
13. Rebello, Patel, Alagappan, Arpaci-Dusseau, Arpaci-Dusseau, "Can Applications Recover from fsync Failures?", USENIX ATC
    2020. https://www.usenix.org/system/files/atc20-rebello.pdf
14. Pillai et al., "All File Systems Are Not Created Equal: On the Complexity of Crafting Crash-Consistent Applications",
    OSDI 2014. https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-pillai.pdf
15. Ganesan et al., "Redundancy Does Not Imply Fault Tolerance: Analysis of Distributed Storage Reactions to Single Errors
    and Corruptions", FAST 2017. https://www.usenix.org/system/files/conference/fast17/fast17-ganesan.pdf
16. Microsoft, "Delayed Write Failed" error.
    https://learn.microsoft.com/en-us/troubleshoot/windows-server/networking/delayed-write-failed-error-data-lost
17. Holzmann, "The Power of 10: Rules for Developing Safety-Critical Code", IEEE Computer 2006.
    https://spinroot.com/gerard/pdf/P10.pdf
18. TigerBeetle, TIGER_STYLE.md. https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md
19. Linux, mm/mempool.c. https://github.com/torvalds/linux/blob/master/mm/mempool.c
20. io_uring_setup(2). https://man7.org/linux/man-pages/man2/io_uring_setup.2.html
21. slotmap. https://docs.rs/slotmap/latest/slotmap/
22. Weissflog, "Handles are the better pointers", 2018. https://floooh.github.io/2018/06/17/handles-vs-pointers.html
23. Ongaro, Ousterhout, "In Search of an Understandable Consensus Algorithm (Extended Version)". https://raft.github.io/raft.pdf
24. Chidambaram, Pillai, Arpaci-Dusseau, Arpaci-Dusseau, "Optimistic Crash Consistency", SOSP 2013.
    https://research.cs.wisc.edu/adsl/Publications/optfs-sosp13.pdf

## Not found

- A Microsoft source naming `STATUS_LOST_WRITEBEHIND_DATA`; NT handle-table reuse with tag bits.
- The SPDK passage on `-ENOMEM` backpressure; Kafka's high-watermark documentation; ARIES' checkpoint truncation passage.
- Anything comparable to failure-as-capacity (DI-D-100), or to lending failure records by scoped borrow (DI-D-108).
