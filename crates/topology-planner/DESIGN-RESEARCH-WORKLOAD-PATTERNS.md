# Research: small workloads for topology-planning experiments

**Research and experimental proposals, not an accepted archetype catalog.**
Prepared 2026-09-18 19:52:38 -07:00. This is an initial primary-source survey,
not an exhaustive literature review or a report of benchmarks run here.
Work is tracked by [CHECKLIST.md](CHECKLIST.md) `EP-R1.7`.
The broader [DESIGN-PROPOSAL-EP-R1.7.md](DESIGN-PROPOSAL-EP-R1.7.md) remains
a working basis, not a contract to finish before experimentation.

## Direction from the discussion

The engineer's read/parse/collate/format/write example illustrated a complex
topology, not the minimum first implementation. Start with simpler established
workloads, try alternative arrangements against the same constraints, and revise
or discard arrangements that do not work. The eventual archetypes and runtime
boundary are hypotheses to investigate, not commitments already made.

The source observations below are separated from proposed experiments. Similar
mechanisms in another system do not establish their performance on our Windows
backends, and do not require adopting that system as a dependency.

## What established systems do

### Direct processing versus staged processing

[DPDK's architecture overview](https://doc.dpdk.org/guides/prog_guide/overview.html)
describes both run-to-completion packet processing and a pipeline that passes
packets/messages between cores using rings. It also distinguishes polling,
interrupt-driven examples, and event-based hardware support.

This supplies a concrete comparison without first needing several devices:
the same logical processing can stay with one worker or cross a queue boundary.
For this project's experiments, the question is when overlap between stages
outweighs the measured handoff and scheduling cost. That is a question to test,
not an answer supplied by DPDK.

### Independent asynchronous requests with shared workers

Microsoft's [I/O completion port documentation](https://learn.microsoft.com/en-us/windows/win32/fileio/i-o-completion-ports)
describes a completion queue serviced by workers under a concurrency setting.
It applies to overlapped files and network endpoints. Completion packets and
worker scheduling have separate ordering rules; queue order is not an
application-effect ordering contract.

This is a Windows-native comparison for many independent operations: a shared
completion service rather than preassigned per-core ownership. The experiment
should retain it as a genuine candidate, not merely a straw-man baseline that
is expected to lose to pinning.

### Partitioned ownership and per-thread I/O context

[SPDK's NVMe driver](https://spdk.io/doc/nvme.html) exposes independent queue pairs,
with each pair used by one thread at a time. Its documented ownership model
assigns data to an owner and sends operations to that owner.
[SPDK's concurrency description](https://spdk.io/doc/concurrency.html) separates
lightweight execution contexts from the system threads that run them and explains
per-thread I/O channels.

[Seastar's tutorial](https://docs.seastar.io/master/tutorial.html) describes
per-core execution, partitioned memory, and explicit messages between shards.
These are precedents for keeping state with an owner rather than accessing it
from every worker.

The transferable question is shared versus partitioned ownership, not "one Windows
I/O ring equals one hardware NVMe queue." Neither SPDK's direct driver control nor
its performance claims become properties of our Windows file-I/O backend.

### Ordered ingestion with overlapping stages

[RocksDB's pipelined-write description](https://github.com/facebook/rocksdb/wiki/Pipelined-Write)
separates write-ahead-log work from memtable insertion. A write still has its own
ordering dependency, while work belonging to different writers can overlap.

This is a concrete later example of a constraint that permits some overlap but
not arbitrary reordering. We can study that dependency shape without implementing
a database or assuming a filesystem write completion establishes durability.
The source's benchmark is for its stated setup, not a predicted gain for ours.

### Network receive flow ownership

Microsoft's [RSS introduction](https://learn.microsoft.com/en-us/windows-hardware/drivers/network/introduction-to-receive-side-scaling)
describes hashing and indirection to distribute receive processing while keeping
a connection associated with a processor. The
[RSS processor-count guidance](https://learn.microsoft.com/en-us/windows-hardware/drivers/network/setting-the-number-of-rss-processors)
also describes the additional interrupt/DPC overhead of using more processors.

This makes network flow distribution a candidate workload dimension, rather than
assuming that adding workers always increases useful throughput. It does not make
the RSS processor the owner of the application's parser. Receive processing,
application scheduling, and buffer location remain distinct observations.

## Candidate workloads, from simple to more demanding

The following examples are our proposed reductions of those patterns. They are
not claims that the source systems implement these exact fixtures.

| Workload | Competing arrangements | Constraint kept fixed | What it could expose |
|---|---|---|---|
| Read independent fixed-size blocks and checksum each block | Process on the read/completion owner; queued reader/processor; independent direct workers | Same blocks, operation, result identity and memory allowance | Handoff cost versus I/O/CPU overlap and added worker capacity |
| Serve independent small requests | Shared completion workers; explicitly assigned worker lanes | Same request/reply semantics and outstanding-work bound | Scheduling variability, burst response, and partition imbalance |
| Maintain per-key counters or a small lookup service | Shared synchronized state; key-owned workers with messages | Same per-key order and results | Ownership locality versus routing cost, especially with skewed keys |
| Ingest ordered records through two dependent operations | One owner; staged overlap between independent records | Per-record dependency and required publication order | Legal overlap, batch size, and head-of-line blocking |
| Stream network data into one bounded processing stage | Completion-side processing; separate processor; per-flow lanes | Same framing, ordering and delivery obligations | Receive distribution versus application placement and queueing |

This is not a ranking of workload importance. It is a proposed sequence for
introducing one new dependency at a time. The complex multi-endpoint example stays
in the broader proposal; beginning with one source does not remove it from scope.

## Recommended first experiment: read and checksum

Use one caller-authorized fixture file, independent fixed-size blocks, one
deterministic checksum operation per block, and results indexed by block identity.
No parser state, cross-record aggregation, output-device placement, or external
write effects are required. The same result set must be produced by every variant.

Compare separate implementations of these arrangements:

- **Direct owner:** one execution owner submits asynchronous reads, processes
  completions, and computes block checksums. It may keep multiple reads outstanding;
  "one owner" must not be handicapped as one blocking read at a time.
- **Reader plus processor:** the reader/completion owner transfers buffer ownership
  through a bounded queue to a second worker; buffer returns and shutdown are explicit.
- **Independent direct workers:** two owners each read and process their assigned
  blocks, with no inter-worker payload handoff. This controls for the second
  arrangement simply using an additional CPU.

Keep total bytes read, transform, total in-flight I/O limit, and total buffer-byte
allowance equal. Report both CPU time and elapsed time: the single-owner baseline
does not consume the same active CPU capacity merely because the allowed CPU set
has the same size. Keep backend and polling/wait behavior fixed initially; vary
them in separate comparisons rather than attributing all changes to topology.

Vary block size, useful processing work, outstanding read depth, queue capacity
and batch size in bounded sweeps. Separate cache-resident, larger-working-set,
and device-path runs; record cache mode and observed conditions rather than
labelling a file run "NVMe bandwidth" without establishing that path.

Begin within one memory domain. Repeat across observed cache relations and NUMA
domains when the host exposes them, recording actual processor and buffer placement.
On a host without multiple domains, report that comparison unrun. Synthetic
topology can test selection logic but cannot supply cross-NUMA performance evidence.

Record throughput, offered/admitted/completed work, end-to-end and queue latency,
CPU time, buffer high-water, pressure, and rundown behavior. For arrival-paced
tests, start latency at scheduled arrival; for saturation tests, name them as
closed-loop saturation tests rather than treating them as responsiveness to an
independent external source.

A generated-buffer companion can isolate transport and processing overhead, but
remains a different workload from reading the fixture file. Likewise, varying a
synthetic compute kernel is not evidence about a production parser.

The desired result is a map of the conditions under which the variants differ,
tie, or fail a constraint. It is not a winner chosen in advance, nor a universal
threshold copied from another project's benchmark.

## What this changes in the working proposal

My inference from these sources is that an application label and an execution
arrangement are different axes. A request service can use shared workers or
partitioned owners; a stream can run directly or in stages. Consequently the
candidate catalog should be tested as combinations of mechanisms, not frozen
as mutually exclusive whole-application categories.

The first experiment can use an explicit small workload descriptor and hand-authored
candidate arrangements. It need not implement the entire generic matching framework
to investigate its premises. Record the supplied constraints and the exact candidate
construction so later matching can reproduce the choices rather than invent a
different benchmark path.

Keep candidate paths isolated while they are speculative. At the experiment review,
explicitly decide which paths remain experimental, which are retained for their
measured conditions, which can share mechanisms without losing isolation, and which
are deleted. A losing variant under one workload is not proof it is useless under
another. Implementation and that review remain actions in
[CHECKLIST.md](CHECKLIST.md), not completed work in this research note.

## Limits of this pass

This survey establishes examples of patterns and contracts, not their relative
prevalence or a complete taxonomy. No benchmark was run. It does not establish
Windows equivalence to SPDK/DPDK, available local NUMA hardware, or a final client
schema. Those are experiment inputs and subsequent design work, not gaps to fill
by assuming the proposal is already correct.
