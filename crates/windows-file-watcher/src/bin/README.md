# run-scenario

A command-line tool that replays a persisted JSON scenario file through the same data-driven stress
model/harness the crate's `scenario_stress` integration test uses (see `../../CHECKLIST.md` M9). It exists
so a stress scenario can be authored, saved, and re-run without writing or recompiling Rust code.

**The JSON schema this tool reads is not part of `windows-file-watcher`'s semver contract.** It is a
testing/ops tool input, not a documented data format -- see the module docs on
[`windows_file_watcher::scenario`](../scenario.rs) for the full rationale. Only the crate's normal Rust API
surface carries the usual stability guarantee.

## Building and running

The tool is gated behind the `scenario-tool` feature, so an ordinary build of `windows-file-watcher` never
links `serde`/`serde_json` or produces this binary:

```powershell
cargo run --features scenario-tool --bin run-scenario -- <scenario.json>
```

or, once built:

```powershell
cargo build --features scenario-tool --bin run-scenario
target\debug\run-scenario.exe <scenario.json>
```

It prints the run's `HarnessOutcome` (batch/change/desync/... tallies) and exits non-zero if the file
cannot be read or does not parse as a scenario.

### Reproducibility

Every run draws its `WaitRandom` timing from the same seeded PRNG the test suite uses (see
[`windows_file_watcher::scenario::seed`](../scenario.rs)): fixed by default, so a given scenario file
always replays identically, or overridden per run:

```powershell
$env:WINDOWS_FILE_WATCHER_STRESS_SEED = "12345"
target\debug\run-scenario.exe <scenario.json>
```

## The scenario format

A scenario is a JSON object with a `label` and an ordered `operations` array. Each operation is one of the
`Operation` enum's variants (see `../scenario.rs` for the authoritative list), externally tagged the way
`serde` derives by default -- `{ "<Variant>": { <fields> } }`. `Wait`/`WaitRandom` durations are plain
integers, in milliseconds. Paths are relative to a fresh temp directory the tool creates for the run.

### Example: a couple of file creates and a rename

```json
{
  "label": "smoke",
  "operations": [
    { "CreateFile": { "path": "a.txt" } },
    { "Wait": { "duration": 5 } },
    { "Rename": { "from": "a.txt", "to": "b.txt" } }
  ]
}
```

### Example: repeating a pattern without unrolling it

`Repeat` lets a scenario describe hundreds of thousands of operations as a small file -- this repeats one
file create 2,000 times rather than listing 2,000 separate `CreateFile` entries:

```json
{
  "label": "churn",
  "operations": [
    { "Repeat": { "count": 2000, "pattern": [{ "CreateFile": { "path": "churn-0.txt" } }] } }
  ]
}
```

### Example: irregular delete/wait/reintroduce timing

`WaitRandom` draws its duration from `[low, high]` (milliseconds) using the run's own seeded PRNG, so the
same file exercises different timing at a different seed without being edited:

```json
{
  "label": "delete-wait-reintroduce",
  "operations": [
    { "CreateFile": { "path": "marker.txt" } },
    {
      "Repeat": {
        "count": 10,
        "pattern": [
          { "RemoveFile": { "path": "marker.txt" } },
          { "WaitRandom": { "low": 1, "high": 20 } },
          { "CreateFile": { "path": "marker.txt" } },
          { "WaitRandom": { "low": 1, "high": 20 } }
        ]
      }
    }
  ]
}
```

### Example: session and watch lifecycle churn

Beyond filesystem actions, a scenario can open/close named sessions and subscribe/cancel named watches
mid-run (every scenario also gets one implicit session/watch on the temp root for free). This one opens a
second session, watches from it, touches a file, then tears both down:

```json
{
  "label": "two-sessions",
  "operations": [
    { "OpenSession": { "name": "second" } },
    { "Subscribe": { "session": "second", "watch": "second-watch", "path": "", "subtree": true } },
    { "CreateFile": { "path": "seen-by-both.txt" } },
    { "CancelWatch": { "watch": "second-watch" } },
    { "CloseSession": { "name": "second" } }
  ]
}
```

## More examples

The full scenario library the `scenario_stress` test suite runs is checked in as JSON fixtures under
[`../../tests/scenarios/`](../../tests/scenarios/) -- every file there is also a valid `run-scenario` input:

```powershell
cargo build --features scenario-tool --bin run-scenario
target\debug\run-scenario.exe ..\..\tests\scenarios\fast_two_entity_swap.json
```
