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

## The oracle exists, and what it deliberately refuses to know

M2.1 built it: [src/report_oracle.rs](src/report_oracle.rs), seeded with the
three correlations that are known to be real because each was violated. The
defect that forced it is the section above.

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
shadows of that definition. The coupling is deliberate, and confirming it still
holds is what M2.2's sabotage check is for when the call sites are bound.

**Half the tests assert acceptance**, following
[../windows-file-watcher/src/contract.rs](../windows-file-watcher/src/contract.rs)'s
`ContractChecker`: an alarm beside a non-agreeing verdict is legal and is what
the fix produced, a caveated claim under doubt is legal and is what the renderer
emits on every heterogeneous host with a short parse, and a prose-only report is
silence rather than violation. Over-constraining is the same defect as
under-specifying and fails in the more expensive direction, because noise trains
a reader to ignore the instrument.

### The failure mode that would look exactly like success

An oracle whose prose labels do not match the renderer reads nothing, finds
nothing, and passes everything. So the labels were confirmed against a real
`probe-topology` run, and a test corrupts each double-rendered value in turn and
requires a violation -- if a label ever drifts, that test fails rather than the
oracle going quietly blind.

**The first attempt at that injection silently did nothing**, and is worth
recording because it nearly produced the opposite conclusion. The anchor used
was `cross-check:`, which does not occur -- the real text is `cross-check
against independently read Win32 counters:` -- so the "defective" report was
identical to the clean one, the oracle correctly reported no violation, and the
reading was almost "the oracle is blind". A sabotage that fails to apply is
indistinguishable from an instrument that fails to fire, unless the injection
asserts it changed something. It now does.

### The real-host test, and the guard that stops it passing for nothing

[tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs)
composes the report the way `probe-topology` does and applies the oracle
explicitly.

**Why it has to exist.** The oracle's unit tests pin it against fixtures, and a
fixture is a report somebody wrote down -- so a fixture-bound oracle checks
correspondences over states its author already imagined, and the defect it
exists for was a state nobody had imagined. More narrowly, a fixture cannot
notice the *renderer* drifting away from the prose labels the oracle reads:
both sides would still agree with each other. Only the real artifact disagrees.

Some unit tests in this crate do call `measure()` and so do read this host.
What none of them does is run the **oracle** over a report rendered from that
reading, which is the gap this test closes. On CI it runs across the hosted
runner fleet, a survey of shapes no fixture anticipates.

**It asserts nothing about this machine, deliberately.** A test expecting a
processor count, a cache level or a verdict would fail on the next runner shape
rather than on a defect, and would have to be loosened until it asserted
nothing. What it checks is that whatever this host produced, the report's parts
agree with each other -- a property every host must satisfy, including one whose
topology cannot be read at all.

#### The primary assertion can pass having checked nothing

On a host whose report the oracle cannot parse, every lookup returns `None`,
every comparison is skipped, and
`a_report_rendered_from_this_host_agrees_with_itself` passes having checked
exactly zero correspondences. That is why the second test corrupts each
double-rendered fact in a report **this host really produced** and requires the
oracle to report a violation -- and asserts first that the corruption changed
the text at all, for the reason recorded above.

Eight facts rather than one, because corrupting a single field would leave the
others unguarded: the renderer could drift away from the oracle's other prose
labels and the test would still pass on the strength of the one that remained.
