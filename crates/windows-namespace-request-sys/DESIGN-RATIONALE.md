# Design rationale: windows-namespace-request-sys

Tier 2. How the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached --
alternatives considered, drafts that were wrong, and the reasoning that got
discarded along the way. Consulted for "why", never for "what is true now": if
this file and Tier 1 disagree, **Tier 1 wins**.

Cross-referenced by decision ID.

## D-18: `GetFullPathNameW` is not lexical

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-18`. What follows is
the record of getting there, which is unusually worth keeping because the same
mistake recurred in nine successive wordings, enumerated below.

### The shape of the error, which never changed

Every wrong draft did the same thing: **named a mechanism the evidence did not
reach.** The subject moved -- what the call does, what a number measures, which
alternatives cost less -- but the failure did not. That is why the decision
records the constraint rather than only the conclusion: a reader who takes only
"it is not lexical" away from D-18 has the answer without the thing that keeps
producing wrong answers.

### The drafts

1. **"This call is lexical."** The original text, contradicted by its own next
   sentence, which said it resolves against the process current directory. A
   lexical canonicalizer is a pure function of its input; this reads mutable
   process state.

2. **"It resolves relative components and `.`/`..` against the process current
   directory."** The first correction, which overshot. Collapsing `.`/`..` is
   pure string work and reads no process state at all -- `C:\a\..\b` becomes
   `C:\b` under any current directory, and whether or not `C:\a` exists. Only
   *rooting* reads process state. Measured under two different current
   directories.

3. **"The probe measures roughly 212 ns per resolution."** It does not. The
   probe reports a construct-and-drop cycle whose total contains an allocation
   and a drop, and its own output says so.

4. **"The probe deliberately declines to decompose that total."** An
   over-correction of (3). It does decompose: the report states that recycling
   an already-resolved path pays ~42 ns against ~210 ns and attributes the
   difference to the resolution. What the probe withholds is the **mechanism** --
   whether any of it is a kernel transition -- not the division.

5. **"Roughly 168 ns is this call's measured share."** Taking (4)'s split at face
   value. The subtraction spans the whole preparation step, which makes two heap
   allocations of the crate's own against the clone's one, plus a builder chain.
   Timed directly with no allocation in the loop, the call is about 110 ns --
   the draft overstated it by roughly half.

6. **"Either alternative is cheaper."** Said of `PathCchCanonicalizeEx` and
   `PathAllocCanonicalize` from the first commit onwards, and never measured.
   Nothing in this repository benchmarks them, Microsoft documents behaviour
   rather than relative cost, and `PathAllocCanonicalize` allocates its own
   result. Removed rather than substantiated, because the decision rests on
   rooting semantics and never needed it.

7. **"Touches no filesystem."** The claim that survived longest, because it is
   nearly right. What Microsoft documents is that the function does not verify
   that the resulting path is valid or names an existing file -- a statement
   about *verification*, not about I/O.

8. **"Observation cannot close that gap."** The correction to (7), and wrong in
   a more interesting way: observation *did* close it, in the affirmative
   direction, the moment anyone looked at the right form. Resolving `X:foo` for
   a non-current drive distinguishes an existing directory from an existing
   *file* from a missing one, and rewrites the `=X:` entry when it is not a
   directory. That is a filesystem query and a write to the process environment,
   in a call four drafts had described as reading process memory.

   The reasoning behind those drafts was sound -- the current directory is in
   the PEB, the `=X:` variables are in the environment block, both are process
   memory -- and the conclusion was false. It is the cleanest example in this
   file of the standing failure: a mechanism argued from where the data lives
   rather than measured. Every earlier draft at least *knew* it was asserting a
   mechanism; this one thought it was declining to.

9. **"A filesystem query rather than a syntax test."** Written in the paragraph
   correcting (8), and wrong the same way within a single round. Having found
   that an entry naming a missing directory or an existing *file* is rejected,
   the draft concluded the gate was existence and not shape. Measured, shape
   gates it too and independently: `C:/Windows/System32`,
   `C:\Windows\System32\.`, `C:\Windows\System32\..\System32` and
   `\\?\C:\Windows\System32` are each rejected while naming the same existing
   directory that `C:\Windows\System32` is accepted for. Acceptance is also
   literal -- `C:\Windows\` yields `C:\Windows\\foo`, with no normalisation at
   the join.

   The tell was the word *rather*. Ruling an alternative out is a strictly
   stronger claim than establishing the one you measured, and needs its own
   evidence; three observations of the existence check said nothing about
   shape. Both halves are now pinned by
   `a_rejected_drive_entry_is_replaced_by_the_drive_root` so the next draft
   cannot restate this from memory.

### Two facts that were measured, then asserted too narrowly

Both were found by review after the correction had already shipped, and both
were *enumerations* -- which is the form this kind of error likes.

- **The rooting forms.** Stated first as one case (the current directory), then
  two, and finally three: a relative path takes the current directory, a
  root-relative path takes only that directory's *root* (which is
  `\\server\share\` under a UNC current directory, so "current drive" was
  wrong), and a drive-relative path naming a drive OTHER than the current one
  takes the entry recorded for that drive from `=C:`, while on the current drive
  that entry makes no difference and the process directory wins. The
  current-drive arm is a fourth correction to the same enumeration, found the
  same way as the first three and after them: the count "three forms" was itself
  one of the things stated more confidently than measured.

- **The device set.** The short-circuit was first described as "exact-match
  only", which `CON:` disproves; then enumerated as `CON`/`NUL`/`PRN`/`AUX`/
  `COM1`-`9`/`LPT1`-`9`/`CONIN$`/`CONOUT$`, which omits the superscript
  spellings `COM\u{00b9}`, `COM\u{00b2}`, `COM\u{00b3}` and their `LPT`
  equivalents. Those are exactly the members a hand-written denylist misses, and
  the documentation asserted a closed list without them until a review measured
  it. The tests in [tests.rs](src/full_path/tests.rs) now pin every documented
  spelling so the next omission fails CI instead of a review -- each positive
  spelling against the device path it produces, and each negative one against
  its full rooted result.

  That last distinction was itself a later correction, and it is the reason the
  sentence now says which KIND of pinning each case gets. The claim "pins every
  documented spelling" was written while one negative spelling, `CON:x`, was
  covered only by `!starts_with("\\.\\")` -- a predicate the unrooted literal
  satisfies. So the claim was true of the list and false of the strength, which
  is the same shape as the trimming generalisation two entries below.

### The measurement that was itself unmeasured

The sharpest instance in this whole sequence is not about `GetFullPathNameW` at
all. The test helper that reads a `=X:` entry folded "empty value" and "absent
name" into one answer, and said so in a comment that called the equivalence
*measured*: `SetEnvironmentVariableW(name, "")` was reported to succeed and then
read back exactly as a name that was never set.

It does not. `GetEnvironmentVariableW` returns `0` for both, and the last error
is the only thing that separates them -- so a measurement that never cleared the
last error first could read nothing but whatever an earlier call had left there.
Cleared and re-measured, the two are distinct, for an ordinary name and an `=X:`
name alike:

| state | return | last error |
|---|---|---|
| set to `""` | `0` | `ERROR_SUCCESS` |
| deleted | `0` | `ERROR_ENVVAR_NOT_FOUND` |

An earlier note also recorded that `ERROR_ENVVAR_NOT_FOUND` "never surfaced even
for genuinely absent variables", which has the same cause and one more: the
deletion under test had not happened, because the null that deletes an entry had
been marshalled as an empty string instead. Two layers of the harness agreeing
with each other is not a measurement.

The consequence was live rather than cosmetic. `BorrowedDriveEntry` restores a
borrowed entry on unwind precisely so a panicking test cannot leak process
state; with the two answers collapsed, restoring an inherited *empty* entry
**deleted** it. The guard destroyed the state it existed to preserve, in exactly
one case, and only that case.

Pinned by `an_empty_drive_entry_is_distinguished_from_an_absent_one` in
[tests.rs](src/full_path/tests.rs), and verified by re-introducing the collapse,
which makes it fail.

The general form is worth keeping separately from the specific fact: **a
comment that says "measured" is a claim about a procedure, and the procedure can
be wrong in ways the result never reveals.** Every other entry in this file is an
assertion that outran its evidence. This one had evidence, and the evidence was
of something else.

### The sweep, and its arithmetic

The wrong word had spread well beyond where it was reported. The consuming
probe's checklist item named [full_path.rs](src/full_path.rs) only; that file
held **three of the nine** on its own -- the module doc and both doc examples --
with two more in [path.rs](src/path.rs), two in
[tests.rs](src/full_path/tests.rs), one in an acceptance comment and one in
[DESIGN-NOTES.md](DESIGN-NOTES.md). Nine across five files, from a report naming
one. The reported site was a sample, not the population.

### Why the alternatives were never seriously in contention

`PathCchCanonicalizeEx` and `PathAllocCanonicalize` canonicalize without
rooting, which sounds like a strictly smaller job and therefore an easy win. It
is the wrong trade for this crate: a path that is still relative has its meaning
settled on the worker thread at execution time, against a current directory any
thread may have changed in between. That is precisely the race preparation
exists to close, so the "smaller job" omits the part being bought.

Recorded here rather than only in Tier 1 so the next reader who notices the
cheaper-looking API does not have to re-derive why it was declined.
