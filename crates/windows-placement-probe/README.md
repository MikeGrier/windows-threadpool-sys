# windows-placement-probe

**Measures what thread placement costs on your machine, and prints a result you
can paste back.**

Where two communicating threads run changes how fast they can hand work to each
other -- by more than most optimisations are worth. On one machine measured so
far, moving a producer/consumer pair from one locality domain to another cost
**5.6x** on identical code.

## Why your machine is interesting

The designs this informs are shared; the hardware available to the author is
not. Two things are missing and cannot be fixed locally at any price:

- **Every host measured so far has exactly one NUMA node.** The cost of
  crossing between nodes -- what a multi-socket server does constantly -- is
  entirely unmeasured.
- **The two hosts measured express disjoint sets of placements.** Neither can
  produce a single row the other can, so this is not a matter of collecting
  more of the same. A result from a machine unlike either is worth more than a
  hundred repetitions here.

If your machine has more than one NUMA node, it can answer a question nothing
here can.

## Running it

Download the binary for your architecture from the
[latest release](https://github.com/MikeGrier/windows-threadpool-sys/releases),
then:

```text
placement-probe --preview    see exactly what it collects, measure nothing
placement-probe              measure, and print a result to paste
```

The run states its own worst-case duration before starting. It is usually a
second or two, and grows with the number of NUMA nodes.

Then paste the output into
[the collection thread](https://github.com/MikeGrier/windows-threadpool-sys/discussions/55).
The tool prints its own markdown fences, so you can select everything, copy, and
paste -- it will render correctly without you doing anything else.

## What it collects, and what it does not

**Collected:** the shape of the machine (logical processors, cores, cache
domains, efficiency classes, NUMA nodes), the CPU model, the OS build, whether
virtualisation was detected, and the timings it measures.

**Not collected:** your host name, your user name, file paths, environment
variables, serial numbers, or anything about installed software. That list is a
commitment, not a description of the current implementation.

**It makes no network connections.** It writes a file and prints text; sending
either is your decision and your action.

`--preview` shows the values it would collect **before** measuring, so you can
decide with the real values in front of you rather than a promise about them.
`--no-cpu-model` withholds the processor name.

### If the hardware is confidential, do not send the result

`--no-cpu-model` reduces incidental leakage and nothing more. An unreleased part
is identified by its **topology** -- an unusual core count, a novel cache
arrangement -- at least as well as by its name, and the topology is the
measurement. No switch fixes that, and it would be dishonest to imply otherwise.

## Trusting the binary

Run `placement-probe --version`. A binary built by this repository's CI reports
its commit and reads as official; anything else is marked `!!UNOFFICIAL!!`,
including a build from a local working copy. The same marking appears in the
result, so a submission always says which build produced it.

That distinction is why the download is the recommended path: an artifact
attached to a release here is traceable to the commit that built it, in a way a
local build of byte-identical source is not.

## What a result does not establish

These are **timing** measurements. They say nothing about memory ordering, and a
long clean run is not evidence of correctness in that sense -- stress testing is
measurably blind to a weakened ordering. The tool says so in its own output
rather than leaving it to be assumed.

## An instrument, not a library

This crate exists to produce measurements. It is not a placement policy, it does
not decide where your threads should run, and nothing in it is tuned for use in
a running system.
