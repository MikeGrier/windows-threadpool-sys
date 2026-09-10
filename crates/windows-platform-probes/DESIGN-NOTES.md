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

## This crate is never distributed, and its dependencies carry no versions

Not to a registry, and not as a released binary either -- unlike
`windows-placement-probe`, which ships a CI-built binary to people running it on
hardware this workspace does not own. These probes are a development
instrument, run from a checkout by someone who has the checkout. `publish =
false` is the whole story, and it is permanent rather than "not yet".

**The consequence is that every workspace dependency here is path-only.** A
`version` beside a `path` exists to tell a registry what to resolve when the
depending crate is packaged. Nothing packages this crate, so those pins named a
version no one would ever consult -- while still having to be correct, because
cargo requires the path crate's own version to satisfy the pin **at every
build**, not merely at publication.

That is not a theoretical tidy-up. Measured on 2026-09-02: bumping
`windows-topology-sys` to 0.2.0 while this crate pinned `"0.1.0"` failed
`cargo metadata` for the whole workspace. Six such pins were six standing
chances to break `main` on someone else's release, in exchange for nothing,
and they are gone.
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

`probe-queue-contention` measures what M31.5 of
[CHECKLIST-io-domains.md](../../CHECKLIST-io-domains.md) exists to decide: whether the bounded array
queue's tail claim contends badly enough to justify the linked and sharded MPSC shapes, and what
`reserving_mpsc`'s extra read of the consumer's position actually costs.

**It is deliberately absent from the `platform-probes` CI job, unlike every other probe, and the reason is
a measurement rather than a preference.** That job runs `cargo run` without `--release`. Measured in a
debug build, `mpsc` and `reserving_mpsc` come out at 249.7 and 254.0 ns/push at sixteen producers --
indistinguishable. In release, on the same machine in the same minute, they are 193.5 and 52.2. The
un-inlined overhead of a debug build swamps the cache-coherence effects that *are* the finding, so a debug
run of this probe does not merely lose precision: it reports the two shapes as equivalent, which is a
confident wrong answer of exactly the kind this crate's `doorbell_cost` notes warn about.

Two further reasons it stays out. A contention curve needs more cores than a hosted runner has, and the
32-producer rows on a four-core runner would measure the scheduler. And the run costs about two minutes in
release, against a job whose other probes are seconds.

So this one is run by hand, on a known machine, and its numbers are recorded with the machine attached.

### Reading it

Two regimes, and the pair is the point.

**Isolated** gives producers a capacity large enough that nothing is ever refused and runs no consumer, so
whatever curve appears against N is the claim and nothing else. **Drained** runs a consumer popping
continuously, which is the only regime that can price `reserving_mpsc`'s read of `head` -- that read is
cheap until a consumer is *writing* the line, and measuring it in isolation would report it as free.

The drained regime has a **single** consumer, because that is what MPSC means, so at high producer counts
it becomes consumer-bound and a plateau there says nothing about the claim. Each row carries the refusal
count from the queue's own `Observable` counters precisely so that is visible as a fact rather than
mistaken for contention: the sixteen- and thirty-two-producer drained rows show millions of refusals and
should be read as measurements of the consumer.

## `probe-core-affinity`: placement costs 5.6x, and it refuted the hypothesis it was written to test

This probe pins an SPSC producer and consumer to chosen logical processors and measures the handoff
under each placement the machine can express. It exists because
[`probe-peer-index-cache`](#probe-peer-index-cache-a-result-that-inverts-by-host-which-is-why-it-is-kept)
gave opposite answers on two hosts, and the obvious suspect was *placement*: a machine with two
efficiency classes might be decoupling the two threads in a way a homogeneous one does not.

**The plain answer, which is the useful one.** On the ARM64 development host the unoptimised handoff
costs **38.5 ns/item within a domain and 215.3 ns/item across domains -- 5.6x, for no change but where
the two threads run.** Within a class, the performance cores (class 1) run the same handoff at 30.4 ns
against the efficiency cores' 38.7, about 27% apart, which is a real but far smaller effect than
crossing the boundary. Medians of three, stable across three invocations.

**The hypothesis was refuted, and backwards.** The prediction was that mismatched core speeds would
decouple the two sides, letting a backlog form and giving peer-index caching the deep batch it needs.
Measured, threads placed *together* batch **~135x deeper** than threads placed apart (49.6 against 0.4
items per shared read). A coherent reading is that a cheap handoff lets the producer race ahead and
build a backlog while an expensive one throttles it into lockstep -- so cost drives depth rather than
core speed driving it -- but **this run does not test that**, and the probe says so rather than
recording a replacement conclusion it did not earn. What is established is only that the original
prediction is wrong.

**It also failed to explain the host disagreement, which was its main purpose.** Caching wins at *both*
placements here (14.4x together, 3.0x apart), so placement alone does not account for x64 rejecting the
technique while ARM64 accepts it. That question stays open under `D-28` and M-inf.4.

**A confound this machine cannot escape, stated because it bounds every reading above.** Its efficiency
classes and its cache domains coincide exactly -- processors 0-5 are class 0 behind one L2, 6-11 are
class 1 behind the other -- so every cross-class pair is also a cross-cache pair. The 5.6x is
"across domains", and attributing it to core speed *or* to cache would need a machine whose classes and
caches cut differently. The probe detects this and prints a CAUTION rather than letting a reader draw
the finer conclusion; two of its four placement rows come back `n/a`, and reporting a placement as
inexpressible is deliberately not the same as reporting that it made no difference.

Two construction notes. **Pinning failures panic** rather than warn: a silently unpinned thread turns a
placement experiment into a measurement of the scheduler's preferences while still printing a confident
number. And **batch depth is read from the cached runs only** -- the baseline strategy reads the shared
line on every operation by definition, so its depth is ~1 at every placement and carries no
information. An earlier revision compared the baseline depths and duly reported 0.8 against 0.4, which
is noise around a constant being read as a finding.

## `probe-peer-index-cache`: a result that inverts by host, which is why it is kept

This probe measures peer-index caching -- each side of an SPSC ring keeping a plain copy of the other
side's position, so the shared line is read once per batch instead of once per item -- against the
`windows-waitable-queues` `spsc` shape. **It gives opposite answers on our two architectures**: roughly
1.8x slower on x64, roughly 17x faster on ARM64, because the batch depth it amortises over is set by how
the two threads interleave on that host rather than by our code. The full reasoning lives with the queue
as [DESIGN-NOTES.md](../windows-waitable-queues/DESIGN-NOTES.md) -> `D-28`.

This section previously described the probe as recording a settled rejection, on x64 evidence alone.

Three things about its construction are deliberate and worth keeping if it is ever edited.

**It counts shared reads, not just time.** A timing-only result would have been unreadable: "caching is
slower" is indistinguishable from "the caching was implemented wrongly and never engaged". The read
counters settle that directly, and they are also what made the two hosts comparable -- the reads reveal
a batch depth near 1 on x64 against roughly 150 on ARM64, which is the mechanism rather than the
symptom. Any future variant added here must keep the counters for the same reason.

**Its interpretation is derived from the run, and must never go back to prose.** It used to print the
x64 conclusion as a fixed paragraph -- "the technique WORKED and still lost", "roughly 3.6x", "the
producer count goes UP" -- with only the speedup ratio computed. Run on ARM64 it printed all three while
its own table three lines above showed the opposite, and the contradiction was noticed by a reader
rather than by the tool. A probe that states its finding regardless of what it measured is worse than no
probe, because it is believed. The interpretation now computes the batch depths and says outright that
this verdict is host-dependent.

**It carries a calibration row and a warming control.** The calibration times the real shipping `spsc`
beside the model, and the probe prints a CAUTION when they diverge by more than 25% -- which they
currently do, so the probe says out loud that its rows describe the model rather than the shipped
queue. That guard earned its place immediately: the first run's 3x gap would otherwise have been read
straight past. The warming variant is a control for the hypothesis that a discarded prefetch could
substitute for the real thing; it removes no read and moves no time, which is exactly what a control
that confirms the null should do.

Like `probe-queue-contention`, this probe is **absent from the CI probe job**, and for the same
measured reason: the effects it studies are coherence effects that a debug build's overhead buries.

## The fingerprint carries provenance inside the string, not beside it

The fingerprint is a **canonical summary of a machine's marginal shape**: two hosts rendering the
same string have the same processor, core, cache-domain, class and node sizes, so string equality
is a supported way to group results by shape. (It does *not* mean the two can express the same
placements -- the sizes are recorded without how the partitions intersect. See
[`Fingerprint::provenance`](../windows-placement-probe/src/fingerprint.rs).) That the string is
compared at all is what forces the provenance marker to live *inside* the rendered form. A marker kept alongside -- a separate field, a
second printed line, a note in the surrounding prose -- would leave a fabricated machine claiming the
exact shape of a real one **comparing equal to it**. That is a concrete bug rather than a display
preference, and it has a test named for it.

Three details are deliberate:

- **A measured host renders exactly as before, with no prefix.** Every fingerprint already recorded in
  a checklist or design note came from a real machine, so those strings stay valid and comparable
  rather than being silently reinterpreted by this change.
- **The prefix leads**, so a reader scanning a column of pasted results cannot skip it, and it is
  removable -- stripping `!!SYNTHETIC!! ` yields exactly the measured rendering, so a synthetic host
  can still be compared against a real one on purpose.
- **`RESTORED` and `SYNTHETIC` are distinguished** rather than collapsed into one "untrusted". They
  are different claims: one describes some real machine, the other describes none, and a reader
  deciding how far to believe a number needs to know which.

`Fingerprint::from_topology` exists so provenance *flows* from the topology rather than being stamped
on afterwards. `discover` is now a thin wrapper over it, which means there is no path that invents an
answer -- whatever the topology says is what the fingerprint reports.

`print_banner` was split so the line is available as a string. The taint marker reaching that line is
the entire point of carrying provenance, and a property that load-bearing should not rest on someone
having read a format string correctly.

## Which seams are safe: data may be injected, labels may not reach hardware

Two topology-injection seams were considered during this work and they were decided opposite ways.
The rule that separates them is worth stating on its own, because "add a seam for testability" reads
as unambiguously good and here it is only half true:

**A seam that only moves data is safe. A seam that lets fabricated labels reach real hardware is
not.**

- [`places_from_topology`](../windows-placement-probe/src/fingerprint.rs) **has** a seam. It is a pure conversion -- topology in,
  processor positions out, nothing pinned and nothing timed. A synthetic topology yields synthetic
  positions, which is what the caller asked for and cannot be mistaken for a measurement.
- [`measure`](../windows-placement-probe/src/core_affinity.rs) **must not**, and its documentation says so at the definition.
  A synthetic topology's processor *numbers* are still valid on the real host, so every pin would
  succeed and the run would produce genuine timings filed under fabricated node ids -- output
  indistinguishable from a real NUMA measurement that measured no such thing. The pin assertion does
  not catch it: it rejects a processor that does not exist, not a label that is wrong.

The absence of the second seam is also what lets `Slice` carry no provenance marker of its own, so
the two decisions hold each other up.

### The hole this closed, and how it was proven

`discover_places` took no argument and appeared in no test. It was untestable, not merely untested,
and it carries the rules for the partitioning cache level, core and class membership, and the NUMA
node. The NUMA lookup in particular was **unverifiable on every host available to this workspace**:
with a single node, a correct map and a completely broken one both yield node 0.

Replacing the entire lookup with a hardcoded `0` was run against the suite as it stood before this
change. **It passed everything.** Against the suite now, three tests fail. That is the difference the
seam bought, and it is why the existing `ProcessorPlace` fixtures were kept rather than treated as
sufficient: they encode what a test author assumed the conversion produces, which is precisely the
thing that cannot catch the conversion being wrong.

## The claim word's width costs 2-3x in isolation and much less in use

Measured by `probe-queue-contention` for
[CHECKLIST-claim-word-layout.md](CHECKLIST-claim-word-layout.md) `CW-1.4`, on
one host, `x86_64-pc-windows-msvc`. Three apportionments of `reserving_mpsc`'s
claim word, built as duplicates in [claim_layout.rs](src/claim_layout.rs) so the
shipping crate was not disturbed: 32/32 and 16/48 over `AtomicU64`, and 64/64
over `AtomicU128`.

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

**Re-apportioning the bits is free.** 16/48 tracks 32/32 within noise in both
regimes, which is the expected result and worth stating as a confirmed
prediction rather than a discovery: both issue the same `lock cmpxchg` on the
same `u64`, so only the shift and mask constants differ. The 48-bit position
does force `head` and the per-slot `sequence` to 64 bits, and that cost does not
show up either. What this buys is the recurrence moving from 2^32 to 2^48 --
from about 37 seconds of sustained maximum-rate pushing to about 28 days.

**Widening the word is not free, and how much it costs depends entirely on the
regime.** Isolated, where the claim is the only thing happening, `cmpxchg16b`
costs 2-3x and the penalty *grows* with contention. Drained, with a consumer
running, it is 5-12%.

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
leaves twenty years. The last is the same practical answer a 128-bit word gives,
on a plain `AtomicU64`, at no measured cost, without a third-party dependency and
without reopening `D-18`'s i686 question.

So the candidates worth considering are **12/52 and 8/56**, not the 16/48 first
sketched here: 16/48's 12.7 days at the conservative floor is still reachable by
a busy long-lived process, and 12/52 is the first row that is not.

### Re-measured on the shipping type, and the duplicate had understated the wide word

`CW-1.6` deleted the duplicated protocol in this crate once
`windows-waitable-queues` took the layout as a parameter, so the probe now
instantiates the real type at each layout. The numbers below supersede the ones
above, which were taken from the stand-in.

| producers | 16/48 vs 32/32 | 8/56 vs 32/32 | 64/64 vs 32/32 |
|---|---|---|---|
| 1 | 1.02x | 0.98x | 1.45x |
| 4 | 1.04x | 1.01x | 1.33x |
| 8 | 1.05x | 1.05x | 1.59x |
| 16 | 1.21x | 1.21x | **3.83x** |
| 32 | 1.05x | 1.13x | **3.99x** |

**The finding about apportionment survives contact with the real type.** Both
`u64` re-apportionments track the default within noise, including `Perpetual`'s
8/56 -- so buying twenty years of headroom really is free, and it is now
measured on the code that ships rather than on something resembling it.

**The finding about width did not survive unchanged.** The duplicate reported
the 128-bit exchange at 2.37x and 2.99x at sixteen and thirty-two producers; the
real type reports 3.83x and 3.99x. The stand-in was *understating* the cost of
the layout it was built to evaluate, and by the widest margin exactly where the
decision is most sensitive. The conclusion is unaltered in direction and firmer
in degree.

**The residual offset is gone, which is the point of the deletion.** The
duplicate ran about 1.26x slower than `reserving_mpsc` at high producer counts,
an error that had to be carried as a caveat on every figure. Running the same
configuration twice through the shipping type now agrees within noise -- 50.3 ns
against 52.1 ns at thirty-two producers -- because both rows are the same code.

The general lesson is worth keeping even though the duplicate is gone:
**a stand-in is only evidence about the thing it stands in for while something
checks that it still does.** This one was checked, which is how the missing
cache padding was caught; but the checking only ever bounded the error, and the
bound was loose enough to hide a third of the wide word's cost.
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

Three probes needed more than a signature change, because they were composing a
`String` and calling `emit` directly rather than going through `emit_report` at
all: `core_affinity`, `peer_index_cache` and `queue_contention`. They are the
branch-local probes, and they had never been through the round that fixed the
same bypass in the peeled ones -- the crate's "every probe routes through this"
claim was false in three places until now. `core_affinity` also measured in
`main`'s argument list, so a failure to read the topology produced no banner and
no indication of which probe had died; it now measures inside the renderer,
after the banner, and reports a failed read as a failure to observe rather than
as a finding.

**Verifying that no report changed needed a control, because most of these
probes are not deterministic.** Comparing before and after directly showed
differences in nine of fifteen reports -- which proves nothing on its own, since
these probes print measured nanoseconds and render verdicts branching on them.
Running the *same* build twice showed differences of the same size or larger
(`peer-index-cache` 22 lines between two runs of one build, against 20 across
the conversion). The twelve deterministic reports were structurally identical.
A before/after diff on a probe is not evidence without that control.

### Measured: an interrupted probe keeps what it had already measured

M1.3 asked for this to be measured once rather than assumed, because it is the
property the whole milestone exists for and no unit test reaches it -- a test
cannot terminate its own process without taking the harness with it.

`probe-doorbell-cost` runs for about 0.8 seconds, which makes it the subject.
(It was **the** longest-running probe when this was measured, on a crate that
did not yet have `probe-queue-contention`'s ~65 seconds. The merge that brought
the branch-local probes back invalidated the superlative, not the measurement:
the numbers below are unchanged and were taken against the shorter probe, which
is the harder case -- a 300 ms window against 800 ms leaves far less room for a
slow start to masquerade as buffering than it would against 65 seconds.)
Started with stdout redirected, left for
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

## The defects that survived were correspondence failures, and no instrument here could see them

<a id="d-correspondence-failures"></a>

This probe was reviewed twenty-eight times before it opened as a pull request,
by two independent readers per round on different models, with `cargo-mutants`
reporting **zero surviving mutants** on both of its modules. A review on the
pull request then found, in code none of that had touched, a state where the
renderer printed

```
BUG IN THIS PROBE: the topology crate named L3 as the outermost
partitioning cache and this survey carries no summary for it. Nothing
below about cache partitioning can be trusted.
```

while `cross_check` had no branch for that state at all, so `verdict()` could
return `Agree` for the same run and print `=> agree` two paragraphs below. A
second finding in the same review had the same shape: one fact rendered twice
in one report -- `efficiency classes: [0]` in prose, `"efficiency_classes":1`
in the NDJSON -- in two shapes a consumer cannot reconcile, where the numeral
happens to read as a plausible class *label*.

Neither is a bug inside a function. Every function involved was correct on its
own terms, and each had been read repeatedly and found so. The defect lived in
the **relation between two artifacts**, and that is a place none of the
instruments in use could look.

### Why each instrument was structurally incapable, not merely unlucky

**Mutation testing cannot find absent code.** `cargo-mutants` perturbs what is
written and asks whether a test notices. A missing branch has no mutants, so
the missing `SummaryMissing` check did not lower the score -- it was invisible
to it. The 180/0 result was true and said nothing about the gap. A perfect
mutation score is compatible with an entirely missing feature, and this
component is the proof.

The same run also shows the weaker half of what a mutation score means. A test
existed asserting `"efficiency_classes":2`, so every mutant of that line died.
It was pinning the wrong shape faithfully. **Mutation testing measures whether
behavior is pinned by tests; it is silent on whether the pinned behavior is
right.** Both halves were over-read here for many rounds as though they were
evidence of correctness.

**Exhaustiveness checking protects `match` expressions, not concepts.**
`PartitioningCache` exists precisely to force a decision -- its own doc says a
renderer or serialiser "cannot emit the absent case without having decided
which absent case it is" -- and it worked, in the two consumers that wrote a
`match`. It bought nothing in the two that did not: `domain_counts` reached the
same information through `outermost_partitioning_cache`, a second accessor
returning `Option`, which launders five states into two; and `cross_check`
never asked. A type can only compel a consumer that consults it.

**Per-artifact review finds per-artifact defects.** Two readers checking each
function against its own documentation will confirm both sides of a
contradiction, because each side is locally true. Worse, the readers were
answering questions posed in a prompt, and across rounds that prompt
accumulated focus areas and "already verified, do not re-litigate" facts. The
shared prompt correlated the readers far more strongly than their differing
models decorrelated them; the instrument was being shaped to agree with its
author. Removing that framing in the final round is what got a reader to trace
`simultaneous_multithreading` out of this crate into `windows-topology-sys` and
check it against the Win32 `LTP_PC_SMT` contract.

The single sentence that covers all three: **every instrument in use verified
properties of things that exist.** Tests assert existing behavior, mutation
perturbs existing code, reviewers check written claims. A correspondence
failure is a property of a *pair*, and an absent branch is not a thing at all.

### Integration-level analysis was absent, which is where these live

At the time of the pull request the crate had one integration test, asserting
that a probe writes something to stdout. Of twenty-five `report()` calls in the
suite, **none rendered from a real host's `measure()`** -- every one used a
synthetic `Observation` built by hand. A hand-built fixture can only contain
states its author already imagined, and each assertion checked one local fact
about it. Nothing anywhere rendered the artifact a consumer actually reads and
asked whether it was self-consistent.

### What to do instead: a sparse matrix to explore with, an oracle to keep

The obvious response -- tabulate every state against every consumer and fill
the grid -- is wrong, and was proposed and rejected during this analysis. Such
a table grows combinatorially, most of its cells are meaningless, and a version
of it committed beside the code would be a second copy of the code's structure
that nothing verifies. It would rot exactly as every restatement in this
component rotted, and a stale "all cells covered" table is more dangerous than
no table.

The division that does work:

- **The matrix is a transient, exploratory instrument.** Draw it for one type
  at one boundary to find out which correlations exist. It is expected to be
  **sparse**; most cells are empty and discovering that is cheap. Correlations
  cannot be derived -- which is why twenty-eight rounds of reading produced
  none -- so populating it is exploration, not specification.
- **An oracle is the durable artifact.** Only cells that turn out to mean
  something graduate into it. It stays small because discovery, not
  enumeration, fills it.

`windows-file-watcher`'s `ContractChecker` is this repository's worked example
of the oracle half: a shared executable definition of the rules, owned by the
crate that owns the contract, that the producing crate's own tests and every
consumer's test doubles all bind to. It already existed while this probe was
being written, and was not reached for.

Three correlations are known to be real here, each because it was violated:

1. an alarm in the report implies the verdict is not `agree`;
2. a fact rendered twice must agree across its renderings;
3. an uncaveated hardware claim implies `!parse_in_doubt`.

What makes an oracle different from three more tests is where it is invoked: if
every test renders *through* it, all twenty-five existing call sites inherit
the checks and so does every future one. A test added beside them checks one
case; an oracle checks every case anyone ever writes.

**Record the vacuous findings too.** "We examined whether X and Y must
correspond, and they need not" is a result, and it is the half that normally
evaporates -- without it the next person re-explores the same empty cells.

An oracle is a forcing function for correlations already discovered. It will
not find a new one. The discipline that makes it compound is that each newly
found cross-artifact contradiction adds an invariant to the oracle rather than
a one-off test.

Whether this generalises to `Coherence`, `BracketOutcome`, `Verdict` and the
sibling probes is **an open question, deliberately not answered here.** The work
this decision implies is queued as M2 in [CHECKLIST.md](CHECKLIST.md); this
section schedules nothing on its own.

### The oracle exists, and what it deliberately refuses to know

M2.1 built it: [src/report_oracle.rs](src/report_oracle.rs), seeded with the
three correlations that are known to be real because each was violated.

**It reads the rendered artifact, never the state behind it.** Checking state
would miss precisely this defect class -- in the original finding the state was
consistent and the two *renderings* of it were not.

**It relates two things already visible in the report, and re-derives nothing.**
A second implementation of the rendering rules would be a check of the copy
rather than of the contract, and would drift the moment either moved. So the
alarm rule compares an alarm line against a verdict line, the double-rendering
rule compares prose against NDJSON, and the gating rule compares a claim against
the report's own published evidence of doubt.

That last one is the interesting boundary. `CrossCheck::parse_in_doubt` is
`!disagreements.is_empty() || !parse_incomplete.is_empty()`, and the NDJSON
publishes `parse_incomplete` as a **count** rather than the predicate -- so the
oracle reads the count and the `disagree` verdict, which are the two visible
shadows of that definition. The coupling is deliberate and is the thing M2.2's
sabotage check must confirm still holds.

**Half the tests assert acceptance**, following `ContractChecker`: an alarm
beside a non-agreeing verdict is legal and is what the fix produced, a caveated
claim under doubt is legal and is what the renderer emits on every heterogeneous
host with a short parse, and a prose-only report is silence rather than
violation. Over-constraining is the same defect as under-specifying and fails in
the more expensive direction, because noise trains a reader to ignore the
instrument.

#### The failure mode that would look exactly like success

An oracle whose prose labels do not match the renderer reads nothing, finds
nothing, and passes everything. So the labels were confirmed against a real
`probe-topology` run, and a test corrupts each double-rendered value in turn and
requires a violation -- if a label ever drifts, that test fails rather than the
oracle going quietly blind.

Then the oracle was run against a **real rendered report** with the historical
defect injected into it. It reported the contradiction twice, once for the prose
verdict and once for the NDJSON, and reported nothing on the same report
unmodified.

**The first attempt at that injection silently did nothing**, and is worth
recording because it nearly produced the opposite conclusion. The anchor used
was `cross-check:`, which does not occur -- the real text is `cross-check
against independently read Win32 counters:` -- so the "defective" report was
identical to the clean one, the oracle correctly reported no violation, and the
reading was almost "the oracle is blind". A sabotage that fails to apply is
indistinguishable from an instrument that fails to fire, unless the injection
asserts it changed something. It now does.

### Binding the oracle, and the measurement that shows it is not cosmetic

M2.2 bound the oracle inside `topology_report::report` and `report_unmeasured`
under `cfg(test)`, rather than at each of the 26 test call sites.

The placement is the whole difference between an oracle and three more tests.
Asserting at each site checks 26 cases and relies on the 27th author
remembering; asserting in the renderer checks every case anyone writes later,
**including the ones written to exercise something else**. That last part is not
incidental -- the original defect was found by a reviewer reading two paragraphs
together, not by a test aimed at it, so the cases most likely to catch the next
one are the cases nobody pointed at it.

**Both directions were measured**, because a binding that only moves when its
own test moves is cosmetic:

| | tests red |
|---|---|
| correspondence defect, binding in place | **13**, all in `tests`, none in `report_oracle::tests` |
| same defect, binding removed | **0** of 173 |

The defect used was the NDJSON emitting the processor count where the core count
belongs -- both renderings individually well-formed, so no per-part assertion
can see it. One of the 13 is `every_report_carries_the_banner_and_title`, which
exists to check the banner.

The second row is the one that matters. It says the existing suite cannot see
this class of defect at all, so the detection is genuinely new rather than a
restatement of assertions already present. Had only `report_oracle::tests` gone
red, the binding would have been reaching nothing.

**`cfg(test)` rather than always-on** is deliberate. A real probe run must still
print a contradictory report: a self-contradicting report is a finding *about
this probe*, and a panic that suppressed it would destroy the evidence a reader
needs. The real-host path is covered separately, by an integration test that
applies the oracle explicitly.

### The real-host test, and the guard that stops it passing for nothing

M2.3 added [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs).
It composes the report the way `probe-topology` does and applies the oracle
explicitly, because an integration test links the library without `cfg(test)`
and so does not inherit M2.2's binding.

**Why it had to exist.** All 26 in-crate call sites build their `Observation` by
hand, and a hand-built observation can only contain a state its author already
imagined. The oracle bound to those sites was therefore checking correspondences
over cases chosen by the same person who wrote the renderer. The defect the
oracle exists for was a state nobody had imagined. `measure()` reads the actual
host, and on CI that is the whole hosted-runner fleet -- the population where an
unimagined shape actually turns up.

**It asserts nothing about the machine**, deliberately. A test expecting a
processor count, a cache level or a verdict would fail on the next runner shape
rather than on a defect, and would be loosened until it asserted nothing. "The
report does not contradict itself" is checkable without knowing anything about
the host, including a host whose topology cannot be read at all.

#### The primary assertion can pass having checked nothing

If the renderer's prose labels drift from the oracle's, every lookup returns
`None`, every comparison is skipped, and the test passes. So a second test
corrupts each of the four double-rendered counts in **this host's own report**
and requires a violation for each. Corrupting one would have left the other
three pairs unguarded.

That guard was verified by widening one prose label by a single space. It failed,
naming `"packages":` and pointing at label drift -- **and the primary test passed
in the same run.** That pairing is the whole argument for the guard: the
assertion that matters went green while checking one fact fewer than it thought.

The corruption asserts it changed something before concluding anything, which is
the lesson from M2.1's first injection.

### The M2.4 matrix: what was examined, including what needed nothing

The instrument was a walk of every NDJSON field against the report's prose, and
every state enum against both. Recorded in full because **the cells that needed
nothing are the half that normally evaporates** -- without them the next person
re-explores the same ground and cannot tell an unexamined cell from an examined
one.

The prediction in M2.4 was that the matrix would be mostly empty. It was, along
the axis the item named, and was not along an axis the item did not.

#### Empty, and why

| cell | why nothing correlates |
|---|---|
| `Coherence` | Never rendered. It feeds the verdict and appears in no prose line and no NDJSON field, so it has one rendering and cannot contradict itself. |
| `BracketOutcome` | Rendered once, in the banner, as `HOST READINGS DISAGREE` / `HOST NOT ESTABLISHED`. No NDJSON counterpart. |
| `Verdict` | Already covered by M2.1's prose-against-`cross_check` rule. |
| `reason`, `arch` | NDJSON only. No prose renders them, so there is nothing to disagree with. |
| `cores with SMT` | Prose only. |
| `not_compared`, `parse_incomplete`, `enumeration_anomalies` | NDJSON only as counts. `parse_incomplete` is read by the gating rule, but as evidence rather than as a second rendering of a prose fact. |
| `GetNumaHighestNodeNumber` against `numa_domains` | **Examined and deliberately not correlated.** It reports the largest node *number*, which the report itself says is not a count, so comparing the two would manufacture a disagreement on any machine with sparse node numbering. A test pins this exclusion so it is not "fixed" later. |

#### Not empty, and promoted

The productive axis was not the state enums the item named but the **facts**: six
more were rendered twice with nothing comparing them.

| fact | prose | NDJSON |
|---|---|---|
| NUMA domains, and those without processors | `NUMA domains        : 1 (0 with no processors)` | `numa_domains`, `numa_domains_without_processors` |
| cache domains per level | the `caches:` table | `caches[]` |
| outermost partitioning level | `outermost cache that partitions...: L2` | `outermost_partitioning_cache_level` |
| domains per policy | the policy table | `policies{}` |
| active processor count | `GetActiveProcessorCount` | `processors` |
| active group count | `GetActiveProcessorGroupCount` | `groups` |

The last two are a **different rule shape** and the closest to what this probe is
for. The counters are read independently precisely so a mismatch is a finding, so
a counter contradicting the enumeration while the verdict reads `agree` is the
original defect in its purest form: the report printing its own contradicting
evidence directly above a verdict denying it. A must-accept test pins the legal
case, where the verdict reports the disagreement.

Reading `caches` and `policies` also forced the field reader to balance brackets
rather than stop at the first closer -- `caches` is an array *of objects*, so the
naive read returned its first entry and would have silently skipped every later
cache level.

#### The finding that goes beyond this milestone

`probe-doorbell-cost` and `probe-request-cost` render **every measured figure
twice** -- once in their prose table and once in their NDJSON line -- with
nothing comparing the two. That is the same class as the topology defect, in two
more probes, and it is not covered: the oracle's rules are written against
topology's prose labels.

That is a scope question rather than a mechanical follow-on, and is queued as
M2.9 rather than taken here.

### M2.9: two renderings that must match come from a common source

The M2.4 finding was that both cost probes rendered every measured figure twice
with nothing comparing the two. The obvious response was a third oracle rule
set. **The decision was that a fact rendered twice must be derived once**, so
both renderings now walk the same `Observation::timings` and a `json_key`
function decides only what the machine-readable one calls each entry.

That is strictly stronger than an oracle rule and it is cheaper. An oracle finds
a contradiction that already exists; deriving both from one value means there is
none to find. It also **deleted** code -- ten hand-named NDJSON fields and a
`get` closure went, because naming each figure separately was exactly what made
the two renderings independent restatements.

`json_key` panics on a label it does not know, which is the whole safety of the
scheme: a figure added to `measure` reaches both renderings or fails loudly, and
cannot reach one only. Verified by adding an unnamed timing -- the probe printed
its prose row and then died naming the missing key. (That the row appeared
before the panic is M1.2's streaming; the two milestones compose.)

**What is left to test is narrow, and that is the mark of the right fix.** The
derivation is structural in the source, so the only remaining question is
whether the structure survives rendering, formatting and the process boundary.
One integration test per probe runs the real binary and compares each table row
against its NDJSON field, reusing the crate's own `json_key` rather than
restating the pairing -- a test carrying its own copy would be checking the
copy, which is the defect this milestone is about.

Its emptiness guard fired on the first run: `request_cost`'s table has ratio
columns after the figure, so a parser requiring exactly two tokens matched
nothing and the test would have passed having compared zero rows. That is the
third time in M1-M2 that a check written to prevent a vacuous pass caught one
immediately.

#### Why the topology report is not converted too

Its prose and NDJSON are still written separately, guarded by the M2.1 oracle.
That is a real inconsistency and it is deliberate rather than overlooked: the
topology renderer's two sides are not one list rendered twice but many
individually-formatted claims, several with prose that has no NDJSON counterpart
and vice versa, so a common source is a much larger change than a `json_key`
map. The oracle covers it today and the eight cells M2.4 promoted are what make
that coverage real. Converting it is a decision available later, not a gap left
by accident.


### M2.5: the banner is built from the read the body describes

A `probe-topology` run makes **three** independent discoveries of the machine --
one before, `measure`'s own, and one after -- and the banner naming the host was
built from an *endpoint*. `attribution` compared only those two endpoints, so
when they agreed it printed their fingerprint unqualified, with nothing having
established that the middle read agreed with either. The line naming the machine
could therefore describe a different topology from the body beneath it, and the
report would say so nowhere.

**The uncovered window is narrow, and stating it exactly is the point.**
`measure` already brackets counter reads around its own discovery, so a
processor, group or NUMA change during the middle read is caught as
`BracketOutcome::Changed`. What no counter reaches is cache and
efficiency-class structure. The reachable case is a run whose cache structure
differs between the endpoints and the middle read while processor, group and
NUMA counts stay identical -- near-impossible on real hardware, since caches do
not change without processors changing, and entirely reachable on a hypervisor
returning inconsistent `GetLogicalProcessorInformationEx` results, which is
precisely the population this probe exists to survey.

**Construction, not a third comparison** -- the same choice M2.9 made, for the
same reason. A third comparison would be new prose able to drift from what it
compares; a banner built from the body's own read cannot disagree with it,
because there is no second value to disagree. `measure_observed` is a sibling of
`measure` returning the observation *and* `Fingerprint::from_topology` of the
very topology it parsed, so `measure`'s six existing callers are untouched.

The endpoint reads keep their job rather than being deleted: they bracket a
**wider** window than `measure`'s counter bracket, which spans only its own
discovery, so they still detect structural change the counters cannot see. They
simply no longer supply the banner.

**The fix nearly reintroduced the defect it removes.** The first attempt
formatted `host:  {fingerprint}` inline -- a second copy of a line whose owning
function documents, in the crate that owns it, that a probe's banner is
comparable with every other probe's only while exactly one place produces it. It
now routes through `banner_line_for`, wrapping in `Ok` to do so.

Measured, not read: sabotaging the banner back to the endpoint turns exactly one
test red. The real-host test does **not** catch that sabotage -- on a stable host
all three fingerprints are equal -- and its comment now says so. What it does
catch is the seam construction leaves open: `Fingerprint::from_topology` and
`observe` are two derivations from that one topology, each with its own filter
for which processors count, and they have already disagreed once, when
`from_topology` summed core-domain membership and printed `0p` for a machine
about to be measured on four processors.

### M2.7: probe steps are gated on the build, not on the job

All twelve probe steps in [ci.yml](../../.github/workflows/ci.yml) now carry
`if: "!cancelled() && steps.build.outcome == 'success'"`, against three before.

**The item posed this as a trade and it turned out not to be one.** Its argument
for guarding was already settled -- a probe step exists to emit diagnostics, so
Actions' default `if: success()` suppresses it in exactly the run that wanted it,
and the long-path pair is the sharpest case since either half alone "says
nothing". What kept it queued was the cost: a plain `!cancelled()` also runs the
step when the *build* failed, where `cargo run` cannot compile, turning a skipped
grey step into a failed red one.

Gating on the build takes both halves. A failing test still emits its
diagnostics; a broken build still goes quiet. The trade the first three steps
accepted is no longer necessary, so the decision the item reserved for an
engineer was answered by removing the thing being traded rather than by choosing
a side.

**Two defects found while implementing it, both by verification rather than by
reading.**

The first: `id: build` was added to the workspace build step, which lives in job
`build-test`, while every probe runs in `platform-probes`. `steps.build` does not
cross a job boundary, so the expression would have evaluated against an empty
context, made the condition permanently false, and **silently skipped all twelve
probes** -- a guard that reads as more careful while disabling everything it
guards. The probes job had no build step at all (its first step is `cargo test`,
which builds implicitly but whose outcome cannot separate "did not compile" from
"a test failed"), so one was added there.

The second was pre-existing and unrelated: a conflict resolution in merge
`1abcaaf` had welded a step's `if:` and `run:` onto one line, which is not valid
YAML. It survived the merge and the repository's own workflow gate, which checks
references by regex without parsing the document. Fixed, and the gap queued as
M34.5 in the root checklist.

Both are the same lesson this milestone keeps producing: the failure mode of a
check is to pass.
