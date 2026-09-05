# windows-platform-probes

Executable probes for the undocumented Windows behaviour this workspace's
designs rest on.

**Windows only. Not published** -- this crate exists to keep measurements honest,
not to be depended on.

**An experiment, not a component.** These probes measure platform behaviour and
are not for production use: that scope is what lets one do things a shipping
component must not -- change process-wide state, hang by design, require
privileges. Do not call them from production code, and do not lift a technique
out of here into a crate that ships.

## Why

Several decisions in this repository are justified by measurements of behaviour
Microsoft does not document, or documents differently from how it behaves. A
measurement that lives only in prose decays silently: the claim stays in the
design note while the platform, or our reading of it, moves. These probes exist
so a claim can be re-run rather than re-argued.

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

The binaries are the ones to reach for on a machine the test suite has not run
on -- notably an **x64** host, since every measurement recorded so far was taken
on ARM64.

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

**Queue claim contention, and the claim word's layout.** How `slotwise_mpsc`,
`reserving_mpsc`, and the experimental permit claim scale as producers are
added, against an uncontended `fetch_add` floor, in two regimes: producers alone
so the compare-and-swap is the only thing happening, and producers against a
continuously draining consumer. Reports each run's refusal count, so a run that
was bounded by the consumer rather than by the claim is visible as a fact rather
than mistaken for contention.

Also measures the four apportionments of `reserving_mpsc`'s claim word --
32/32, 16/48, 8/56, and the 128-bit 64/64 -- which is what established that
re-apportioning the bits is free while widening the word is not. That decided
how the layouts ship. The probe instantiates the shipping type at each layout
rather than a stand-in, and the reason is recorded in
[DESIGN-NOTES.md](DESIGN-NOTES.md): an earlier version carried its own copy of
the protocol and was found to be *understating* the cost of the layout it
existed to evaluate.
