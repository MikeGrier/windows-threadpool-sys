# Design rationale: windows-namespace-request-sys

Tier 2. How the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached --
alternatives considered, drafts that were wrong, and the reasoning that got
discarded along the way. Consulted for "why", never for "what is true now": if
this file and Tier 1 disagree, **Tier 1 wins**.

Cross-referenced by decision ID.

## D-18: `GetFullPathNameW` is not lexical

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-18`. What follows is
the record of getting there, which is unusually worth keeping because the same
mistake recurred five times in five different wordings.

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
   about *verification*, not about I/O. Observation cannot close the gap:
   resolving a path under a directory that does not exist shows no check was
   made, not that no filesystem was touched.

### Two facts that were measured, then asserted too narrowly

Both were found by review after the correction had already shipped, and both
were *enumerations* -- which is the form this kind of error likes.

- **The rooting forms.** Stated first as one case (the current directory), then
  two, and finally three: a relative path takes the current directory, a
  root-relative path takes only that directory's *root* (which is
  `\\server\share\` under a UNC current directory, so "current drive" was
  wrong), and a drive-relative path takes that drive's own current directory
  from `=C:`.

- **The device set.** The short-circuit was first described as "exact-match
  only", which `CON:` disproves; then enumerated as `CON`/`NUL`/`PRN`/`AUX`/
  `COM1`-`9`/`LPT1`-`9`/`CONIN$`/`CONOUT$`, which omits the superscript
  spellings `COM^1`, `COM^2`, `COM^3` (U+00B9, U+00B2, U+00B3) and their `LPT`
  equivalents. Those are exactly the members a hand-written denylist misses, and
  the documentation asserted a closed list without them until a review measured
  it. The tests in `full_path/tests.rs` now pin every documented spelling so the
  next omission fails CI instead of a review.

### Why the alternatives were never seriously in contention

`PathCchCanonicalizeEx` and `PathAllocCanonicalize` canonicalize without
rooting, which sounds like a strictly smaller job and therefore an easy win. It
is the wrong trade for this crate: a path that is still relative has its meaning
settled on the worker thread at execution time, against a current directory any
thread may have changed in between. That is precisely the race preparation
exists to close, so the "smaller job" omits the part being bought.

Recorded here rather than only in Tier 1 so the next reader who notices the
cheaper-looking API does not have to re-derive why it was declined.
