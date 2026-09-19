# Read/checksum experiment

An unpublished experiment supporting topology-planner `EP-R1.7`, not a production
planner or measurement-foundation API. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for the
measurement contract and the parent [CHECKLIST.md](../../CHECKLIST.md) for remaining work.
EP-X2.1's behavioral completion is in [COMPLETED-CHECKLIST.md](../../COMPLETED-CHECKLIST.md#ep-x21).
The first observations are in the [capture record](captures/2026-09-19/README.md).
The role-swapped repeats are in the
[EP-X1.1 capture record](captures/2026-09-19-ep-x1-1/README.md).
The parameter sweeps and pending result review are in the
[EP-X1.2 capture record](captures/2026-09-19-ep-x1-2/README.md).
The offline core/cache comparison and missing cross-NUMA evidence are in the
[EP-X2.1 capture record](captures/2026-09-19-ep-x2-1/README.md).

From the workspace root, build the `windows-read-checksum-experiment` package in
release mode, then run:

```powershell
.\target\release\windows-read-checksum-experiment.exe fixture .scratch\read-checksum.dat 33554449
.\target\release\windows-read-checksum-experiment.exe run .scratch\read-checksum-config.json .scratch\read-checksum-result.json
```

The scratch directory must exist. Fixture and report paths must not exist.
Put this JSON in the config file:

```json
{
  "file": ".scratch\\read-checksum.dat",
  "block_bytes": 65536,
  "depth": 8,
  "buffer_count": 32,
  "batch_size": 1,
  "queue_capacity": 4,
  "checksum_passes": 1,
  "repetitions": 6,
  "timeout_ms": 30000,
  "processors": null
}
```

Paths are resolved from the current directory. `processors: null` selects a
same-memory-domain, distinct-core pair with matching observed efficiency class.
An explicit pair is an array of `{"group": 0, "number": 0}`-shaped identities.
Selection is not a permission oracle; an actual binding refusal fails the run.
The capture retains requested and observed bindings and sampled payload-page nodes.

Each repetition runs direct-owner, bounded reader/processor handoff, and independent
direct workers in both processor orientations. The `read-checksum-v4` report labels
each trial with `reversed` relative to the capture's selected pair. Choose a multiple
of six repetitions for balanced treatment positions and within-repetition predecessor
pairs; see [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D7`. Warm-up runs are not recorded. Total payload
buffer capacity and maximum aggregate pending reads are the same across arrangements.
Queue, result and thread metadata are additional allocations.

Each trial's `resources` records participating CPUs, worker threads, checksum workers,
pending-read ceiling and payload capacity. `checksummed_blocks`, `read_capacity` and `buffer_capacity`
are recorded per worker. The direct path is labelled `unequal_cpu_reference`:
pipeline and independent paths both use two CPUs, but only independent workers
checksum on both. These contrasts do not isolate stage overlap from ownership and
handoff overhead.

After building release, run the bounded EP-X1.1 sequence from the workspace root:

```powershell
.\crates\topology-planner\experiments\read-checksum\capture-ep-x1-1.ps1 -OutputDirectory .scratch\ep-x1-1-retake
```

The directory must not exist. The script retains configs, reports and a derived
summary beside its fixture. It uses one executable and reuses the first capture's
processor pair for the remaining cases. It stops on a failed command.

The same driver runs EP-X1.2 with `-Study EP-X1.2`. Add `-PlanOnly` to print its
matrix without creating files or running the binary:

```powershell
.\crates\topology-planner\experiments\read-checksum\capture-ep-x1-1.ps1 -Study EP-X1.2 -OutputDirectory .scratch\ep-x1-2-retake
```

The exact protocol is [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D9`.
Reports include the configuration at every point; the driver retains a matrix,
raw reports and a derived summary with per-orientation control/point envelopes.
Range overlap is descriptive, not a statistical significance test. Direct and
independent workers are also same-code controls within the queue-capacity sweep:
they never construct the handoff queue.
`-SummarizeOnly -OutputDirectory <existing-capture-directory>` reads its saved
matrix and raw reports without running the executable, and creates
`summary-recomputed.json` without overwriting the original summary. Capture-script
and analysis-script identities are retained separately. A replay does not require
the original fixture to remain on disk.

`depth` limits aggregate pending reads; `buffer_count` independently limits payload
buffers and defaults to depth when absent or null. Both are powers of two, with
`2 <= depth <= buffer_count <= 1024`. Queue capacity is a power of two no larger
than buffer count. Total buffer bytes must not exceed 256 MiB.
`batch_size` defaults to one and accepts 1..=1024, including sizes exceeding the
pool or remaining input. It limits jobs per scheduling phase; phases stop early
on empty/full and do not wait for a full batch. It does not batch OS calls or
make queue operations atomic. Pipeline returns use a pool-sized return queue.

Per-worker `batches` counts nonempty processing batches (forwarding batches for
the reader); `return_batches` measures the reader's reclaim phase. Each records
jobs, maximum jobs, count and partial count relative to the requested quantum.
`buffer_pressure_observations` counts refill attempts with input remaining and
no free buffer, not stall durations. Payload occupancy is reported as per-reader
lease peaks; multiplying a peak by block bytes gives occupied buffer capacity,
not useful payload bytes in the short final block or a simultaneous global peak.

The fixture is read synchronously for per-block reference checksums before timing.
Trials use buffered asynchronous reads. This does not isolate storage-device service:
the OS cache, completion observation, checksum computation and handoff all participate.
`observed_read_latency` ends when the owner dequeues the completion, not when the device
finishes. `checksum_latency` ends when processing completes; it does not include wait
before admission. `compute_time` measures the checksum operation, including deadline
checks. `handoff_latency` includes full-queue waiting after completion observation.

## Offline placement and generated input

`input` defaults to `buffered_file`. Set it to `generated` with an explicit
`generated_bytes` to use the same deterministic logical bytes without opening the
`file` path. Generation fills each leased buffer on its producer inside timing;
this is not a prefilled transport-only benchmark. `observed_read_latency` then
measures generation service to readiness, not file or device latency. The report's
`evidence_class` distinguishes the two inputs. `file_bytes` retains its historical
field name but is the logical byte count for generated input.

`payload_node` defaults to null, retaining heap allocation on each reader. An
explicit Windows NUMA node requests `VirtualAllocExNuma`-backed buffers on that node
independently of worker bindings. Buffers stay owned through asynchronous completion
and rundown. `numa_backed_buffers` records the actual allocation backing; allocation
preference alone is not proof of page residency. Reservations and committed pages
may be rounded by Windows; the payload budget counts usable bytes, not all virtual
address space or allocator overhead.

`pages_before` and `pages_after` record payload observations. Unknown pages stay
unknown; `node_ids_truncated` means the working-set API's six-bit node encoding
cannot identify every node on this host. Such observations cannot establish that
a preference was achieved. Allocation, binding and query failures are errors.
Resource-exhaustion and driver/API failure conditions are propagated rather than
manufactured by the live suite.

Run discovery without a workload:

```powershell
.\target\release\windows-read-checksum-experiment.exe placements
```

The plan retains the full topology and one deterministic pair per observed
core/memory/data-cache relationship signature and endpoint-node-label availability.
It compares each discovered data/unified cache level separately, not a total
proximity rank; unknown and ambiguous memberships are explicit. Only online,
representable processors with matching observed efficiency-class status are paired.
Reported node labels come from Windows relationship observations, never from domain
positions. Reversing trial roles preserves the physical relationship but reverses
the pipeline's directed transfer. Synthetic selection tests are not hardware timing.

After building release, run the explicitly authorized offline capture:

```powershell
.\crates\topology-planner\experiments\read-checksum\capture-ep-x2-1.ps1 -OutputDirectory .scratch\ep-x2-1-retake
```

Add `-PlanOnly` to discover and print the matrix without creating files or running
workloads. [capture-ep-x2-1.ps1](capture-ep-x2-1.ps1) retains discovery, raw reports
and a summary. It brackets node-preference cases with heap controls and reverses
the node sequence. Each case has balanced arrangements and both role directions.
It stops on execution failures, preserving error and unrun-case records; unavailable
relationships are reported separately. The full protocol is
[DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D10`. This tool is offline research, not
part of topology planning or application startup.

`work_wall_ns` covers gate release through all measured worker phases and buffer
returns; `joined_wall_ns` additionally includes page sampling and thread teardown.
`worker_cpu_ns` sums kernel/user time from the participating threads, with the OS's
accounting granularity. `handoff_full_observations` counts failed enqueue attempts,
not unique stalls. Per-reader peaks cannot be added and called a measured global peak.

The executable emits one JSON status through its output writer. Failure returns a
nonzero exit code; once a report file has been created, execution failure is recorded
there too. Cooperative deadlines stop further work; they cannot make a driver finish
cancellation. Existing inputs are never overwritten.

Run the package's ordinary Cargo test suite for deterministic checks and live
file/thread cases. The live comparison requires a host with a selectable same-domain,
distinct-core processor pair. [sabotage.json](sabotage.json) supplies a wrong-offset injection
and a non-defect control for [run-sabotage.ps1](../../../../tools/run-sabotage.ps1).

## Faux-NUMA behavioral validation

The package's unit suite injects one test-only platform through the actual discovery,
placement-selection and scheduler paths. Binding, allocation, residency and CPU
observations all come from its shared scenario; generated inputs need no file or
completion port. Tests use sparse logical node IDs and synthetic processor groups,
record directed payload use, and verify release after normal completion and failures.
The CLI always uses the real Windows provider and has no synthetic-execution mode.

Faux reports are labelled `faux_numa_behavior_only_not_hardware_timing`. Their CPU
fields are zero placeholders and elapsed fields measure the host test harness, not
NUMA distances. No timing comparison is an acceptance criterion. Existing live tests
exercise the Windows path separately. The contract and path disposition are in
[DESIGN-NOTES.md](DESIGN-NOTES.md#rc-d11-consistent-platform-injection-through-actual-consumers).
Future production-component adoption and the one physical-hardware follow-up remain
in parent [CHECKLIST.md](../../CHECKLIST.md); EP-X2.1 is complete under that policy.
