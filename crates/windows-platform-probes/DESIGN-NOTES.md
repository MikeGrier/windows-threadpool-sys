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
