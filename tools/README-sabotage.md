# Sabotage sweeps

A green test suite is evidence that the code passes its tests. It is not
evidence that the tests would fail if the code were wrong. Those are different
claims, and only the second one tells you whether a guard you just wrote is
worth the lines it occupies.

[run-sabotage.ps1](run-sabotage.ps1) measures the second claim. It takes a
manifest of deliberate defects, and for each one: patches the source, runs the
suite, restores the source, and records whether the suite noticed.

```powershell
# What is in a manifest
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json -List

# Sweep it
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json

# Re-run one, after changing a test
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json -Name '*doorbell*'
```

It exits 0 only when every sabotage behaved as the manifest declared.

**This is an occasional instrument, not a CI gate.** Every sabotage forces a
rebuild, and any that is caught *as a hang* costs the full timeout. The
waitable-queues manifest takes upwards of twenty minutes. Run it when a guard is
written or changed, not on every commit.

## Reading a result, which is where the judgement is

**`caught`** -- the suite went red, or hung. The guard is real.

**`survived (NOT caught)`** -- the suite stayed green with the defect in place.
This is the finding worth having, and it means one of two things. Either the
tests have a hole, or **the sabotage is not a sabotage**. Check the second
before believing the first: the script prints the injected patch for every
unexpected result precisely so you can. A patch that inserts unreachable code
beside a live call, rather than deleting the call, changes the file without
changing the behaviour, and the suite then passes for the honest reason that
nothing was broken. That has happened in this repository, and it read as a hole
in the tests for a while before anyone looked at the patch.

**`MANIFEST STALE`** -- the pattern no longer matches exactly one site.
Refactoring moved the code out from under the manifest. Fix the manifest; the
sabotage was not run and proves nothing.

**`MANIFEST INERT`** -- the patch does not change the file at all.

## Controls matter as much as defects

A manifest should contain at least one entry with `"expect": "survives"`: a
change that is *not* a defect, usually the removal of an optimisation. It must
leave the suite green.

If a control is ever reported as caught, a test has started asserting the
implementation rather than the contract, and that test is the thing to fix. A
manifest with no controls can only tell you your tests are sensitive; it cannot
tell you they are sensitive *to the right things*.

## Three rules the script encodes, each learned by getting it wrong

**Judge by exit code, never by reading output.** A test process that dies of
heap corruption prints no `test result: FAILED` line at all. A harness that
greps for that string reports a hole in the tests where there is none, and the
time then spent hunting for it is pure loss.

**A timeout counts as caught.** A missing wakeup does not fail a test, it hangs
it. A harness without a bound hangs with it -- and a lost-wakeup defect that
hangs the suite has been detected exactly as intended, so a hang is a pass for
the tests, not a failure of the run.

**The baseline must be green before anything is patched.** Against an
already-red suite every sabotage "fails" and the sweep means nothing while
looking like a clean bill of health. The script refuses to start otherwise.

## Manifest format

JSON. `find` and `replace` are arrays of lines, joined with newlines --
line-array rather than one embedded string, so no backslash or newline ever
needs escaping.

```json
{
  "package": "windows-waitable-queues",
  "root": "../some/other/crate",
  "testArgs": ["-p", "windows-waitable-queues", "--locked"],
  "sabotages": [
    {
      "name": "push does not signal the doorbell",
      "file": "src/spsc.rs",
      "expect": "caught",
      "why": "A producer that never rings the bell leaves a parked consumer asleep.",
      "find": ["        self.shared.doorbell.signal();", "        Ok(())"],
      "replace": ["        Ok(())"]
    }
  ]
}
```

| Field | Required | Meaning |
|---|---|---|
| `package` | yes | Cargo package to test, unless `testArgs` overrides the command. |
| `root` | no | Where `file` paths resolve from, relative to the manifest. Defaults to the manifest's own directory. |
| `testArgs` | no | Replaces the arguments after `cargo test`. |
| `name` | yes | Unique; also the `-Name` filter key and the transcript filename. |
| `file` | yes | Source to patch, relative to `root`. |
| `expect` | yes | `caught` for a defect, `survives` for a control. |
| `why` | yes | What breaks, and why the suite should or should not notice. This is the part a future reader needs; the patch only says what changed. |
| `find` | yes | Lines to replace. Must match **exactly once**. |
| `replace` | yes | Replacement lines. `[""]` deletes. |

Keep the manifest **beside the code it sabotages** -- `sabotage.json` in the
crate root -- so a refactor and its manifest move together and a stale pattern
shows up in the same review.

## Writing a good sabotage

**Delete or invert; do not add.** The strongest patch removes the guard being
tested. A patch that adds something beside it risks changing the file without
changing the behaviour.

**One defect per entry.** Two at once cannot distinguish which test caught what.

**Target the guard, not the feature.** `pop` returning `None` unconditionally
will be caught by every test in the file and tells you nothing. Sabotage the
specific ordering, bound, or branch whose necessity is in question.

**Prefer the smallest patch that inverts the guarantee** -- swapping two
statements, flipping a `TRUE` to a `FALSE`, returning `None` from one accessor.
Small patches survive refactoring and stay readable in the failure output.

## Safety

Files are restored in a `finally` block and the restoration is verified by
comparing contents; if it cannot be restored the script stops immediately and
tells you the `git checkout` to run. Targets must be clean in git before a
sweep starts -- that is what makes an interrupted run recoverable -- and
`-AllowDirty` waives it if you accept the risk.

Transcripts land in `.scratch/sabotage/`, one per sabotage plus the baseline.
