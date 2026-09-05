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

Runs on **Windows PowerShell 5.1 and PowerShell 7 alike**, like the other
scripts in this directory, and the full sweep is verified on both. Worth stating
because the two differ in ways that bite here specifically: 5.1 rejects
PowerShell 7 syntax at parse time, and it reports a `Start-Process -PassThru`
exit code as `$null` unless the handle is held -- which this script would read
as "the phase failed", making every sabotage look caught while proving nothing.

**This is an occasional instrument, not a CI gate.** Every sabotage forces a
rebuild, and any that is caught *as a hang* costs the full test timeout. The
waitable-queues manifest takes about three minutes. Run it when a guard is
written or changed, not on every commit.

**Build and test are timed separately, and the split is what keeps the test
bound tight.** A hang is what a lost wakeup looks like and it happens during
test execution, so that phase gets a short bound (60s). A build is merely slow
sometimes, and a slow build killed by a short bound would be reported as a hang
-- crediting the tests with a detection that never happened -- so the build gets
a generous one (300s) and its failure is reported as its own outcome. Under a
single combined bound the number had to cover the slowest imaginable cold build,
which made every genuinely-hanging sabotage cost that same large number; the
split cut this manifest from twenty-five minutes to three.

Measured here: building the crate after a one-file edit takes under a second,
while test execution takes about twelve, nearly all of it compiling doctests --
`cargo test --no-run` does not build those, and Cargo offers no `--doc --no-run`
to pre-pay it. Pass `testArgs` with `--lib` if you want the sweep faster and
accept that a sabotage caught only by a doctest would then read as survived.

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

**`MANIFEST DOES NOT COMPILE`** -- the patch is not valid Rust, so the tests
never got a chance to notice it. Reported for a build-phase failure and,
separately, for `a doctest would not build`: `--no-run` cannot pre-build
doctests (cargo rejects `--doc --no-run` outright), so a patch that compiles in
the crate but breaks a `///` example gets as far as the run phase and fails
there with a compile error. Left unclassified that reads as `caught`, which is
the weaker claim wearing the stronger one's label -- so it is detected from
rustdoc's own `Couldn't compile the test.` marker and reported as the manifest
problem it is. Renaming or deleting a public item an example uses is the usual
way to land here.

## Beware the test that exercises a copy of the code

If a sabotage is caught only *sometimes*, the usual cause is not a slow machine.
It is that the test which was supposed to catch it deterministically tests a
hand-written duplicate of the logic rather than the real thing, leaving the real
path covered only by whatever races the scheduler happens to produce.

This is worth stating because it is invisible from a green suite and from a
passing sweep. It was found here only because a sweep was re-run under a tighter
bound and one result flipped from caught to survived: the guard's deterministic
test exercised a copy of the sequence with two statements swapped, so it proved
that *a* reversed order was wrong while being structurally incapable of noticing
the real one being reversed. Measured detection was one run in three.

**A flaky sabotage is a finding, not noise.** Re-run it a few times before
accepting either answer, and if it is intermittent, look for a duplicate of the
logic in the test rather than reaching for a longer timeout. The fix is usually
a `#[cfg(test)]` hook that lets a test drive the real code through the window in
question on one thread.

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
names the file to copy back.

**The pre-patch contents are written to `.scratch/sabotage/restore/` before any
file is touched**, and deleted once the restore is verified. That copy is what
makes recovery possible when the in-memory one is gone -- after a Ctrl+C, a
crash, or a `Stop-Process` -- so a file left in that directory means a run was
interrupted before it could restore.

**A sweep refuses to start while any such file is present**, and names them.
Otherwise the next run would overwrite the very copy this section tells you to
go looking for. Compare each against the file it names, copy it back if that
file is still sabotaged, then delete it.

**Targets must be inside the repository**, and `-AllowDirty` does not waive
that. A manifest's `root` may point anywhere, so the boundary is checked for
every target on every run: the switch waives the cleanliness check, not the
limit on what may be modified -- and it is the boundary that has no
`git checkout` behind it.

Targets must also be clean in git before a sweep starts, which additionally
makes `git checkout` a safe second recourse; `-AllowDirty` waives that check. Note
that it waives only the *second* recourse: once a target carries uncommitted
work, `git checkout` would destroy it, and the backup above is the only correct
recovery. The script's own messages say so rather than offering a `checkout`
that would discard your work.

Transcripts land in `.scratch/sabotage/`, one per sabotage plus the baseline.
Those this run may write are cleared before it starts -- and only those, since
the directory is caller-supplied via `-OutputDirectory` and nothing else in it
is the tool's to delete -- so a transcript named in an error message is always
from the current run. Listing a manifest with `-List` writes and deletes
nothing. Both a transcript and a backup are named after
the sabotage with non-alphanumerics collapsed to dashes, so two entries
differing only in punctuation would collide; the manifest is checked for that up
front and rejected rather than allowed to overwrite one entry's evidence -- or,
worse, one entry's recovery copy -- with another's. A stem of `baseline` is
rejected for the same reason: that name is taken by the baseline's own
transcript, which is the evidence that the suite was green before any patching. Build-phase diagnostics go to the `.build.err`
transcript, since cargo writes them to stderr, and error messages name whichever
of the two actually holds the evidence.
