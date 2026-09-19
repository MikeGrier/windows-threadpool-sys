# Read/checksum experiment

An unpublished experiment supporting topology-planner `EP-R1.7`, not a production
planner or measurement-foundation API. See [DESIGN-NOTES.md](DESIGN-NOTES.md) for
the measurement contract and the parent [CHECKLIST.md](../../CHECKLIST.md), `MX1`
and `EP-X2.1`, for remaining work.
The first observations are in the [capture record](captures/2026-09-19/README.md).
The role-swapped repeats are in the
[EP-X1.1 capture record](captures/2026-09-19-ep-x1-1/README.md).

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
direct workers in both processor orientations. The `read-checksum-v2` report labels
each trial with `reversed` relative to the capture's selected pair. Choose a multiple
of six repetitions for balanced treatment positions and within-repetition predecessor
pairs; see [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `RC-D7`. Warm-up runs are not recorded. Total payload
buffer capacity and maximum aggregate pending reads are the same across arrangements.
Queue, result and thread metadata are additional allocations.

Each trial's `resources` records participating CPUs, worker threads, checksum workers,
pending-read ceiling and payload capacity. `checksummed_blocks` and `buffer_capacity`
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

The fixture is read synchronously for per-block reference checksums before timing.
Trials use buffered asynchronous reads. This does not isolate storage-device service:
the OS cache, completion observation, checksum computation and handoff all participate.
`observed_read_latency` ends when the owner dequeues the completion, not when the device
finishes. `checksum_latency` ends when processing completes; it does not include wait
before admission. `compute_time` measures the checksum operation, including deadline
checks. `handoff_latency` includes full-queue waiting after completion observation.

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
