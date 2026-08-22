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
same file exercises different timing at a different seed without being edited.

**Keep bounds above Windows's scheduling floor.** `std::thread::sleep` cannot sleep for less than the OS
scheduling quantum -- commonly cited as ~15.6ms, though this crate's own stress runs measure an effective
floor closer to ~23ms -- so a `(1, 20)` bound rounds every draw up to the same one tick, silently turning
"irregular" timing into a fixed delay. Prefer something like `(25, 250)`:

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
          { "WaitRandom": { "low": 25, "high": 250 } },
          { "CreateFile": { "path": "marker.txt" } },
          { "WaitRandom": { "low": 25, "high": 250 } }
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

The full scenario library the `scenario_stress` test suite runs is checked in as loose JSON files under
[`../../tests/scenarios/`](../../tests/scenarios/) -- every file there is also a valid `run-scenario`
input. Beyond the basics above, several files there are worth a look for less obvious composition:

- `nested_repeat.json` -- an `Operation::Repeat` nested inside another, showing that repetition composes
  rather than being a single flat loop.
- `multi_directory_churn.json` -- churn spread across several sibling directories, not just one.
- `narrow_watch_on_subdirectory.json` -- a second watch scoped to a subdirectory (non-recursive), alongside
  the implicit root watch every scenario gets, so a change outside the narrow watch's path is still seen by
  the root watch but not by it.
- `directory_tree_rename.json` -- a directory containing a file is renamed as a whole, then a new file is
  created inside it at its new location.
- `rapid_session_name_reuse.json` -- the same session/watch name is opened, used, and closed five times in
  a row with **no** delay between rounds -- see the WaitRandom timing-floor note above for why this is a
  meaningfully different posture from spacing the same rounds out.

```powershell
cargo build --features scenario-tool --bin run-scenario
target\debug\run-scenario.exe ..\..\tests\scenarios\fast_two_entity_swap.json
```
