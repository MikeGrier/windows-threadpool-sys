# Spike reproducers

Standalone programs that established behaviour this crate's design notes now assert. They are
**not** built by the workspace: each is a single file written directly against `windows-sys` with no
dependency on this crate, so that what it measures is the operating system's behaviour and not ours.

To run one, drop it into a scratch binary crate with a single dependency:

```toml
[dependencies]
windows-sys = { version = "0.61.2", default-features = false, features = [
    "Win32_Foundation", "Win32_Security", "Win32_Storage_FileSystem",
    "Win32_System_IO", "Win32_System_Threading",
] }
```

| File | Establishes |
|---|---|
| [completion-event-spike.rs](completion-event-spike.rs) | [D-19](../../DESIGN-NOTES.md#d-19) -- the completion event is edge-triggered on the completion queue going empty to non-empty; also what `SetIoRingCompletionEvent` permits (call at any time, replace, clear with `NULL`, duplicate survives closing the original) |
| [drain-ordering-spike.rs](drain-ordering-spike.rs) | [D-23](../../DESIGN-NOTES.md#d-23) -- an unflagged flush does not cover preceding writes; [D-24](../../DESIGN-NOTES.md#d-24) -- `DRAIN_PRECEDING_OPS` is a full, ring-wide barrier spanning submissions |
| [write-pending-spike.rs](write-pending-spike.rs) | `M20.6` -- which handle flags make a ring write or flush *pend* rather than complete inside `SubmitIoRing`, measured as a rate over 500 trials per condition. `FILE_FLAG_OVERLAPPED` alone changed nothing. A fifth condition over a `set_len` extent was added in `M25.3`, and sixteen runs are in [measurements/2026-09-24-set-len-vs-zero-fill/](../../measurements/2026-09-24-set-len-vs-zero-fill/README.md) -- read them before quoting any single run, because they show the buffered/unbuffered split is the part that replicates and that "only the pre-written extent pends" does not. See below for why this one reports frequencies and what may **not** be built on them. |
| [set-len-zero-fill-spike.rs](set-len-zero-fill-spike.rs) | What `set_len` costs and when. Written because two statements about it were being made from documentation rather than measurement -- see [measurements/2026-09-24-set-len-zero-fill-cost/](../../measurements/2026-09-24-set-len-zero-fill-cost/README.md). `set_len` is free, and so is a write that lands at the valid data length; a write that lands *past* it pays to zero the whole gap synchronously, about eight times the cost of writing the extent outright. A sequential writer pays nothing extra for pre-setting its length. No dependencies. |

## The pending spike reports rates, and none of them is a contract

[write-pending-spike.rs](write-pending-spike.rs) exists because `epoch_log`'s strategy harness was
built on the premise that a log "keeps appending while a commit is outstanding", and measurement
showed nothing was ever outstanding: the commit's `SubmitIoRing` took 289-555 us and returned with
every completion already queued.

Its first draft ran each condition **once** and printed a verdict. That is the error `D-47` records --
the spike behind `D-24` saw a barrier hold a handful of times and wrote down a guarantee, when the
real violation rate was nearer one in a thousand. The rewrite to 500 trials per condition immediately
justified itself: the `NO_BUFFERING`-extending condition pends in about **1%** of trials, which a
handful of runs would have reported as "never".

Measured here (single node, ARM64, one device), across two consecutive runs:

| condition | pended / 500, run 1 | run 2 | submit p50 |
|---|---|---|---|
| buffered, no `OVERLAPPED` (what `epoch_log` opened) | 0 | 0 | ~510 us |
| buffered + `OVERLAPPED` | 0 | 0 | ~490 us |
| `NO_BUFFERING` + `OVERLAPPED`, extending | **5** | **271** | ~270 us |
| `NO_BUFFERING` + `OVERLAPPED`, pre-written extent | 500 | 500 | ~116 us |

**The extending row moved from 1% to 54% between two runs minutes apart**, with no change to the
program. Whatever drives it -- filesystem allocation state, cache residency, something else -- it is
not under this program's control and was not measured. That row alone would defeat any number of
repetitions of a single condition: a run reporting 5 and a run reporting 271 are both "what the
platform does", and neither is what it will do next time.

**None of this is a contract, including the two stable rows.** Windows specifies nothing about when a
ring operation completes relative to `SubmitIoRing`. A rate of zero bounds a frequency rather than
establishing that something cannot happen, and a rate of 500/500 is the same statement pointing the
other way: it may never have been false here, and it is still not contractually true.

So the obvious use of this spike is the wrong one. Reading the rows, picking the flags that pended,
and rebuilding a harness on them would bind the sample's premise to incidental behaviour -- the
failure PLATFORM INTEGRITY rule 2 names. A log, and a benchmark of one, has to be correct whether an
operation completes inline or pends. What the spike is legitimately for is explaining why a
measurement looks the way it does, and knowing which configurations are worth testing *across*.


## One spike here has only a narrow result

[file-handle-numa-spike.rs](file-handle-numa-spike.rs) is the exception to the table above: it is a
**ready instrument whose result so far is vacuous on node count**, checked in deliberately rather than
held back. It asks whether a file handle yields a NUMA node, and which question that answer answers.

It is unsettled because of a **hardware gap, not a decision to defer**: it needs more than one NUMA node
and storage whose PDO advertises a proximity domain, and the machine this workspace is developed on
has a single node and reports zero `Win32_NumaNode` instances. On such a machine the spike is
vacuous in the same sense the drain spike's control case guards against -- failure would prove
nothing and success could only ever report `0`. It prints that warning itself before running.

Anyone with a multi-node server and a real NVMe or SAN volume can settle it in a few minutes, and the
result would settle what
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) leaves open under "What is not reachable": whether either call
ever names a node that distinguishes one device from another.

It **has** been smoke-run here, which is why it compiles and why its Q5 works: the first version
opened the directory with `File::open`, which fails on a directory without
`FILE_FLAG_BACKUP_SEMANTICS`, so that question could never have been answered. Running an instrument
on hardware where its result is vacuous still validates the apparatus.

That run also settled one narrow thing worth knowing before you start: on ARM64 Windows with a single
node, **both** calls succeed on an ordinary NTFS data file and on a directory handle, and agree on
`0`. So "ordinary NTFS file" is not the no-association case; absence must come from a device layer
advertising no proximity domain, which is what needs the other hardware.

### The second unrun spike: does creation-time affinity place the stack?

[thread-stack-numa-spike.rs](thread-stack-numa-spike.rs) is the other ready-instrument-without-a-result.
It asks whether a thread created with `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY` receives a node-local
*stack*. That matters because a stack is allocated at thread creation on the creating thread's node,
so binding affinity afterwards cannot move it -- which is the entire argument for constructing domain
threads with the affinity already set rather than applying it later. The argument is currently
**assumed**, and this measures it.

Three threads discriminate the possibilities: **A** created with the affinity attribute, **B** created
with no attribute list at all (the baseline, and what `std::thread` does), and **C** created plain then
bound to the far node from inside itself -- the shape a naive consumer writes. Each reports the node of
a **shallow** stack page and a **deep** one behind a 64 KiB frame, because Windows commits stack pages
on demand and the two may be placed by different mechanisms. `shallow != deep` on either thread means
pages follow **first touch** under the running affinity rather than a decision made once at creation,
which would make the whole question subtler than the design assumes.

It reports `Valid` beside every node, because `QueryWorkingSetEx` only fills `Node` for a resident
page, and it refuses to print a conclusion when a probe was non-resident or when the machine has one
node -- rather than emitting a confident zero.

Smoke-run here, so the apparatus is proven even though the result is vacuous: the attribute list
assembles, all three threads are created, `GetNumaNodeProcessorMaskEx` returns `mask 0xfff` matching
this machine's twelve cores, all six probes come back resident, and the vacuity guard fires instead of
concluding. What remains untested is only what one node cannot show.

## Why the drain spike looks over-built

It carries a concurrency check and a control case because the first two versions of it **could not
discriminate** and would have produced confidently wrong answers:

1. Buffered writes complete in submission order, so a barrier and no barrier looked identical.
2. `NO_BUFFERING` but *extending* writes -- still identical, because the filesystem serializes
   extending writes and writes past the valid-data length.
3. `NO_BUFFERING` over a **pre-written extent** -- 28 of 32 small writes overtook large ones with no
   flags at all, which is the baseline that makes the barrier results mean anything.

The concurrency check is retained precisely so that a future run on different hardware reports
"results below are VACUOUS" rather than a false pass. A control that matches the treatment means the
harness is measuring nothing.

## Re-running these is worthwhile

Every behaviour here is **undocumented**, measured on a single machine (`IoRing` version 400, real
kernel ring, `UM_EMULATION` absent), against one device. None of it is a contract Microsoft has
published, so it could differ on another Windows build, on Server, or under the user-mode emulation
path -- and could in principle change under servicing. If you have access to different hardware or a
different OS build, running these and recording the result is a cheap contribution.
