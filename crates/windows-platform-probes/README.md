# windows-platform-probes

Executable probes for the Windows behaviour this workspace's designs rest on --
supported, documented APIs, in the specific cases their documentation does not
describe.

**Windows only. Not published** -- this crate exists to keep measurements honest,
not to be depended on.

**An experiment, not a component.** These probes measure platform behaviour and
are not for production use: that scope is what lets one do things a shipping
component must not -- change process-wide state, hang by design, require
privileges. Do not call them from production code, and do not lift a technique
out of here into a crate that ships.

## Why

Several decisions in this repository rest on behaviour the Windows
documentation does not describe: a guarantee stated for one case and silent
about the next, an error path whose result is unstated, an interaction between
two APIs that neither page covers.

**Not the undocumented-API kind.** Nothing here loads `ntdll`, calls an `Nt*`
entry point, or reaches past the supported surface in any other way -- which is
usually what "undocumented Windows behaviour" is taken to mean. Every probe
calls an API Microsoft documents and supports, in a way it supports, and the
manifest requests only `Win32_*` surfaces. What is undocumented is the *answer
to a particular question* about that call: not whether we may make it, but what
it does when we do.

One probe's notes do quote an `ntdll!` frame -- `probe-cancel-io` records where
a stack trace showed a thread blocked while a hang was being diagnosed. That is
evidence about what happened, not an entry point this crate calls.

Those answers are what a measurement can settle and prose cannot. Measuring
here usually means establishing a relation rather than a quantity -- whether two
APIs report the same thing, whether state is shared between handles, whether one
setting is independent of another -- so the result is most often a yes, a no, or
a description of how two readings differ. A few probes do time something. A
measurement that lives only in prose decays silently -- the claim stays in the
design note while the platform, or our reading of it, moves -- so these probes
exist to make a claim re-runnable rather than re-arguable.

Where the platform has not committed to an answer, the probe records an
observation rather than a contract, and says which it is. That distinction is
the point of measuring: an answer we can re-run is worth having precisely
because it tells us when to stop trusting it.

Each probe's logic is a **library function that returns its observation**. The
binaries print it and the tests assert it, so the fact has exactly one
implementation -- writing the check twice, once to print and once to assert,
would make the test a check of the copy rather than of the platform.

## Three tiers

| Tier | What it means |
|---|---|
| **Asserted** | Fast, deterministic, no side effects outliving the call. A real `#[test]`, so a platform change fails the build instead of quietly falsifying a design note. |
| **Ignored** | Correct to assert but too slow, too resource-hungry, or dependent on a specific environment. `#[ignore]`d with the reason and cost stated. |
| **Binary only** | Cannot be a test at all -- hangs by design, mutates process state irreversibly, or needs privileges a test run must not assume. |

Every tier is **compiled** by an ordinary workspace build. That is the floor: a
probe that no longer compiles has already rotted.

## Running

```text
cargo test  --package windows-platform-probes     # the asserted tier
cargo run   --bin probe-error-mode                # observations, including the irreversible one
cargo run   --bin probe-handle-state
```

The binaries are the ones to reach for on a machine the suite has not run on,
and running them there is the most useful thing anyone outside this repository
can do with this crate.

## Please run these, and tell us if one disagrees

These findings are meant to be properties of *Windows*, not of the machines
that happened to measure them. That is a claim this repository cannot finish
checking on its own. Every measurement here was originally taken on ARM64,
which left the obvious question open; an x64 CI job later re-took the ones it
can run and every qualitative finding held, which is recorded finding-by-finding
in
[DESIGN-NOTES.md](DESIGN-NOTES.md#d-x64) -- including one row honestly marked
*evidence void* rather than counted as agreement. One probe is deliberately
absent from that job and so has never been cross-checked: `probe-cancel-io` is
binary-only precisely because `CancelSynchronousIo` can block indefinitely --
that being the finding -- so running it in CI would hang the job by design.

But two architectures on two Windows versions is evidence, not proof, and nobody
here can run every edition, configuration and build that exists.

So if a probe contradicts what is written here on your machine, that is a
result worth having, and the maintainers would genuinely like to know. It means
one of two things and both matter: the finding is narrower than it was stated
to be -- true of a version, an edition, a configuration, rather than of Windows
-- or the platform has moved since it was measured. Either way, a design note
in this workspace is resting on it and needs to hear about it.

A probe that fails is doing its job. That is the whole reason the measurements
are executable instead of written down.

### The space is much larger than the architecture

Cache and NUMA topology is the clearest case. It varies by vendor and model, by
SKU within a model, and -- for one physical CPU -- by firmware settings that
split a socket into several NUMA nodes or change which cache levels are
reported as partitioning the machine. Add processor groups, which appear above
64 logical processors and change the shape of everything above them; hybrid
parts with more than one efficiency class; and virtual machines that present
topologies no physical board has. The number of distinct shapes a Windows
machine can hand to these APIs is not something a few developer boxes and a CI
runner can enumerate.

We think the coverage is enough for reasonable confidence. We would still much
rather have concrete results than think so, and every genuinely different
machine is worth more than another instance of one already seen.

### On sharing anything back

**Please do not feel any obligation.** A topology report describes your machine
in real detail -- processor and core counts, cache domain sizes, NUMA layout --
because that is exactly what makes it useful, and the same properties make it
identifying. Whether that is yours to share is your call, and possibly your
employer's. We would rather you ran the probes and shared nothing than did not
run them.

Useful things you can do, in descending order of effort and none of them
expected:

- Send a report, if you are able to. The more different the machine, the more
  it is worth.
- Say only that a probe disagreed, and which one. That alone tells us a stated
  finding is narrower than we believed, which is the part that matters.
- Run them purely for yourself, and simply know whether the designs in this
  workspace rest on anything that is not true where you are. That is a
  perfectly good reason for this crate to exist.

## What is measured

**Thread error mode.** Which `SEM_` bits `SetThreadErrorMode` accepts;
that an invalid bit fails the *whole* call rather than being dropped from it;
that the thread error mode is independent storage rather than a view of the
process mode. Binary-only: that `SEM_NOALIGNMENTFAULTEXCEPT` cannot be cleared
once set at process scope -- irreversible, so no test performs it.

**Handle state.** That `DuplicateHandle` shares directory-enumeration state with
its source, with the control that makes that attributable; that closing a
duplicate leaves the source usable; and that single-shot metadata queries do not
disturb an enumeration in progress, on the handle or on a duplicate.
