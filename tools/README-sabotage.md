# Sabotage sweeps

A green test suite is evidence that the code passes its tests. It is not
evidence that the tests would fail if the code were wrong. Those are different
claims, and only the second one tells you whether a guard you just wrote is
worth the lines it occupies.

[run-sabotage.ps1](run-sabotage.ps1) measures the second claim. It takes a
manifest of deliberate defects, and for each one: patches the source, runs the
suite, and records whether the suite noticed.

**It never touches your working tree** -- the sweep runs against a copy, with
its own build directory. See [Safety](#safety-the-sweep-never-touches-your-working-tree)
below for what that buys and why it is the premise rather than a nicety.

```powershell
# What is in a manifest
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json -List

# Sweep it
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json

# Re-run one, after changing a test
.\tools\run-sabotage.ps1 -Manifest crates\windows-waitable-queues\sabotage.json -Name '*doorbell*'
```

It exits 0 only when every sabotage behaved as the manifest declared.

| Exit | Meaning |
|---|---|
| 0 | Every sabotage behaved as declared. |
| 1 | The sweep ran and at least one sabotage did not behave as declared. |
| 2 | Nothing was swept: the manifest or the invocation is wrong. |

A bad manifest is always the reported exit 2 with a message naming the file and
the field, never a raw PowerShell error: the tool is a diagnostic instrument, so
its own failures must not need diagnosing, and must not take a calling script
down with them. Missing file, invalid JSON, absent `package` or `sabotages`, an
empty `sabotages`, an unresolvable `root`, a missing per-sabotage field, and an
`expect` that is neither `caught` nor `survives` are each reported this way, as
is running from outside a git repository.

Runs on **Windows PowerShell 5.1 and PowerShell 7 alike**, like the other
scripts in this directory, and the full sweep is verified on both. Worth stating
because the two differ in ways that bite here specifically: 5.1 rejects
PowerShell 7 syntax at parse time, and it reports a `Start-Process -PassThru`
exit code as `$null` unless the handle is held -- which this script would read
as "the phase failed", making every sabotage look caught while proving nothing.

**This is an occasional instrument, not a CI gate.** Every sabotage forces a
rebuild, and any that is caught *as a hang* costs the full test timeout. Run it
when a guard is written or changed, not on every commit.

**Hangs dominate the wall clock, not builds**, so the hang bound is what moves
the total. An incremental rebuild is about four seconds; a hang costs the whole
bound. Measured on the 39-entry waitable-queues manifest: **853 seconds** at a
fixed 60-second bound, of which twelve hangs accounted for 720 -- and **335
seconds** once the bound is derived from the baseline instead.

**The bound is derived, not fixed.** The baseline runs the unmodified suite
first anyway, so its measured *test* duration is the best available statement of
how long this suite legitimately takes on this machine. The bound is

```
hang bound = max(TimeoutFloorSeconds, TimeoutMultiplier * baselineTestSeconds)
```

where `TimeoutFloorSeconds` defaults to 15 and `TimeoutMultiplier` to 3, so a
baseline whose tests take 4 seconds yields a bound of 15 (the floor beats 12).
The sweep prints what it derived and why:

```
Baseline is green in 4s. Hang bound: 15s (3x the 4s baseline, floor 15s).
```

A fixed number is wrong in both directions: too tight on a loaded machine or a
large suite, where a slow-but-finite run is scored as **caught** and quietly
inflates the result; too loose on a fast one, where every hang pays the
difference. Deriving it removes the guess.

**Raise it where it is actually needed, not everywhere.** `-TimeoutSeconds`
overrides the derivation for one run; a manifest may set `timeoutSeconds` for
its whole sweep; and a single sabotage may set its own for the case neither can
express -- one entry that legitimately runs far longer than the rest. A
per-entry value only ever *raises* the bound, because the reason to lower one is
speed and the cost of being wrong about it is a false `caught`.

(An earlier version of this file said this manifest took "about three minutes";
that was not reachable at the old fixed bound with twelve hangs in it, and the
claim is corrected here rather than propagated.)

**Build and test are timed separately, and the split is what keeps the test
bound tight.** A hang is what a lost wakeup looks like and it happens during
test execution, so that phase gets the short derived bound above. A build is
merely slow
sometimes, and a slow build killed by a short bound would be reported as a hang
-- crediting the tests with a detection that never happened -- so the build gets
a generous one (300s) and its failure is reported as its own outcome. Under a
single combined bound the number had to cover the slowest imaginable cold build,
which made every genuinely-hanging sabotage cost that same large number; the
split cut this manifest from twenty-five minutes to three.

Measured here: rebuilding the crate in the working copy after a one-file edit
takes about four seconds, while test execution takes about twelve, nearly all of
it compiling doctests --
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

Top level, describing the sweep:

| Field | Required | Meaning |
|---|---|---|
| `package` | yes | Cargo package to test, unless `testArgs` overrides the command. |
| `root` | no | Where `file` paths resolve from, relative to the manifest. Defaults to the manifest's own directory. |
| `testArgs` | no | Replaces the arguments after `cargo test`. Must not contain `--target-dir`; the sweep supplies its own. |
| `sabotages` | yes | The entries, described below. Must not be empty. |
| `timeoutSeconds` | no | Hang bound for the whole sweep, overriding the derived one. `-TimeoutSeconds` still wins over it. |

Inside each entry of `sabotages`:

| Field | Required | Meaning |
|---|---|---|
| `name` | yes | Unique; also the `-Name` filter key and the transcript filename. Two names that differ only in punctuation are rejected, as is `baseline`. |
| `file` | yes | Source to patch, relative to `root`. |
| `expect` | yes | `caught` for a defect, `survives` for a control. |
| `why` | yes | What breaks, and why the suite should or should not notice. This is the part a future reader needs; the patch only says what changed. |
| `find` | yes | Lines to replace. Must match **exactly once**. |
| `replace` | yes | Replacement lines. Either `[]` or `[""]` deletes -- the lines are joined with newlines, so both spell the empty string, and the shipped manifests use both. |
| `timeoutSeconds` | no | Hang bound for this entry alone, when it legitimately runs far longer than the rest. Only ever raises the sweep's bound. |

The two `timeoutSeconds` are different fields at different levels: one beside
`package`, one inside a sabotage.

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

## Safety: the sweep never touches your working tree

**It patches a copy.** The sweep keeps a working copy under
`.scratch/sabotage/tree/`, refreshed from your tree at the start of each run,
with its own cargo target directory at `.scratch/sabotage/target/`. Your files
are read and never written. Verified rather than asserted: a sweep is run with
all 570 tracked files fingerprinted by content hash *and* mtime before and
after, and the two sets match exactly.

This is a premise, not a precaution, and it is the same approach `cargo mutants`
takes. Patching the developer's own files means every sabotage needs a backup, a
restore, a check that the restore worked, a guard against running on a dirty
tree, and a recovery path for when any of that is interrupted -- and each of
those is a chance to damage work that was never in a commit. Against a copy none
of it exists. An earlier version of this tool did work in place, and most of its
defects came from that machinery rather than from the sweep itself.

Three consequences worth knowing:

**A dirty tree is swept exactly as it stands.** The copy is made from your
working tree, not from a commit, so uncommitted edits are what get measured --
usually the code whose guards you are asking about. There is no cleanliness
requirement and no `-AllowDirty` switch, because there is nothing to waive.

**Your build cache is not touched either.** The copy builds into its own target
directory, so a sabotaged artifact can never be left behind for the next
`cargo test` you run. That directory persists between sweeps, so builds stay
warm: measured here at about 4 seconds per sabotage, against 30 for a cold
build after the copy is first created.

**Getting this wrong costs a directory, not your work.** If the copy is ever
left in a bad state, delete `.scratch/sabotage/` and run again.

Which files are copied is decided by git -- tracked files plus untracked ones
that are not ignored -- so `target/` (28 GB here) and the scratch directory stay
out of it without a second exclusion list to drift from `.gitignore`. Only files
whose contents actually differ are copied, which is what keeps the builds warm;
files the source no longer has are deleted from the copy, so a rename cannot
leave a stale twin for cargo to compile.

## Transcripts

Transcripts land in `.scratch/sabotage/`, one per sabotage plus the baseline.
Those this run may write are cleared before it starts -- and only those, since
the directory is caller-supplied via `-OutputDirectory` and nothing else in it
is the tool's to delete -- so a transcript named in an error message is always
from the current run.

Build-phase diagnostics go to the `.build.err` transcript, since cargo writes
them to stderr, and error messages name whichever of the two actually holds the
evidence.

A transcript is named after its sabotage with non-alphanumerics collapsed to
dashes, so two entries differing only in punctuation would collide; the manifest
is checked for that up front and rejected rather than allowed to overwrite one
entry's evidence with another's. A stem of `baseline` is rejected for the same
reason: that name is taken by the baseline's own transcript, which is the
evidence that the suite was green before any patching.

**`-List` is inert**: it creates no directories, copies no tree, and writes and
deletes nothing. All of that setup happens after the listing path has exited.
