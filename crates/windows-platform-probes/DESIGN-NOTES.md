# Design notes: windows-platform-probes

Decisions for this crate. Pending work is in [CHECKLIST.md](CHECKLIST.md); the
crate's *creation* is tracked separately in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md), milestone M27,
which is feature-scoped and deleted when that feature completes.

## A probe is a function that returns an observation, never a program that prints one

<a id="d-derived-not-restated"></a>

Each probe's logic lives in a library function returning a structured result.
The binaries print that result and the tests assert it, so the fact has exactly
one implementation.

The alternative -- a binary that prints and a test that separately re-checks --
is the restatement failure this repository has already paid for, in its most
concentrated form: a hand-written second copy of a platform check is not a check
of the platform, it is a check of the copy. When the two disagree, nothing
detects it.

## A probe reports observations parameterized by their capture; it does not draw the client's conclusion

<a id="d-observations-not-verdicts"></a>

Every figure this crate publishes is **what one machine did on one day**, and it
is recorded together with the parameters of its capture -- at minimum the probe's
host banner (architecture, logical and core counts, SMT, cache groupings,
efficiency classes, NUMA nodes), the build profile, and the number of runs with
their dispersion. A ratio quoted without those is an anecdote, not data a reader
can compare against their own hardware.

**Some capture parameters are approximated, and some are simply absent; the
difference is stated rather than elided.** Windows exposes no NUMA *distance*
table -- there is no Win32 equivalent of reading ACPI SLIT, as recorded at the
`proximity` site in
[core_affinity.rs](../windows-placement-probe/src/core_affinity.rs). What it does
expose, and what this crate uses as the best available analog, is the
**assignment of processors to NUMA nodes** -- which is what the banner's
`numa[...]` field carries, as processors-per-node. Device-to-node mapping is
obtainable on the same footing. That analog answers "are these two things in the
same domain", which is the question most placement decisions actually turn on; it
does not answer "how much further is node 2 than node 1", and no amount of
probing on Windows will. Memory configuration and BIOS state are not captured at
all.

**Reading the analog is part of reading the figure.** A banner of `numa[16]` is
a *single-domain* machine, so measurements taken on it say nothing whatever about
cross-domain behaviour -- not "a little", nothing. A figure is only evidence
about the domain structure its banner records.

**The conclusions this crate is willing to draw are coarse, mechanically
reasoned, and observation-backed** -- "the buffers should be in the same memory
domain as the executor" is the shape of a claim that earns its place, because it
follows from how the hardware works *and* the measurements agree. **Fine-grained
topological and layout choices are handed to the client, not made for them.**
Which position/reservation apportionment a queue should use is exactly such a
choice: the layout is a type parameter of the shipping queue, the probe measures
every candidate, and the note reports what it saw. It does not name a winner.

This is why the apportionment claim in the queue-contention section was
*withdrawn in both directions* rather than reversed. The measurement stopped
supporting "re-apportioning is free", but it equally did not support "it costs
30%" -- one host, seven runs, against a control that wanders. The correct output
of a probe that cannot call something is a flag saying *measure this on your own
hardware*, never a verdict chosen because a verdict reads better.

The failure this prevents is a reader inheriting a number as though it were a
property of the code. It is a property of the code **on that machine**, and the
distinction is the whole value of shipping the probe rather than only its output.

### High variance in our own control is a finding about the instrument, not just a wider yardstick

<a id="d-variance-is-a-finding"></a>

When the same code measured twice in the same run disagrees by tens of percent,
the first thing that has been measured is **the method**. It is tempting to treat
a wide control as merely a coarser ruler -- to widen the band and carry on
judging ratios against it -- and that is the mistake this decision exists to stop.
A control that wanders is a defect report against the measurement, and it is
logged as one even when the measurement is still used.

The candidate causes are not distinguishable from the dispersion alone, and all
of them are live here:

- **The wrong instrument for the variable.** A probe that moves several things at
  once cannot attribute a difference to the one under test.
- **A defect in the probe itself.** This crate has already shipped one -- the
  timing window that read the coordinator's clock rather than the producers'.
  That defect was invisible in the numbers until it was found by reading, and it
  moved high-producer figures by roughly 45%.
- **Insufficient runs or too short a duration** -- straightforward hygiene, and
  the cheapest to rule out.
- **A noisy machine.** These are fine-grained measurements taken on a shared,
  general-purpose desktop running everything else it normally runs. Scheduling,
  frequency scaling, and other tenants all land inside the timed region.

**How much this matters depends entirely on what the number is for, and that
calibration is recorded rather than assumed.** In benchmarking or marketing
literature it would be disqualifying: those documents exist to support a
comparative claim, and a comparative claim resting on a control this wide is not
supported. Here the purpose is *planning for deployment environments resembling
the measured one* -- and a figure gathered on an ordinary loaded machine is not
obviously the wrong input for planning on ordinary loaded machines. So the
honest treatment is neither to suppress the data nor to promote it: **record it,
record the dispersion beside it, and record that the dispersion is itself
unexplained.**

### What to try first, and how to tell when you have reached the floor

**The cheapest move is always to gather more of the same before gathering
anything different.** Lengthen the timed span, raise the repetition count, or
both, on the *unchanged* configuration. This costs only wall time and it
partitions the problem in one step: if the control narrows, the dispersion was
sampling noise and the previous run simply had too few samples to resolve
anything; if it does not, the width is structural and the remaining candidates
are the interesting ones. Do this before pinning threads, before quiescing the
machine, and before suspecting the probe -- each of those changes what is being
measured, and a change made before the cheap check cannot be evaluated.

**A warmup pass separates transient cost from ongoing noise, and the two are
different things.** This probe already discards one untimed pass -- though not for
the reason an earlier version of this paragraph gave. Every timed repetition
builds and drops its own queue, so the discarded pass cannot fault in any
allocation a timed pass will use; what it warms is process state, the allocator's
size class, the OS page cache, the instruction cache and the branch predictors.
Cold caches, predictors, and CPU
frequency ramp are the same *kind* of cost -- one-time, front-loaded, not a
property of the steady state -- and lengthening the timed span dilutes them
whether or not a warmup removes them.

It is worth being clear that this does **not** contradict the position that some
noise is inherent to a shared machine. A warmup removes *transients*; contention
with other tenants continues for the whole run and is not removable by any amount
of warming. The two widen dispersion for unrelated reasons, and removing the
transients is what makes the inherent floor *visible* rather than what hides it.
Expect warming and lengthening to shrink the spread to some value and then stop
shrinking it, and treat that plateau as the interesting result.

**There is always a floor, and recognising it is the skill this decision is
really about.** A measurement cannot resolve a difference smaller than the noise
in the quantity being differenced, and past that point more runs buy nothing --
continuing to gather them is how a project spends a week proving that two numbers
are the same. The floor is a real, findable property of the setup, not a failure.

**The floor is not necessarily a percentage of the measured value**, and assuming
it is will mislead you in both directions. It can be set by the sampling regime
instead: the granularity of the clock, how many independent samples the run
actually takes, or how the measured span is constructed. This probe times a whole
pass and divides -- two timestamps per worker per repetition -- so at small
absolute values the resolvable difference is governed by how many independent
passes were taken, not by any fixed fraction of the nanoseconds reported. That is
why "the 1-producer rows are noisy because the numbers are small" is a guess
rather than a diagnosis, and why the first move above is to add samples: it tests
that guess directly.

What this decision forbids is the quiet version -- reporting a wide control as
though a wide control were normal. It is not normal. It is an open question, and
where it is open, the note says so and the checklist carries the work.

## Three tiers, because "run all the probes" is not a safe instruction

<a id="d-three-tiers"></a>

Some of the behaviour worth measuring is actively hostile to an ordinary test
run. The `CancelSynchronousIo` measurement recorded in the workspace
[DESIGN-NOTES.md](../../DESIGN-NOTES.md) *never returns* -- that is the finding.
A thread-pool growth measurement spends seconds and 512 threads. An `IoRing`
measurement moves 512 MiB. A device-map measurement needs `subst` drives and a
second logon session.

So each probe declares a tier: **asserted** (a real test), **ignored**
(assertable but too slow, too heavy, or environment-dependent), or **binary
only** (cannot be a test at all). Every tier is compiled by an ordinary build,
which is the floor -- a probe that no longer compiles has already rotted.

Placing a probe in the wrong tier is the failure mode to watch: an asserted probe
that is slow or flaky will be deleted by whoever it blocks, taking the
measurement with it.

## An asserted probe must not outlive its own call

<a id="d-no-lasting-side-effects"></a>

This is the rule that decides tier membership, and it has already changed a
probe's design. The natural way to show that the thread error mode is
independent of the process error mode is to set `SEM_NOALIGNMENTFAULTEXCEPT` at
process scope and observe that it does not appear in the thread mode. That bit
is **sticky**: Windows ignores every later attempt to clear it, so the test
process would be permanently altered, and any other test in the same binary would
inherit the change.

[`thread_mode_independent_of_process`](../../crates/windows-platform-probes/src/error_mode.rs)
therefore demonstrates the same property with a **reversible** bit, which proves
exactly as much. The stickiness itself is real and worth recording, so it is
measured by
[`alignment_bit_is_sticky_at_process_scope`](../../crates/windows-platform-probes/src/error_mode.rs)
-- binary-only, and documented as irreversible at the call site.

## A probe may change process-wide state; a component may not

<a id="d-experiment-not-component"></a>

`thread_mode_independent_of_process` calls `SetErrorMode`, which is
**process-scoped**: for the length of that call the whole process carries a mode
it did not ask for, and every other thread in it sees that.

That is unacceptable in a library, and the rule is general rather than a fact
about this bit: **a component does not change process-wide state unilaterally.**
Process-wide state belongs to whoever owns the process. A component that mutates
it is making a decision on behalf of code it has never heard of, and one that
restores it afterwards has only narrowed the window, not acquired the right.

This crate does it anyway, and the tension is resolved by what this crate *is*.
It is an experiment for discovering platform behaviour, not a component: the only
way to learn whether the thread mode is a view of the process mode is to move the
process mode and look. `publish = false` and version `0.0.0` are the enforcement
-- nothing ships this, and nothing outside the workspace can depend on it. The
call site says the same thing in its own rustdoc, because a reader arriving at
the function will not have read this file.

### The concurrency hardening is knowingly declined

A review proposed two changes: set `previous | bit` rather than `bit`, so the
probe does not briefly clear unrelated process bits; and serialize the mutation
behind a process-wide lock.

Both are technically right, and the second addresses a real defect rather than a
hypothetical one. Two overlapping calls can interleave so that the second saves a
value the first had already installed:

| step | thread A | thread B | process mode |
|---|---|---|---|
| 1 | `previous = SetErrorMode(bit)` saves the entry mode | | `bit` |
| 2 | | `previous = SetErrorMode(bit)` saves **`bit`** | `bit` |
| 3 | `SetErrorMode(entry)` | | entry |
| 4 | | `SetErrorMode(bit)` | **`bit`**, permanently |

The entry mode is lost and the probe's bit is left installed for the life of the
process. Neither is a reason to change the code, and it is left as it stands.

The reason is that hardening it would send the wrong signal. A locked,
non-clobbering version of this function looks like something safe to call, and
the one thing that must never happen is a component reaching for it because it
appeared fit for use. It is not fit for use; it is an experiment. The honest
response to "this is unsafe to call concurrently" is to say plainly that it must
not be called from production code at all, which is what the rustdoc now does --
not to make the process-wide mutation *tidier* and leave the real objection
standing.

Recorded so the suggestion is not re-raised, and so nobody "fixes" it into
something that looks reusable. If a *component* ever needs this behaviour, the
answer is not this function hardened; it is a design conversation about who owns
the process mode.

## A probe asserts its own controls, and refuses to pass vacuously

<a id="d-controls"></a>

Two guards, both from measurements in this repository that read as passes while
measuring nothing.

**A control is part of the probe, not a separate courtesy.** "A duplicated handle
continued the enumeration" says nothing on its own -- perhaps any handle
continues. The paired measurement, that two *separate opens* do not continue each
other, is what makes the first attributable, so it is asserted as its own test
rather than left to a reader's inference.

**A fixture that cannot exhibit the behaviour is a failure, not a pass.**
[`ground_truth`](../../crates/windows-platform-probes/src/handle_state.rs) panics
if the whole directory fits in one call, because every cursor question below it
would then be vacuous. The buffer is small deliberately, and the assertion is
what keeps it small when someone later "tidies" the constant.

## Scope

Measurements of **platform** behaviour that a design decision rests on. Not a
test suite for this workspace's crates: a probe answers "what does Windows do?",
never "does our code work?". A probe that starts asserting our own behaviour
belongs in the crate that owns that behaviour.

**One carve-out, and it is narrow.** A probe may compare a workspace crate's
reading of the platform against a *second, independent* reading this probe takes
itself -- see [the topology cross-check](#d-topology-three-lists), which reads
`GetActiveProcessorCount` and friends directly and compares them with what
`windows-topology-sys` parsed out of `GetLogicalProcessorInformationEx`. The
subject is still the machine; the crate's parse is one of two readings of it,
and a divergence is a finding about the platform-facing code precisely because
the second reading came from the platform.

What makes it belong here rather than in the owning crate is the thing the
owning crate's own tests cannot get: CI runs these probes across a
heterogeneous hosted-runner fleet, so the comparison happens on machines nobody
enumerated in advance. A unit test in `windows-topology-sys` can only assert
against topologies someone thought to construct.

The boundary this preserves: the probe never asserts a value it did not read
from the platform. It has no expected core count, no golden topology, no
knowledge of what the crate *should* have said -- only two readings and whether
they agree. A probe that grew a hard-coded expectation would be back on the
wrong side of the rule above.

## The earlier probes are migrated, and two of them corrected in the move

<a id="d-migration"></a>

M27.4 moved the nine measurements that existed only in a git-ignored
`.scratch/` directory. They are now `worker_context`, `pool_growth`,
`device_map`, `cancel_io`, and `ioring`, alongside the `error_mode` and
`handle_state` probes that established the scheme.

Two things changed in the move, and both are worth recording because they were
defects in the originals rather than translation choices.

**The device-map control could not have passed.** It compared the logon-session
LUID of the two contexts, reading the *thread* token -- which the
non-impersonating side does not have. So the control reported "these are not
different sessions" no matter what, and a reader checking it would have been
misled into distrusting a correct finding. It now falls back to the process
token, and the two LUIDs differ as they should. A control that cannot succeed is
worse than no control, because it looks like one.

**The `IoRing` registration probe must not use `windows-ioring-sys`.** That
crate refuses a second registration *because of* the assumption being measured,
so probing through its safe API would test the guard rather than the platform --
confirming our own belief by consulting it. The probe therefore calls the Win32
entry points directly. This is the same circularity the contract-integrity rule
names, in the one place where it would have been easiest to miss: the code under
test and the thing asserting it would have been the same claim.

That probe also closes a standing gap. `windows-ioring-sys` recorded its
replace-not-append assumption as explicitly **unverified**; it is now measured,
and it holds.

## "Cannot measure" is a third answer, and is not "no"

<a id="d-cannot-measure"></a>

Several probes need something a host may not have: an `IoRing`, a free drive
letter. Each reports that it could not run rather than returning a negative,
because conflating the two is exactly how a design note ends up citing a
measurement that never happened.

The distinction has teeth in the ignored tier. A test that cannot set up its
fixture returns early; a test whose fixture *is* set up but does not exhibit the
behaviour **fails**. `the_subst_letter_really_was_visible_before_impersonating`
is that check written down: if the drive never resolved in our own session, then
"not found while impersonating" would be true of any letter at all, and the
finding would be vacuous.

## Binaries print magnitudes; tests assert shape

<a id="d-shape-not-magnitude"></a>

The pool-growth migration made the reason concrete. Growth is **not uniform**:
on the measuring host an initial burst of workers arrives in under 500
microseconds, and beyond that the pool adds roughly one thread per 165
milliseconds. A design sizing a stall threshold from the burst would be badly
wrong about the tail.

Neither number belongs in an assertion -- both are host-specific, and a test
pinned to them would fail on the next machine for no useful reason. The *shape*
is what survives a change of host, so the test asserts that there is a burst
followed by a visibly slower regime, and `probe-pool-growth` prints the
magnitudes for a human to read.

## The completion-port fork: two probes disagreed, and the corrected one was right

<a id="d-completion-port"></a>

M27.6 was split out of the migration because the original Probe D exists in two
versions that reach opposite conclusions. Settling it was measurement work, not
a port, and the result is worth recording in full because a shipped decision
rests on it.

The first version declared **COEXIST** on seeing a completion arrive on the
ring. That completion's result code was `ERROR_INVALID_PARAMETER` and its byte
count was zero. It had checked *where the completion arrived* rather than
*whether the operation succeeded*, and so read a clean refusal as a success.

Measured on Windows 11 Enterprise 10.0.28000, `aarch64-pc-windows-msvc`, the
corrected reading holds:

| Case | Result |
|---|---|
| control: no association | `0x00000000`, 4096 bytes, fill byte -- **pass** |
| after `CreateIoCompletionPort` | `0x80070057`, 0 bytes -- **refused** |
| before a late association | pass |
| after that late association | `0x80070057`, 0 bytes -- **refused** |
| control: overlapped read via the port | pass -- the handle is still healthy |
| before `CreateThreadpoolIo` | pass |
| after `CreateThreadpoolIo` | `0x80070057`, 0 bytes -- **refused** |

So association forecloses `IoRing` use of the handle, and the port control shows
it is the ring path specifically that is refused rather than the handle being
broken. **`CreateThreadpoolIo` does the same**, which matters more than the raw
`CreateIoCompletionPort` case because it is the path this workspace actually
uses -- the consequence lands on `windows-threadpool-sys`'s own users.

This is the evidence behind `windows-namespace-request-sys` returning an opened
handle plain and unassociated: the association cannot be undone, so a layer that
made it on a caller's behalf would silently remove a capability.

### A read is judged on three fields, and the fixture is not zero-filled

Both choices exist because of how the first version failed. A read passes only
when the result code is success, the byte count is the full length, **and** the
buffer holds the fill byte -- and the fixture is filled with `0xAB` rather than
zeros precisely so "nothing was written" cannot be mistaken for "zeros were
read". A zero-filled fixture would make the third check vacuous.

`a_failed_ring_read_is_judged_on_more_than_where_the_completion_arrived` pins
this: it asserts the refusal is visible in every field, so the original mistake
cannot be repeated without failing a test.

### Probe fixtures are unique per instance, not per label

Migrating this found a latent fault in the shared `IoRing` fixture: its path was
keyed by process id and label, so two tests running concurrently in one process
shared a file. The second's write hit a sharing violation against the first's
open handles -- and a probe reporting a fixture failure *looks like the platform
refusing something*, which is the worst possible failure mode for a measurement.
The path now carries a per-instance counter.

## The thread-agnosticism probe measured nothing, twice over

Found by the M24-M27 code review, and worse than the review reported. The
finding itself survives re-measurement -- an `IoRing` operation really does
outlive the thread that submitted it -- but neither of the reasons the original
probe gave for believing it was sound.

### `PopIoRingCompletion` reports an empty queue with a success code

`PopIoRingCompletion` returns `S_FALSE` when the completion queue is empty.
`S_FALSE` is `1`: a *success* `HRESULT`. The probe's `pop` tested `hr >= 0`,
the usual shape for an `HRESULT`, so an empty queue was indistinguishable from
a popped completion -- and the value handed back was the zeroed `IORING_CQE`
the call had left untouched, whose `ResultCode` field is `0` and therefore
reads as **a successful operation**.

So the probe submitted a read, immediately popped a phantom completion, read
`ResultCode == 0`, and reported success. It would have reported exactly that
without a ring doing any work at all. Measured directly:
`PopIoRingCompletion` on an empty ring returns `0x00000001` with
`ResultCode=0x00000000, Information=0`.

`pop` now tests `hr == S_OK`.

### The read had already completed anyway

Independently, the probe read 512 bytes from a small cached temp file. Measured
on this workspace's hardware, that read completed *inside* `SubmitIoRing` on 8
runs out of 8, so the operation was already finished before the submitting
thread returned. Even with a correct `pop`, such a run says nothing about
thread affinity: a completed operation is collected afterwards however
thread-affine the platform is.

The probe now reads from a **pipe with nothing written to it**, which cannot
complete until the probe chooses to write. The sequence is: submit; confirm
from the submitting thread itself that no completion is available; let that
thread exit; only then write. The pending state is controlled rather than
hoped for.

`submitter_exited` -- which was a hard-coded `true`, making the test's own
guard `assert!(true)` -- is replaced by `pending_at_submitter_exit`, which is
observed. That the submitter exited was never worth recording: the probe joins
it, so it is true by construction.

### Re-measured, the finding holds

With both faults fixed: `pending_at_submitter_exit=true`, `result_code=0`,
and the fill byte actually transferred, on 8 runs out of 8. Sabotaged (by
filling the pipe *before* submitting, so the read can complete early) the
probe reports `pending_at_submitter_exit=false` and the test fails -- so the
new guard is load-bearing rather than decorative.

### The cost of not consulting our own crate

This module calls Win32 directly rather than using `windows-ioring-sys`, on
the deliberate ground that probing through that crate's safe API would confirm
our own belief by consulting it. That decision is still right, and this is the
price of it: `windows-ioring-sys` had already got `S_FALSE` right
([ring.rs](../windows-ioring-sys/src/ring.rs), `try_pop`) *and* recorded it in
its own [DESIGN-NOTES.md](../windows-ioring-sys/DESIGN-NOTES.md), and the
duplicated `pop` here got it wrong regardless.

The lesson is not "consult the crate" -- that would reintroduce the
circularity. It is that a probe re-implementing a primitive owes that
primitive the same scrutiny as the platform behaviour it is measuring, because
a defect in the re-implementation is indistinguishable from a platform finding.

## A collect that cannot give up, and what that costs

<a id="d-collect"></a>

Every read path in [ioring.rs](src/ioring.rs) and the completion-port read in
[completion_port.rs](src/completion_port.rs) frees its buffer when it returns.
An operation the kernel has accepted owns that buffer until it completes, so
there is no safe early return from the collection step: abandoning a
still-outstanding operation leaves the kernel writing into freed heap, and in
the completion-port case into a freed stack frame as well. `Ring::collect` and
the cancel-then-drain loop therefore **do not give up**. They wait inside the
kernel and recheck, rather than spinning, but no timeout releases them.

The decision that makes this affordable is the one *above* it: a caller only
enters the collection step when the ring reported entries as actually
submitted. `submit_and_wait` returns that count alongside its `HRESULT`
precisely so the two cases can be told apart -- a failing `HRESULT` does not
mean nothing was queued, because the wait can time out with entries in flight.
When the count is zero nothing owns the buffer and the caller returns
immediately.

### The cost: an operation that never completes hangs rather than reports

This is a real trade and worth stating plainly. If a submitted operation never
produced a completion at all, the probe would hang, and in CI it would consume
the job's time budget instead of reporting a failure -- which is the wrong
failure mode for a crate whose whole purpose is reporting what the platform
does.

Two things make that acceptable rather than merely tolerated:

- **The negative result this probe exists to detect does not take that path.**
  A submitted `IoRing` operation yields a CQE in every terminal case,
  including cancellation, so a platform that *did* cancel the IRP when the
  submitting thread exited would surface a completion carrying a failure code
  and `survives_submitter_exit()` would report `false`. The finding being false
  is detectable; it is not the hanging case.
- **The alternative is worse.** Giving up and returning is memory corruption,
  not a lesser bug.

If a genuinely-never-completing operation is ever encountered, the fix is not
to add a timeout to `collect`: it is to leak the buffer deliberately
(`mem::forget`) on the give-up path and report a "could not collect"
observation, so the kernel's writes land in memory nothing will reuse and the
probe can still say what happened. That is recorded here as the known escape
hatch, **not** as scheduled work -- nothing in this workspace has encountered
the case, and building the machinery now would be speculative.

## The x64 comparison: no finding is architecture-dependent

<a id="d-x64"></a>

Every measurement in this workspace was originally taken on ARM64, which left a
standing question: how much of it was a property of Windows, and how much a
property of that machine? The `platform probes (x64, ...)` CI job answered it on
the first run.

Measured on the two hosts:

- **ARM64** -- Windows 11 Enterprise 10.0.28000, `aarch64-pc-windows-msvc`.
- **x64** -- GitHub `windows-latest` runner, `x86_64-pc-windows-msvc`.

| Finding | ARM64 | x64 | Same? |
|---|---|---|---|
| settable `SEM_` bits | all but the alignment bit, rejected with error 87 | identical | yes |
| an invalid bit costs every valid bit | whole call fails, nothing installed | identical | yes |
| thread vs process error mode | independent storage | identical | yes |
| the alignment bit is sticky at process scope | restore ignored | identical | yes |
| a duplicate shares the enumeration cursor | continues where the source stopped | identical | yes |
| control: separate opens are independent | second open restarts | identical | yes |
| closing a duplicate | source keeps enumerating | identical | yes |
| interleaved single-shot queries | all four undisturbed | identical | yes |
| a worker inherits no token | `ERROR_NO_TOKEN` | identical | yes |
| a worker's critical-error handler | enabled | identical | yes |
| impersonation changes the device map | letter resolves in our session, not anonymous | identical | yes |
| `IoRing` registration | replaces the table | identical | yes |
| `IoRing` thread agnosticism | survives its submitter | identical | **evidence void, see below** |
| IOCP association vs `IoRing` | forecloses, `0x80070057` | identical | yes |
| `CreateThreadpoolIo` vs `IoRing` | forecloses the same way | identical | yes |

**Every qualitative finding held.** Nothing in this workspace's designs rests on
an ARM64 peculiarity.

One row above is marked **evidence void**. The thread-agnosticism probe was
measuring nothing on *either* architecture when this comparison was run, for
the two reasons given in the section above, so "identical" recorded only that
both architectures produced the same phantom completion. The finding was
re-measured with a corrected probe and holds; what that row cannot claim is
that the original comparison established it. Rewriting the row to say it did
would be the kind of tidy history this file exists to prevent.

### The magnitudes differ, within noise, and the shape is what was asserted

Only the pool-growth timings moved, and only in scale:

| Measure | ARM64 | x64 |
|---|---|---|
| burst arrivals (max 8) | 249, 393, 468, 483 us | 212, 315, 460, 538 us |
| throttled gaps | ~158-167 ms | ~163-252 ms |
| slowest arrival (max 8) | 651 ms | 751 ms |
| raise 2 -> 6 while saturated | ~1.8 ms | ~1.6 ms |

The two-regime shape -- a burst of roughly four, then one thread per throttle
interval -- is identical, which is exactly what the ignored tests assert and why
they assert shape rather than numbers. Pinning the 165 ms would have failed here
for no useful reason; the x64 host is a shared CI runner, so slower and noisier
tails are expected rather than interesting.

### One prediction was wrong, and it is worth recording

The checklist warned that `IoRing` might report `Unavailable` on the runner --
`CreateIoRing` needs a recent build, and an older image would leave three
findings unanswerable. It did not: `windows-latest` has a usable ring, and all
four `IoRing`-dependent probes ran and agreed. The caution was reasonable and
the outcome was better than it.

### The one CI failure was a test defect, not a platform difference

`build + test` went red on
`final_path::tests::a_directory_resolves_to_its_own_path`, which compared a
resolved path against `std::env::temp_dir()` as text. The runner's temp path
comes back in 8.3 short form (`C:\Users\RUNNER~1\...`) while
`GetFinalPathNameByHandleW` with `FILE_NAME_NORMALIZED` returns the long form.
Same directory, different strings.

It had passed locally only because that machine's user name is exactly eight
characters, so no mangling occurred -- an accident of environment, not of
architecture. It would fail on any host with a longer user name, on either
architecture. Recorded here because it is exactly the kind of result this
comparison exists to classify correctly: a red build that is **not** a finding.

## The queue-contention probe, and why it must not run in the CI probe job

`probe-queue-contention` measures two things a design decision is waiting on: whether the bounded
array queue's tail claim contends badly enough to justify the linked and sharded MPSC shapes, and how
[`reserving_mpsc`](../windows-waitable-queues/src/reserving_mpsc.rs) and `slotwise_mpsc` compare end
to end in the regime where `reserving_mpsc`'s extra read of the consumer's position is most expensive.

**An end-to-end comparison, and deliberately nothing finer.** Two earlier wordings of this sentence
were both wrong: the first said the probe *prices* that read, the second said it *bounds* it from
above. Neither holds. Writing `R` and `S` for the two shapes' total push costs, `R - S` contains the
read plus the differences in claim protocol, slot metadata and retry behaviour, and those terms are
not ordered -- in the **isolated** regime `reserving_mpsc` is several times faster despite doing the
extra read (55.5 against 207.2 ns/push at sixteen producers in one run), so the other terms can be
large and negative. A difference that can go either way bounds the read in neither direction, and in
the drained regime which shape leads varies between runs on this host, so even its sign is not a
finding. Isolating the read would need a matched control this probe does not have.

**The checklists carrying those decisions are not in this repository yet** -- they arrive with the
rest of the queue work -- so this note deliberately names the QUESTIONS rather than linking to items
that would dangle. The probe is the instrument; it is useful before the plan that consumes it lands,
and it is landed first precisely so the decision is made against measurement rather than argument.

**It is deliberately absent from the `platform-probes` CI job, and the reasons are a core count and a
clock rather than a preference.** A contention curve needs more cores than a hosted runner has: the
sixteen- and thirty-two-producer rows on a four-core runner would measure the scheduler and report it as
contention. And the run costs about 65 seconds, against a job whose other probes are seconds apiece.

**It must be run in release, which is a measurement and not a preference.** In a debug build
`slotwise_mpsc` and `reserving_mpsc` come out at 249.7 and 254.0 ns/push at sixteen producers --
indistinguishable. In release, on the same machine in the same minute, 193.5 and 52.2. The un-inlined
overhead of a debug build swamps the cache-coherence effects that *are* the finding, so a debug run does
not merely lose precision: it reports the two shapes as equivalent, which is a confident wrong answer of
exactly the kind this crate's `doorbell_cost` notes warn about.

**Those four figures predate a correction to the timing window and have not been retaken.** The
qualitative finding is unaffected -- a debug build still swamps the effect -- but the numbers themselves
were measured while the probe timed from this thread's clock rather than from the producers' own, which
overstated throughput at high producer counts. Measured on `x86_64 16p/8c` after the correction:
`reserving_mpsc` at sixteen producers moved from 35.0 to a median of 52.3 ns/push across seven runs
(46.3-55.6). The move is larger than that shape's own run-to-run spread on this host, so the direction
is not in doubt; the magnitude is a single host's observation. Any figure in this note taken before the
correction should be read as optimistic until retaken.

**An earlier version of this paragraph put that run-to-run spread at "2-6%", which seven runs do not
support** -- the same shape and configuration ranges 18% at sixteen producers, and the layout rows below
range considerably wider. The 2-6% figure came from comparing two runs, which cannot measure a spread; it
is corrected here rather than quietly dropped because several conclusions in this note were written
against it, and one of them did not survive the correction (see the layout section below).

**That is a constraint on HOW it runs, not an argument for keeping it out**, and an earlier draft of this
paragraph confused the two -- it said the CI job "runs `cargo run` without `--release`", which is not true
of the job it describes: `probe-doorbell-cost` and `probe-request-cost` already run there with `--release`,
under a comment establishing exactly the rule this probe would fall under. It also said "unlike every other
probe", and `probe-cancel-io` is likewise absent. Corrected by a review. The release precedent exists; what
keeps this one out is that it costs an order of magnitude more than the two probes that use it, on hardware
that cannot answer the question anyway.

So this one is run by hand, on a known machine, and its numbers are recorded with the machine attached.

### Reading it

Two regimes, and the pair is the point.

**Isolated** gives producers a capacity large enough that nothing is ever refused and runs no consumer, so
whatever curve appears against N is the producer side alone, with no consumer traffic in it. It is not
the claim alone -- what is timed is each shape's whole push path, tail claim and slot write and
publication and doorbell together, so a difference here is a difference in PUSH COST rather than
evidence about the claim on its own. **Drained** runs a consumer popping
continuously, which is the regime in which `reserving_mpsc`'s read of `head` is most expensive -- that
read is
cheap until a consumer is *writing* the line, and measuring it in isolation would report it as free.
It neither isolates that read nor bounds it: the ratio is between two complete push paths whose other
differences are not ordered.

The drained regime has a **single** consumer, because that is what MPSC means, so at high producer counts
it becomes consumer-bound and a plateau there says nothing about the claim. Each row carries the refusal
count from the queue's own `Observable` counters precisely so that is visible as a fact rather than
mistaken for contention: the sixteen- and thirty-two-producer drained rows show millions of refusals and
should be read as measurements of the consumer.

## The claim word's width costs 1.1x to 3.8x in isolation, and the drained figure is withdrawn

Measured by `probe-queue-contention` on one host, `x86_64-pc-windows-msvc`.
Four apportionments of `reserving_mpsc`'s claim word: 32/32, 16/48 and 8/56 over
`AtomicU64`, and 64/64 over `AtomicU128`. The last is measured only where a
128-bit exchange is native -- x86-64 and aarch64 -- so on a target without one
the report carries the other three and leaves its column empty.

**These were duplicated scaffolding when the measurement was taken, and they
ship now.** The layouts were built as copies so the shipping crate was not
disturbed while the question was open; the measurement below is what closed it,
and they are now
[`ClaimLayout`](../windows-waitable-queues/src/reserving_mpsc.rs) with
`Balanced`, `Enduring`, `Perpetual` and `Wide` as its implementations -- which is
what this probe imports. Recorded because the original wording still described
the scaffolding, and a reader who went looking for `claim_layout.rs` would not
find it.

`AtomicU128::is_always_lock_free()` is **true** on this target and
`cfg(target_feature = "cmpxchg16b")` is enabled by default, so the 128-bit
exchange is a compile-time-guaranteed native instruction here and no CPUID
branch was measured as though it were the algorithm.

| producers | 16/48 vs 32/32 (isolated) | 64/64 vs 32/32 (isolated) | 64/64 vs 32/32 (drained) |
|---|---|---|---|
| 1 | 1.14x | 2.05x | 1.05x |
| 4 | 1.21x | 1.37x | 1.12x |
| 8 | 1.00x | 2.33x | 1.07x |
| 16 | 0.88x | 2.37x | 1.00x |
| 32 | 0.98x | 2.99x | 1.11x |

**Re-apportioning the bits looked free here, and that reading was withdrawn.**
The reasoning was that both layouts issue the same `lock cmpxchg` on the same
`u64`, so only the shift and mask constants differ, and the table above was read
as confirming it. The table cannot carry that weight: these are single-run
figures, and the same-code control measured later ranges 0.69-1.27x, which is
wider than most of the differences being called "noise" -- note that this very
table has 16/48 at 1.14x and 1.21x while the prose beneath it says "within
noise". See
[Re-measured on the shipping type](#d-queue-layout-observations)
below for the seven-run figures and the withdrawal. What the re-apportionment
buys is not in dispute: the recurrence moves from 2^32 to 2^48, from about 37
seconds of sustained maximum-rate pushing to about 28 days.

**Widening the word is not free in the isolated regime, and the drained figure
below does not survive the re-measurement.** Isolated, where no consumer touches
the queue, `cmpxchg16b` cost 2-3x on the stand-in and the penalty *grows* with
contention; that is the one conclusion in this section the seven-run
re-measurement strengthened, to 3.45x and 3.81x at sixteen and thirty-two
producers. (These are shares of total push cost, not of the exchange: the
isolated regime times the whole push path, and only the layout differs between
these rows.) The drained figure of 5-12% is **withdrawn** -- not because the
number moved, but because nothing was measuring whether it meant anything. The
re-measured drained 128-bit rows run 2-13%, which resembles the old figure
closely enough to look like confirmation, while every one of them sits inside a
same-code control spanning -32% to +27%. A number that agrees with its
predecessor is not thereby established; that is precisely the trap the control
exists to catch, and this is the case where it catches it.

### The drained regime flatters the slower layout, and the refusal counts say so

The two regimes must not be averaged, and the drained one must not be read as
the answer on its own. **A slower producer is less backpressured**, so it earns
fewer refusals, and refusal retries are inside the timed region. At eight
producers the 64/64 layout took 12,149 refusals against 32/32's 74,181 -- so
part of what makes its per-push number look close is that it spent less time
being turned away. The drained figures are therefore an *understatement* of the
128-bit word's cost, not a measurement of it under load.

The isolated regime is the clean measurement of the claim itself; the drained
one shows that in a queue doing real work the claim is not the dominant cost. A
real application sits between them, nearer the drained end the more
consumer-bound it is.

### What the control caught

The first run reported 3.7x against the shipping shape and a completely
different scaling curve. The cause was that the duplicate had not padded `head`
and the claim word onto separate cache lines, which `reserving_mpsc` does
deliberately -- every producer reads `head` on every push, so sharing a line
puts the consumer's writes in their path. Aligned, the duplicate tracks the
shipping shape's curve.

A residual gap remains: the duplicate runs about 1.26x slower than
`reserving_mpsc` at high producer counts. That offset applies equally to all
three layouts, so the ratios above stand, but it means these figures are **not**
absolute numbers for the shipping shape and must not be quoted as such.

**Comparing a duplicate against the original it stands in for is what made both
of these visible.** A run of three layouts that agreed with each other and
disagreed with reality would have looked entirely healthy.

### What each apportionment actually buys

The rollover figures for candidate splits, computed from the rates above. The
rate model reproduces the crate's own published figure -- 32/32 at 116M/s gives
37 seconds, which is what `reserving_mpsc`'s module documentation discloses -- so
these are an extension of that disclosure rather than a competing estimate.

| split (reserved/position) | max outstanding reservations | @257M/s | @116M/s | @33M/s |
|---|---|---|---|---|
| 32/32 (ships) | 2^32 | 17 s | 37 s | 2.2 min |
| 24/40 | 2^24 | 71 min | 2.6 hr | 9.2 hr |
| 21/43 | 2^21 | 9.5 hr | 21.1 hr | 3.1 days |
| 20/44 | 2^20 | 19.0 hr | 42.1 hr | 6.1 days |
| 16/48 | 2^16 | 12.7 days | 28.1 days | 98 days |
| 12/52 | 2^12 | 202 days | 449 days | 4 yr |
| 8/56 | 2^8 | 9 yr | 20 yr | 69 yr |
| 64/64 (`u128`) | 2^64 | 2,270 yr | 5,039 yr | 17,607 yr |

Rates: 257M/s is the measured isolated peak at one producer, which has no
consumer and so is not a rate any draining queue can sustain -- it is a
conservative floor on time-to-wrap. 33M/s is the measured drained rate at one
producer. 116M/s is the crate's own disclosed figure and is the honest planning
number.

**The reservation half is where the bits are being spent, and it is the half
worth least.** Outstanding reservations are bounded by how many producers are
mid-flight -- hundreds, perhaps thousands -- and the field currently holds four
billion. Giving up reservations nobody will allocate is what buys the position
bits: 2^21 reservations leaves about a day, 2^12 leaves over a year, and 2^8
leaves twenty years. The last reaches the same practical headroom a 128-bit word
gives, on a plain `AtomicU64`, without a third-party dependency and without
reopening `D-18`'s i686 question.

**This paragraph previously added "at no measured cost", and that clause is
withdrawn** -- it was the same claim the layout section below withdrew, restated
a third time in a section about counter arithmetic rather than about speed. The
arithmetic above is unaffected, because time-to-wrap follows from the field width
and a rate, not from a measurement of either layout; what does not follow is any
statement about what the re-apportionment costs to run. See
[Re-measured on the shipping type](#d-queue-layout-observations).

So the candidates worth considering are **12/52 and 8/56**, not the 16/48 first
sketched here: 16/48's 12.7 days at the conservative floor is still reachable by
a busy long-lived process, and 12/52 is the first row that is not.

### Re-measured on the shipping type, with the probe's own control to read it against

<a id="d-queue-layout-observations"></a>

`CW-1.6` deleted the duplicated protocol in this crate once
`windows-waitable-queues` took the layout as a parameter, so the probe now
instantiates the real type at each layout. The numbers below supersede the ones
above, which were taken from the stand-in.

**Read every figure here as one host's observation, not as a portable result.**
The capture parameters are the probe's own banner, reproduced in full because a
ratio without them is an anecdote rather than data someone else can use:

```
host:  x86_64 16p/8c smt+ L2[2,2,2,2,2,2,2,2] ec[0:16] numa[16]
```

Seven runs, median of the per-run ratios with the observed range beside it,
release build. **The sampling parameters are capture parameters too**: each run
is a whole probe invocation, within which every configuration is measured five
times and the median reported, each measurement being 50,000 pushes per producer
thread, preceded by one untimed pass. That pass does **not** pre-touch any
allocation a timed pass will use -- every repetition builds and drops its own
queue -- so what it warms is process state: the allocator's size class, the OS
page cache, the instruction cache and the branch predictors. So a figure below
rests on 35 timed passes per
configuration, and "seven runs" alone would not let anyone reproduce it. These
are fixed at
[src/queue_contention.rs](src/queue_contention.rs)`::PUSHES_PER_PRODUCER` and
`REPETITIONS`; M4.2 in [CHECKLIST.md](CHECKLIST.md) makes them adjustable, which
is what the first diagnostic step above needs and cannot currently do.

**The banner's `numa[16]` is load-bearing here: it means a single
NUMA node holding all sixteen processors**, so every figure below was taken
inside one memory domain and says nothing about cross-domain behaviour. What is
not pinned down at all is memory configuration and BIOS state; NUMA *distances*
are unavailable on Windows by platform limit rather than by omission, and the
processor-to-node assignment in the banner is the analog this crate uses in their
place (see [A probe reports observations parameterized by their
capture](#d-observations-not-verdicts)).

The layout is a *parameter* of the shipping type, so this note's job is to report
what this machine did and hand the reader the tooling -- the probe -- to measure
the machine they actually care about. It is not to pick a winner on their behalf.

**The probe emits its own noise control, and it is the only honest yardstick for
these ratios.** The `reserving_mpsc` row and the `reserving(32/32)` row are the
same code at the same layout, measured twice in the same run, so their ratio is
what "no difference" looks like on this host:

| regime | same-code control (`reserving_mpsc` vs `32/32`) |
|---|---|
| isolated | median 0.94-1.05x, observed 0.69-1.12x |
| drained | median 0.98-1.07x, observed 0.68-1.27x |

So a ratio inside roughly 0.9-1.1x is indistinguishable from zero effect here,
and at sixteen and thirty-two producers the control alone wanders past 1.12x.

**That control is far too wide, and saying so is part of reporting it.** Two
measurements of *the same code in the same run* should not differ by 27%, and
the same-configuration spread across seven runs reaches 61%. Used above as a
yardstick, this is the honest yardstick available -- but a yardstick this elastic
is first a defect report against the probe, not a fact about the queue. The cause
is not determined: it could be the probe measuring more than the variable under
test, a residual defect like the timing window already found and fixed here, too
few runs or too short a measured span, or simply that these are nanosecond-scale
measurements taken on a shared desktop that is doing other things. The dispersion
alone cannot distinguish them, and this note does not guess. See
[High variance in our own control is a finding about the
instrument](#d-variance-is-a-finding) for what to try first and how to recognise
the floor, and M4.2 in [CHECKLIST.md](CHECKLIST.md) for the probe controls that
make those steps executable without a source edit.

What follows is therefore reported as *data with a known-unexplained spread*,
which is a reasonable input for planning a deployment on comparable hardware and
an unreasonable basis for a comparative claim about the layouts.

| producers | 16/48 vs 32/32 | 8/56 vs 32/32 | 64/64 vs 32/32 |
|---|---|---|---|
| 1 | 1.00x [0.74-1.00] | 1.00x [0.67-1.04] | 1.37x [1.16-1.57] |
| 2 | 0.94x [0.89-1.05] | 0.96x [0.80-0.98] | 1.13x [1.02-1.15] |
| 4 | 0.96x [0.83-1.03] | 1.00x [0.90-1.10] | 1.29x [1.14-1.36] |
| 8 | 1.01x [0.95-1.13] | 0.94x [0.92-1.08] | 1.82x [1.64-2.20] |
| 16 | 1.23x [1.09-1.35] | 1.26x [1.16-1.33] | **3.45x [2.91-4.27]** |
| 32 | 1.30x [1.15-1.41] | 1.28x [1.11-1.42] | **3.81x [2.70-4.31]** |

In the drained regime nothing separates at all -- every u64 layout *and* the
128-bit word sit inside the control band at every producer count (the widest
median is 1.13x at one producer, against a control that reaches 1.27x).

**Widening the word is the one effect this probe establishes.** At sixteen and
thirty-two producers the isolated 128-bit rows sit three to four times the
64-bit rows, an order of magnitude outside anything the same-code control does.
That is a real effect on this machine, and its direction is mechanically
unsurprising -- `cmpxchg16b` against `lock cmpxchg`. Whether it reproduces on
another microarchitecture is a question for the probe, not for this note.

**The apportionment claim is withdrawn, in both directions.** This section
previously said the `u64` re-apportionments "track the default within noise" and
that twenty years of headroom is therefore "free". That was asserted from a
single run against a noise floor quoted as 2-6%, and neither half holds: the
measured control is far wider than 2-6%, and the re-apportionments do not sit
inside it at sixteen and thirty-two producers. But the replacement is *not* the
opposite claim. 1.23-1.30x against a control that itself reaches 1.12x is a
flag, not a finding -- it says this is the configuration worth measuring on your
own hardware before choosing, and it says this probe, on this host, at seven
runs, could not call it. A client who needs the headroom should measure the
layouts on their target rather than inherit either verdict from here. This is
[the rule for what this crate concludes](#d-observations-not-verdicts) applied to
the case that earned it.

**The residual offset is gone, which is the point of the deletion.** The
duplicate ran about 1.26x slower than `reserving_mpsc` at high producer counts,
an error that had to be carried as a caveat on every figure. That the same-code
control now sits on 1.00x is what says the offset is gone -- and building that
control into the probe's output, rather than asserting a noise floor in prose,
is what let every ratio above be read honestly.

The general lesson is worth keeping even though the duplicate is gone:
**a stand-in is only evidence about the thing it stands in for while something
checks that it still does.** This one was checked, which is how the missing
cache padding was caught; but the checking only ever bounded the error, and the
bound was loose enough to hide a third of the wide word's cost.

A second lesson the correction above earned: **a ratio means nothing without the
dispersion of the thing it is a ratio of.** Two runs cannot measure a spread, so
quoting one to two decimal places invites exactly the over-reading that produced
the withdrawn claim. Where this note gives a ratio it now gives the range too.

## The report is buffered, and what that costs

<a id="d-buffered-report"></a>

**Superseded by [A renderer writes into the sink through
`fmt::Write`](#d-streaming-report).** The renderers stream as of M1.2, and the
`catch_unwind`/`resume_unwind` pair described below no longer exists. Kept
because the cost it records is what motivated the replacement, and because the
ordering argument at the end is still the reason the sink was built this way
first.

Every probe's output goes through one sink: the renderer composes its report into
a `String` and `emit_report` hands it to a [`Report`]. That is what the
repository's architectural pre-step asks for -- the real stream is named in
`report` and nowhere else, so a probe's `main` is one line that chooses no stream
at all -- and it is what lets a test assert a probe's findings instead of a human
reading them off a terminal.

It also gave something up. Printing line-by-line meant whatever had been measured
was already on the terminal; buffering means nothing is, until the renderer
returns. These probes call into measurements documented to panic --
`worker_context`'s impersonating observation panics three ways, and its renderer
composes several completed findings before reaching it -- stated without a count
deliberately, because the number moves whenever a line is added, and a stale count
is the drift this repository keeps paying for -- so this is not hypothetical. For an
instrument whose whole purpose is that a failure be diagnosable, how far it got is
exactly the information worth keeping.

`emit_report` recovers it for an unwinding panic: catch, emit what was composed,
resume, so the exit status and message are unchanged and the partial report is
added to them rather than substituted. **It does not recover it for a termination
that does not unwind** -- Ctrl-C, which the default Windows console handler serves
by terminating the process, and an abort from a panic raised during unwinding.

That bound is known rather than overlooked, and it is not a defect in
`emit_report` to be patched there: the fix is renderers writing into a [`Report`]
as they measure rather than into a `String`, which restores streaming for *every*
termination mode and makes the catch/resume machinery unnecessary. That changes
every renderer and the shape of the sink trait, so it is queued as its own work --
[CHECKLIST.md](CHECKLIST.md) milestone M1 -- rather than folded into the commit
that introduced the sink.

The ordering was deliberate. The sink had to exist before the probes could be
peeled off their originating branch in reviewable stages, and a design that
streams is a different design, not a later revision of this one.

## A renderer writes into the sink through `fmt::Write`, not through a sink method

<a id="d-streaming-report"></a>

**Superseding the buffering above**, as M1 said it would: the mechanism by which
a formatted line reaches a [`Report`] is
[`LineSink`](src/report.rs), an adapter implementing `std::fmt::Write`.

The choice was between giving `Report` a method taking `fmt::Arguments` (with a
`report_line!` macro), implementing `fmt::Write` on a sink so existing
`writeln!` calls keep working, and keeping the `String` while flushing it at
line boundaries. **It was decided by counting rather than by taste.** Every
renderer already writes through `writeln!(out, ...)` against a `String`'s
`fmt::Write`, at **332 sites** in this crate; only 18 functions take the `&mut
String` those sites write into. A sink method would have been the most explicit
option and would have rewritten all 332; `fmt::Write` moves the 18 and leaves
the 332 untouched, because `String` implements `fmt::Write` too and the call
sites cannot tell the difference.

Worth recording that M1 estimated "upwards of 160" of those sites. The real
figure is twice that, and it is the whole of the argument -- an option whose
cost is "rewrite every call site" is affordable at 160 and is not at 332. A
plan's estimate is worth re-measuring at the moment it becomes a decision.

### What the adapter has to reassemble, and why that is not a detail

`fmt::Write` is **line-agnostic**: `write_str` receives whatever slices the
formatting machinery produces -- a fragment below a line, several lines at once,
a bare `"\n"` -- while [`Report`] speaks in whole lines and [`Captured`] is
addressable by line, which is what lets a test name a row. So `LineSink` holds a
partial line and emits only completed ones.

Two properties are easy to get wrong and are pinned by tests rather than by
this paragraph:

- **A report whose last write is a `write!` rather than a `writeln!` must still
  emit that line.** `LineSink::finish` does it. Without it a report loses
  exactly its final row, which is invisible except as an absence.
- **`split('\n')`, not `lines()`.** `lines()` cannot distinguish text that ended
  on a newline from text that did not, and that distinction is precisely what
  decides whether the tail is a finished line or a partial one. Sabotaging each
  of these in turn fails three tests and two tests respectively, so the
  distinction is measured rather than asserted here.

### Every renderer now writes into the sink, and the catch-and-resume is gone

M1.2 pointed all thirteen probes at the sink. Two things about that conversion are
worth keeping.

**The `catch_unwind`/`resume_unwind` pair was deleted rather than left in place.**
Once lines leave as they are produced there is no buffer to rescue, so the pair
would have been machinery that no longer earned its place -- and worse, it would
have kept implying that partial output depends on the panic unwinding, which was
precisely the limitation this milestone removed. The test that guarded it is
unchanged and still passes: the property held by machinery before and holds by
construction now.

**A panic still loses at most a partial final line** -- one on which a renderer
called `write!` without a newline. Flushing it would need a `Drop` on `LineSink`,
and a `Drop` that writes can panic while unwinding, which aborts and replaces a
diagnosable failure with one that explains nothing. An unterminated fragment is
not a finding, so the trade is one-sided.

Every probe in this crate needed only the signature change, because each already
went through `emit_report` rather than composing a `String` and calling `emit`
itself. That is worth stating because it was not free: it is what the one-sink
refactor bought, and it is why converting thirteen probes to stream is a
mechanical change to one function plus one line per renderer.

Three further probes are being developed on a branch and do **not** hold that
property -- they compose a `String` and call `emit` directly, so the crate's
"every probe routes through this" claim is false for them. They are converted
where they land rather than here, since they do not exist in this crate yet.

**Verifying that no report changed needed a control, because several of these
probes are not deterministic.** Comparing before and after directly showed four
of the thirteen reports differing -- which proves nothing on its own, since
these probes print measured nanoseconds and render verdicts branching on them.

Running the *same* build twice is the control, and it differs in **five**, by
the same amount or more in every case:

| probe | lines differing, same build twice | lines differing, across the change |
|---|---|---|
| `probe-doorbell-cost` | 34 | 30 |
| `probe-request-cost` | 32 | 32 |
| `probe-pool-growth` | 14 | 14 |
| `probe-device-map` | 4 | 4 |
| `probe-cancel-io` | 2 | **0** |

`probe-cancel-io` is the one that makes the point sharpest: it is *not*
deterministic, yet it happened to match across the change. Had the before/after
diff been read on its own, that would have counted as evidence of no change --
from a probe whose output varies run to run regardless. The eight reports the
control showed to be genuinely deterministic were byte-identical across the
conversion, and those are the eight that carry the argument.

A before/after diff on a probe is not evidence without that control.

### Measured: an interrupted probe keeps what it had already measured

M1.3 asked for this to be measured once rather than assumed, because it is the
property the whole milestone exists for and no unit test reaches it -- a test
cannot terminate its own process without taking the harness with it.

`probe-doorbell-cost` is the longest-running probe in this crate at about 0.8
seconds, which makes it the subject. Started with stdout redirected, left for
300 milliseconds, then terminated -- **six runs of each build**, with every run
confirmed to have still been alive at the moment it was killed, since a probe
that had already exited would be measuring nothing:

| build | characters captured | runs | content |
|---|---|---|---|
| streaming | **129** | 6 of 6 identical | the host banner and the heading |
| buffered (built from the `LineSink` commit, before the conversion) | **0** | 6 of 6 identical | nothing at all |

The control is the point. Reading 129 characters from the streaming build shows
only that something was written; running the *previous* build through the
identical sequence and reading zero is what shows the change caused it. Both
binaries were release builds of the same crate, killed at the same elapsed time,
by the same code.

The margin is narrower than it looks and deliberately so. 300 ms against an
800 ms probe leaves no room for a slow start to be mistaken for buffering, which
is why each run records whether the process was still running when killed rather
than inferring it from the byte count.

**`TerminateProcess` was used rather than Ctrl-C, and it is the stronger case.**
Ctrl-C on Windows runs the default console handler, which terminates the process
but still lets the runtime unwind its exit path; `TerminateProcess` -- what
.NET's `Process.Kill` issues -- runs nothing at all, so any bytes still sitting
in a userspace buffer are lost outright. A report that survives it survives a
Ctrl-C, so the interactive case is covered by the measurement rather than left
untested.

**Why the bytes are already safe** is worth naming, since it is what makes the
whole design work: Rust's `std::io::Stdout` wraps a `LineWriter`, which flushes
at each newline whether stdout is a terminal or a redirected file. So a line
handed to `println!` has reached the OS before the next one is composed, and no
process-level termination can take it back. Had stdout been block-buffered, this
milestone would have needed an explicit flush per line as well.

## The long-path probe: a pair of binaries, and a second declined hardening
<a id="d-long-path"></a>

The `longPathAware` opt-in has two halves and neither is a runtime switch: a
machine-wide registry value, and a per-executable manifest. Nothing a process can
read off itself tells it whether the manifest half applies, so the question
"does the opt-in lift `MAX_PATH` for a relative path?" cannot be answered by one
binary with a flag. It is answered by two binaries that differ *only* in the
manifest, and the finding is the difference between their reports.

`build.rs` embeds the manifest with `rustc-link-arg-bin` naming
`probe-long-path-aware` specifically, never `rustc-link-arg-bins`: the plural
form would opt every binary in the crate into long paths and silently change what
all the others measure. The aware binary does not assert its own manifest either;
it reads a `cargo::rustc-cfg` the build script emits from the same guarded block
that does the embedding, so the label and the linker cannot disagree. Before that,
a non-MSVC target skipped the block and produced two binaries with no manifest
between them, one of which still reported `manifest longPathAware : yes` -- the
one failure this probe cannot make loudly, because the whole finding is the
difference between the pair.

### `measure` moves the process's current directory, and that is the point

<a id="d-long-path-cwd"></a>

A relative path resolves against the current directory, so half of what is under
test is *where the process is*. The probe therefore sets the current directory
deliberately rather than inheriting whatever launched it, restores it in a `Drop`
guard, and removes the tree it built.

This is the same tension the error-mode probe records in
[The concurrency hardening is knowingly declined](#the-concurrency-hardening-is-knowingly-declined):
a probe may change process-wide state, a component may not. As there, **the
concurrency hardening is knowingly declined.** `measure` is not safe to call
concurrently, and its rustdoc says so. Giving each call a unique root would not
fix it, because the current directory is per-process rather than per-call: two
concurrent runs would still fight over the one thing being measured. The
serialization lives in the tests, which take a mutex, and the binaries are
single-threaded and call `measure` once.

The declined alternative is worth naming so it is not re-proposed: threading the
directory through as an explicit parameter and never calling
`SetCurrentDirectoryW` would make the function safe, and would also stop it
measuring the thing it exists to measure -- a *relative* path's resolution, which
is defined against the process's current directory and nothing else.

### The ceiling is applied to the path as written

<a id="d-long-path-literal"></a>

`MAX_PATH` is compared against the literal path handed to the call, before `..`
is collapsed. That matters here because the `..` shape's literal is five units
longer than its canonical form, so the two readings disagree in a five-unit band
-- and the probe classifies every row on that number.

Measured rather than assumed, using this crate's own un-manifested binary with
the deep level forced to 21: plain resolved to 258 and **opened**, `..` resolved
to 263 and was **refused**, against a content ceiling of 259. Had the collapse
come first, both would have been 258 and both would have opened.

The obvious shortcut does not settle this and should not be used. Reaching for
`cmd.exe` measures `cmd`'s manifest, not the un-opted-in case: on the development
host -- Windows 11 build 26200, `cmd.exe` 10.0.26100.1 -- `cmd` carries
`longPathAware` in its own manifest beside `dpiAware`, so a long path that opens
there says nothing about the ceiling. That is a fact about that binary on that
build rather than about `cmd` for all time, which is exactly why the probe rests
on a binary this workspace builds and manifests itself.

## The topology cross-check has three lists, because they have three owners

<a id="d-topology-three-lists"></a>

The topology probe measures what `windows-topology-sys` parsed and compares it
against three Win32 counters read independently. What it may then *claim* is a
`Verdict` of `Agree`, `Disagree`, or `Incomplete`, derived from three lists that
are deliberately not merged:

- `disagreements` -- a counter was compared and did not match. A finding about
  the shipping crate's parse.
- `not_compared` -- a reading this probe could not make or could not trust. A
  gap in this measurement, and nothing at all about the parse. (The causes are
  not enumerated here. This bullet once named two of them, and a machine that
  changed under the run -- a bracket that closed on two *different* instants,
  which is neither of the two named -- had already falsified the pair.)
- `parse_incomplete` -- the parse is short, or its claims are mutually
  inconsistent. Neither of the above: nothing this probe read was
  contradicted, and nothing it wanted to read was missing. Established from the
  parse rather than from any counter, which is why no counter agreeing can
  retire an entry here.

  Note the owner is the *parse*, not "what the crate said about itself". That
  narrower reading held only while every entry happened to be a crate
  self-assessment; the CPU-Sets-only NUMA domain below is derived by the probe,
  from provenance the crate carries but draws no conclusion about.

Collapsing any pair of these produced a shipped defect, each caught in a
separate review round on the same branch. Merging the first two let a failed
`GetNumaHighestNodeNumber` print the report's agreement line directly below
"GetNumaHighestNodeNumber : failed". Omitting the
third let a parse the crate had *already reported as incomplete* satisfy all
three counters and reach `Agree` -- worse, because that evidence was in hand
rather than needing another call to fetch. Treating a counter's zero as a count
inverted the blame, reporting a failed read as though the crate had parsed the
machine wrongly.

The same inversion reached the NUMA node numbers. `highest_numa_node` was taken
from the relationship walk's label alone, so a domain that only CPU Sets
described -- which `fold_memberships` pushes as its own domain, because "the
walk not describing it is a fact about the walk, not evidence the relation is
not there" -- raised the domain count while being unable to raise the highest.
The gap against the machine-wide `GetNumaHighestNodeNumber` was then filed as a
`disagreement`: an accusation against the crate for a label this probe had
discarded, with the contradiction printed in the same report. The maximum is
now taken across every label a PLATFORM source reported, which is sound because
`NumaNodeIndex` is machine-wide from both of them -- unlike `CoreIndex`, which
is group-relative and is why the labels are not interchangeable in general.

"Every observation's label" was the first correction and went one step too far.
`Source::Description` is not a platform source, so a caller annotating a domain
the walk had already reported -- which is platform-backed, and therefore does
not close the provenance gate -- could raise the maximum above anything Windows
said and have the difference filed against the shipping parse. The filter is
what keeps the comparison a comparison of two platform readings.

That left a real finding needing somewhere to go, and it became a
`parse_incomplete` cause: a memory domain only one source described means the
two sources group NUMA membership differently. Nothing else reaches it.
`coherence` compares PROCESSOR SETS, so two sources can name exactly the same
processors and still disagree about nodes; and the node totals can match while
the membership does not, so no counter sees it either.

The rule is fiat rather than derived: **`Agree` requires all three lists
empty**, so anything `cross_check` pushes blocks it, whether or not a counter
noticed. Only `disagreements` yields `Disagree`, because an incomplete parse is
not a wrong one and reporting it as a divergence sends a reader to audit a
mismatch that does not exist.

**Stated over the lists, not over their causes, and deliberately so.**
`cross_check`'s body is the single enumeration of what fills `parse_incomplete`,
and this note does not reproduce it. Read the body.

This section is itself the worked example, twice. An earlier revision stated the
rule as "anything other than an empty anomaly list and `Coherence::Agreed`",
which was true when written; a third cause was added without sweeping the
restatements, and this note then transcribed the stale pair as settled
fiat. As a biconditional it had become false, and the danger ran the wrong way:
the guidance below tells a reader not to loosen the CI assertion, but nothing
would have stopped one *tightening the code* by deleting a branch this document
did not mention. A rule phrased over the lists cannot rot that way, because a
fourth cause satisfies it without anyone remembering to edit prose.

The rule was then correctly restated -- and a *list of the current causes* was
left behind in both this note and `cross_check`'s own rustdoc, hedged with
"treat that as the current contents rather than the rule". A fourth cause was
added one commit later and neither list was swept, so the fix rotted inside two
rounds in the same paragraph that diagnosed the rot. The hedge did not help,
because the rustdoc carried no hedge at all. The lesson is stronger than the one
first drawn: a list of causes kept beside the rule is not a summary of the body,
it is a second copy of it that nothing checks, and the durable answer is not to
keep one.

The `coherence` match is exhaustive for the same reason at the type level: a
variant added later is a compile error rather than a silent new path to "parsed
this machine consistently".

`Verdict` is an enum rather than a `bool` for the same reason, and the NDJSON
carries a `"cross_check"` string rather than a boolean -- `Verdict`'s three
values, plus `not_measured` on the row emitted when discovery itself failed:
a log-mining pass must be able to tell "everything checked out" from "two
things checked out and the third was never established". `cross_check_ok:true`
said the same thing for both.

**`cross_check == "agree"` is the one field a mining pass must read before
trusting any other.** Every count on that line comes from what decoded, so a
record Windows returned that did not fully decode leaves `caches`, `packages`,
`cores` and `outermost_partitioning_cache_level` wrong by an amount no field
states, and a query grouping by cache level has no reason to join against
`enumeration_anomalies`. The verdict closes that.

**It closes that, and no more: `agree` means no record FAILED TO DECODE, not
that the counts are complete.** Nothing independent measures packages, cores or
caches, so a record that decoded cleanly while describing less of the machine
than exists -- a package covering half the online processors, a cache level
whose one domain covers half of them -- raises no anomaly and reaches `agree`.
That gap is deliberate and open: a coverage check would have to hold on every
machine in the runner fleet, and by the decision below any verdict other than
`Agree` fails the build, so a check this probe cannot validate beyond its own
host would fail builds for hosts that are reporting themselves correctly.
`agree` says every check this probe could make was made and matched -- never
that a check exists for every field on the line. The report's own comment above
`x-probe-topology` says the same thing, and the two are meant to be read
together.

Note "wrong", not "short". A record that decodes to nothing is dropped and
shortens a count, but a `TruncatedArray` record is *kept* with the entries that
fit -- so a cache record with a partial affinity mask presents a processor set
smaller than the truth, which `cache_partitions_at_level` counts as its own
distinct partition and which therefore INFLATES a domain count. An anomaly does
not tell you the direction, and nothing in the report claims to.

**The verdict closes that and no more: `agree` means the counts are not
DISTORTED, not that every field is a plain hardware fact.** `outermost_partitioning_cache_level` is
where the difference bites. `windows-topology-sys` answers `None` both when no
level partitions the machine and when two partition it incomparably -- and a
machine of the second kind has a complete parse, agrees with every counter, and
still has no outermost partitioning cache. Both emitted `null`, on a row the
verdict had already certified, so a fleet query counting nulls as "machines no
cache level partitions" -- the natural reading, and the one this crate's own
no-L3 story invites -- folded in machines where a level DOES partition. Opposite
conclusions for anything sizing itself by cache boundary.

The prose report had refused to conflate the two from the start, on the grounds
that "naming only the first turns a reported ambiguity into a false claim about
the hardware". The NDJSON simply had no field to say it in. It now does:
`outermost_partitioning_cache` is always a string, so a consumer filters on
`== "none"` rather than on the absence of a number, and
`Observation::partitioning_cache` returns an enum so a renderer cannot emit the
absent case without having decided which absent case it is.

**The value set is `PartitioningCache`'s variants, and is not reproduced here.**
This paragraph did list them, and went stale one round later when
`no_levels_reported` was added -- the same collapse this field exists to prevent,
moved into the docs, and with a worse failure mode than a merely-missing branch:
a consumer that maps "not one of the documented values" onto the absent-level
default folds machines whose cache survey was EMPTY back into "machines no cache
level partitions", which is a claim about hardware read off a survey that found
no cache structure. Read the enum, which the renderer matches exhaustively, so
the two cannot diverge.

`not_unique` is deliberately not called "incomparable":
the crate reaches `None` both for two maximal candidates that are not the same
partition and for a candidate filter that left nothing, and this probe cannot
tell those apart.

The renderer's prose conclusions -- every claim it makes about the hardware, not
only the cache ones it was written for -- are gated on a **related but
deliberately narrower** condition, `CrossCheck::parse_in_doubt`: `disagreements` or
`parse_incomplete` non-empty, but *not* `not_compared`. The verdict answers
"may this run claim agreement", where a counter that could not be read matters;
the caveats answer "may these counts be read as hardware facts", where it does
not -- every `not_compared` entry is a reading this probe could not make or
could not trust, which says nothing about the parse. (Stated over what the list
means rather than what fills it: this once enumerated "the three Win32 counters
failing to read", and two later rounds falsified it by adding the bracket
outcomes.) Caveating there would assert a doubt the
run does not have, which is the same defect as asserting a certainty it does
not have.

The two conditions are close enough that stating them as one was tempting and
was twice wrong in the other direction. The renderer first re-derived the
condition as "anomalies non-empty", so a host with `Coherence::Disagreed` and no
anomalies claimed "this machine reports no L3 at all"; corrected to
`parse_incomplete` alone, it still missed `disagreements`, so a host whose group
count Windows contradicts printed that same hardware claim directly above
"=> DISAGREE" -- in the one case where the evidence that the parse does not
describe this machine was already in hand. Both times the accompanying comment
asserted the condition was complete. Hence a single named predicate that argues
its own membership, rather than a condition restated at the point of use.

## An incomplete parse fails CI, and that is the point

<a id="d-topology-incomplete-fails-ci"></a>

`the_shipping_parse_agrees_with_the_raw_win32_counters` asserts
`Verdict::Agree`, and by the rule above that requires **all three** lists empty.
So it goes red on anything that fills any of them -- not only a parse the crate
reported as short or disputed, but also a counter this probe simply could not
read (`not_compared`), which is a gap in the measurement and says nothing about
the parse at all. Every non-`Agree` cause is a red build; there is no subset
that is tolerated.

That matters because the parse-side causes are legal `Ok` results -- `discover`
returns the topology and says how the run went -- so this test can go red on a
host that is merely misbehaving, or on a run where a Win32 call failed, rather
than on a defect in this repository. CI runs the probe on `windows-latest`, a
virtualized fleet, which is where a defective hypervisor would show up.

That is deliberate. The probe exists to survey real machines, and a host whose
enumeration is losing records is exactly the finding worth interrupting a build
for; a test that passed quietly on it would be the "report asserting something
the run did not establish" failure this whole probe is built to prevent, moved
up one level into the test suite. The cost is accepted: an occasional red build
that turns out to be the runner rather than the code.

So **if this test goes red, read the verdict before touching the assertion.**
An `Incomplete` verdict names what was not established, and the answer is to
investigate that host -- not to relax the assertion, which would discard the
only signal that would ever have surfaced it.

For that reading to be possible, the workflow step that runs the probe carries
`if: '!cancelled()'`. It sits after this test in the same job, so without it
GitHub Actions skips the report on exactly the host the report is for, and the
only surviving evidence is the `CrossCheck` in the assertion message -- not the
domain counts, the enumeration anomalies, or the machine-readable row. The
probe's own documentation says it prints on every build; that is what makes the
claim true rather than nearly true.

What it prints is a **second measurement**, not a rendering of the one that
failed: the test and the binary each call `measure()`. That recovers a condition
the host holds persistently -- a fleet machine whose enumeration is genuinely
losing records, which is the case worth interrupting a build for -- and does not
recover one that was transient. Rendering the failing observation itself would
mean the assertion and the report were one step, which is a different design
than the binary-plus-asserted split this crate is built around.

No work is scheduled by this decision; it records why the strict form is
correct so a future contributor does not quietly loosen it. Revisiting it means
splitting the verdicts -- `Disagree` failing while `Incomplete` reports loudly
and passes -- which is a change to what CI is for, not a bug fix.

## The host banner is bracketed too, because it is a topology and not a name

<a id="d-topology-banner-bracket"></a>

Every probe here opens with `fingerprint::banner_line()`, and in this one that
line is a *second* topology discovery: the fingerprint renders architecture,
processor and core counts, cache domain sizes and NUMA nodes, none of which
`measure`'s bracket encloses. It was read once and defended as "attribution",
on the grounds that no conclusion in the report is drawn from it.

That defence was weaker than it sounded. A machine that changed across the run
would print one shape in the banner and a different one in the body, and a
reader mining accumulated CI output has no way to tell which described the
measurement -- the banner states the same quantities the body cross-checks, so
the two simply contradict each other with nothing saying so. Calling the header
"attribution" does not stop a reader reading a core count off it.

So it is bracketed like everything else: read before and after, and reduced by
`topology_report::attribution`. Equal readings render exactly as before, which
keeps every fingerprint string already recorded elsewhere comparable with this
probe's. Readings that differ print both, because which of the two is stale is
precisely what cannot be determined here.

**It reports that the readings DIFFER, and does not name a cause.** Saying "the
host changed" was the first wording and was itself an over-claim of the kind
this decision exists to remove: `Fingerprint::discover` returns `Ok` on a parse
that dropped a record or whose two sources disagreed, so a fingerprint can
differ from the one before it because the enumeration was flaky rather than
because any hardware moved. `measure`'s own bracket may say "the machine
changed" because a counter is a simple reading with no such failure mode; a
fingerprint is a whole parse, and the same sentence is not available to it.

The two readings are `Fingerprint::discover` results rather than rendered
lines, and that is load-bearing. `banner_line` renders success and failure into
one string, so comparing two of those cannot tell a host that moved from a
discovery that failed -- and two failures whose `io::Error` text differs compare
unequal while establishing nothing at all. Rendering still goes through
`banner_line_for`, added to `windows-placement-probe` for this, so the format
has one owner and this probe's banner stays comparable with every other
probe's.

This is a wider window than `measure`'s own bracket rather than a duplicate of
it: it closes over the whole run including both banner reads, where `measure`
closes only over the counters. Neither subsumes the other, and no work is
scheduled by this decision.

## The correspondence-oracle investigation, and what it concluded

<a id="d-correspondence-failures"></a>
<a id="d-oracle-refuses-to-know"></a>

**Superseded by [The encoded row is the contract; the prose is not](#d-encoded-row-is-the-contract).**

**Moved to Tier 2: [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).** The anchors
above are kept here so every existing link still lands somewhere that says where
the content went.

What it covers: the two defects a pull-request review found after twenty-eight
rounds of per-artifact review and a zero-surviving-mutant sweep; why each
instrument in use was structurally incapable of seeing them; the sparse-matrix
and oracle split that followed; and the oracle's own design, failure modes and
mutation evidence.

It is Tier 2 rather than Tier 1 for two reasons. It is a record of how a decision
was reached rather than a statement of one -- and the decision it reached has
since been superseded by
[The encoded row is the contract; the prose is not](#d-encoded-row-is-the-contract).
The code it describes is also gone: the prose-reading oracle, its correspondence
enum and its fact-accounting instrument were retired by M3.4 and M3.5, so several
of its sentences name types that no longer exist.

What survives into Tier 1 is the conclusion the two defects actually support,
which is the decision below.

## The encoded row is the contract; the prose is not

<a id="d-encoded-row-is-the-contract"></a>

A probe is a data pipeline that renders, at its tail, to two artifacts: an NDJSON
row and prose. **They are not peers.** The row is a machine contract -- mined
across a fleet, joined against other runs, and the thing this workspace's designs
end up resting on. The prose is for a reader.

So they carry different obligations:

- **The row must be CORRECT**, and that is machine-enforced. Its values, its
  invariants and its shape are asserted.
- **The prose must be ACCURATE AND READABLE**, and that is enforced by review.
  It is not required to be programmatically comparable against the row, and
  nothing here checks that it is.

This supersedes the rule in
[#d-oracle-refuses-to-know](DESIGN-RATIONALE.md#d-oracle-refuses-to-know), which said the oracle
must read the rendered artifact rather than the state behind it.

### What forced it: both originating defects were defects in the row

The reason the earlier rule looked right was a misreading of its own evidence.
Re-checked against the code, for the two defects in
[#d-correspondence-failures](DESIGN-RATIONALE.md#d-correspondence-failures):

**The alarm beside the agreeing verdict.** `report` emits
`BUG IN THIS PROBE: ...` with a `writeln!` into the prose, and the NDJSON row has
**no key for it** -- while `cross_check` IS a key, and read `agree` on the
defective run. So the row certified a clean agreeing measurement on a host where
the probe had detected its own bug, and said nothing about the bug. A survey
mining that row would have been wrong and had no way to know. The prose alarm was
not the defect; it was the only trace that the row was wrong, which is why a
human found it and no instrument did.

**That defect is fixed, and what it left behind was the live gap.** Checked
rather than assumed, because the paragraph above describes the code as it was:
`Observation::cross_check` pushes `PartitioningCache::SummaryMissing` onto
`parse_incomplete`, which forces the verdict away from `agree`, so the row could
not certify that run. But the row published `parse_incomplete` as a **count** --
as it did `not_compared` and `enumeration_anomalies` -- where the prose published
each entry's text. A survey reading `"parse_incomplete":1` could not tell *the
probe detected a bug in itself* from *a core record contradicted itself* from
*this topology was not measured from a running machine*. Those are categorically
different facts, and only the prose distinguished them.

So the shape of the problem was not that the row is out of step with the prose.
It is that **the row was impoverished relative to the prose** -- the artifact
that gets mined carried less than the artifact that gets read -- which is
backwards given which of the two the designs rest on.

**M3.1 closed this**, and the past tense above is deliberate: the three fields
publish the conditions themselves, minted by `topology::diagnostic`, so a survey
reads which one fired rather than how many there were. The count remains
available as the list's length.

M3.3 then gave each entry its DATA, so the published form is an object rather
than a bare code:

```
"parse_incomplete":[{"code":"partitioning_summary_missing","level":9}]
```

Stated here because this is Tier 1 and the wire format is what a reader comes to
it for. The bare-code form this paragraph first showed was M3.1-era and was
superseded three commits later on the same branch -- the drift class this
component keeps meeting, caught by a review. The rest of
this decision is unaffected -- it is about which artifact carries the contract,
not about these three fields.

**`efficiency classes: [0]` against `"efficiency_classes":1`.** Both halves were
correct derivations of one consistent value -- the prose rendered the set, the row
rendered the cardinality -- so no invariant was violated. Note how it was
repaired: the row now publishes `"efficiency_classes":[...]`, the set. **The fix
was to change what the row publishes.** The prose comparison was how a reviewer
noticed, not the repair.

Neither defect needed a prose-against-row oracle to fix. Both needed the
structured output to be made right.

### The rule that falls out, and it is the load-bearing one

**A renderer may not tell a reader something the row cannot tell a survey.** A
state worth naming to a human is a state worth publishing to a mining pass; if
only the prose can say it, the fact exists solely in the artifact nothing
queries, and the only detector is a person reading. A cardinality is not a
statement of the fact -- `"parse_incomplete":1` names no condition -- so a count
beside a prose list is an instance of this rule being broken, not an exception
to it.

With that rule in place the surviving correspondences stop being text
comparisons and become **invariants on the observation, checked before
rendering** -- `summary_missing` implies the verdict is not `agree`, and likewise
for the other diagnostics and the counters. No parser is involved.

**This rule is enforced, and the first thing it would have caught was already
broken when the rule was written.** Every instrument in this crate used to start
from what the row publishes -- the fact accounting enumerated the row's keys, the
mutation sweep perturbed code the row's construction reached -- so all of them
asked "does anything read this key?" and none asked "does the prose state a fact
the row omits?". Measured: `CrossCheck::disagreements` was rendered per-entry in
the prose and published in the row as nothing at all, so a survey could see
`"cross_check":"disagree"` and not which counter disagreed. It survived 41 review
rounds and a zero-survivor mutation sweep. A reviewer found it by reading the
enum and asking who called `code()`.

The second enumeration -- every state that forbids agreement to a published
condition -- now exists, in
[tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs):
`every_state_that_blocks_agreement_reaches_the_row` holds `topology::invariant`'s
blocking states against the row's keys, and `publication_holds` is the shared
predicate the corpus rule and its sabotage both call. Landed as M3.5, archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

(Until M3.5 landed this section said "nothing here enforces that rule yet" and
pointed at CHECKLIST.md for the queued item. Both statements outlived the
milestone that made them false -- the reason this Tier 1 file is swept against
the code rather than trusted, and an instance of the restatement drift the
repository instructions describe.)

### What the text-reading design cost

Counted in [src/report_oracle.rs](src/report_oracle.rs) **as it stood before this decision**: of 38
top-level functions, ten were correspondence rules and four were comparison
helpers. **Twenty-three existed only to extract values back out of rendered
text.** None of them survives: [src/report_oracle.rs](src/report_oracle.rs) reads no
rendered PROSE at all now, and hand-writes no string scanning -- the row's
well-formedness is a `serde_json` parse and its keys come from that parser's own
tokens. (This said "reads no rendered text at all", which a review correctly read
as contradicting the module: the prose reader is gone, the ROW parser is not, and
the row is rendered text. What changed is that nothing here infers a value from a
sentence -- the one parse left is of a format with a specification, performed by a
library rather than by this crate.) So the counts above are what the design cost, not what the file holds.
(They are also the only counts kept here, because they describe a file that no
longer exists in that form and so cannot drift; a count of the CURRENT file would
be a census, and is deliberately absent.)

That is a parser for a format this crate itself writes, and it behaved like one.
A large share of PR #88's review rounds were defects in the READER rather than in
the thing read: a multi-byte panic in `processors_in_banner`, a `p/` substring
matching inside an opaque `io::Error`, `trim_matches` collapsing `[[0]]` and
`[0]`, a prose lookup selecting the wrong line when two began alike. None of
those is a defect in a probe. They are a defect source the design created for
itself.

### Where structure replaces checking, prefer structure

Three of the four hazards this component has actually met are made
*unrepresentable* by construction rather than detected after the fact, and that
is the stronger move:

- **Injection.** Caller text reaching the row is contamination of the mined
  artifact. Measured on PR #88: an `io::Error` containing `{` was selected as the
  report's machine-readable row. A typed row emitted by one writer cannot have
  this.
- **Field order and labelling.** The row was built by interpolating every value
  positionally through a `concat!` template, so a field's name and its value were
  related only by counting -- and a reordered argument or a miscounted `{}` gave
  mislabelled data that still parses. A typed row with one writer cannot have
  this either, and that is what M3.3 built: the template is gone, and
  [src/row.rs](src/row.rs) is the one writer.

  Stated as the coupling rather than as a count, deliberately, and the reason is
  on the record: this said "eighteen values", was corrected to "seventeen" when
  a review counted the placeholders, and was falsified again within the hour by
  M3.1's follow-up adding `disagreements`. The hazard is that the correspondence
  is positional at all; how many positions there are is exactly the sort of
  census this component keeps having to re-correct.
- **Value divergence.** Two renderings of one field cannot disagree about its
  value when both read the field.

What structure does NOT cover, and so still needs something reading bytes: **the
writer itself.** Several of PR #88's defects lived there -- a disclaimer matched
as a suffix so it could be welded onto the line above, a flattening that ate the
disclaimer, a containment that produced `host:  host:  ...`. The residual text
check is therefore small and about well-formedness, not about correspondence.

### What this does not say

It does not say the prose does not matter. An overstated finding in prose
propagates into the design notes that cite it, which is a live concern in this
crate rather than a hypothetical -- M2.9 in [CHECKLIST.md](CHECKLIST.md) is an
open item about exactly that. What changes is that prose accuracy is a **review**
obligation, discharged by a person reading the report, rather than a
correspondence a machine asserts.

It also does not delete the correspondence rules. They relocate onto the
observation, losing the parser in front of them. The containment work in
[src/topology_report.rs](src/topology_report.rs) matters MORE under this
decision, not less, because what it keeps out is now keeping it out of the
contract artifact.

The work this implies was M3, which is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md). (This said "is queued as M3 in
CHECKLIST.md" until a review pointed out that the canonical design note was
advertising landed work as pending.) The session that produced it is
[design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md](design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md).
