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

## One spike here establishes nothing yet

[file-handle-numa-spike.rs](file-handle-numa-spike.rs) is the exception to the table above: it is a
**ready instrument with no result**, checked in deliberately rather than held back. It asks whether a
file handle yields a NUMA node, and which question that answer answers.

It is unrun because of a **hardware gap, not a decision to defer**: it needs more than one NUMA node
and storage whose PDO advertises a proximity domain, and the machine this workspace is developed on
has a single node and reports zero `Win32_NumaNode` instances. On such a machine the spike is
vacuous in the same sense the drain spike's control case guards against -- failure would prove
nothing and success could only ever report `0`. It prints that warning itself before running.

Anyone with a multi-node server and a real NVMe or SAN volume can settle it in a few minutes, and the
result would correct a claim
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) currently makes about what is reachable from user mode.

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
