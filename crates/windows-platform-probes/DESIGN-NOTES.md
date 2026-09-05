# Design notes: windows-platform-probes

Decisions for this crate. Pending work is in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md), milestone M27.

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
