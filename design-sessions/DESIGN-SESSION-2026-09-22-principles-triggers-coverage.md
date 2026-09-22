# Design session 2026-09-22: principles, triggers, and measuring the gap between them

**Status: parked. Not approved, not scheduled, no tooling built.** Recorded so the idea and the
objection to it both survive to a later conversation. The absence of a checklist item is deliberate,
not an oversight.

**The engineer's standing objection, unanswered:** it is not clear this is operationalizable to the
degree claimed. That objection was raised after the proposal below was made and has not been
addressed. Anyone picking this up should treat the proposal as unproven and the objection as the
first thing to settle.

## The problem this came from

Over one long session, eight process errors were made against
[copilot-instructions.md](../.github/copilot-instructions.md). Sorting them by kind produced a
pattern sharp enough to be worth recording independently of any fix.

**Five of the eight were claims about this repository's own state** -- each answerable by a command
in seconds, each asserted instead from recollection:

| # | The claim | The fact |
|---|---|---|
| 1 | the epoch-commit trigger was "armed" for a reader raising `EPOCH_SIZE` | unreachable at any constants; measured over five values |
| 2 | a blast-radius sweep covered "13 files" | 14; the figure was read off `rg`'s grouped summary rather than counted |
| 3 | an item's two named sites were the population | a census found six |
| 4 | "`M22` is a testing-heavy milestone" | all three `M22` items touch only `examples/` |
| 5 | "`M23.1` touches the crate's contract surface" | it names the *sample's* `contract.rs` |

The other three were claims about Windows behaviour -- an error-kind mapping, `SubmitIoRing`'s
timeout result, and inline completion on a synchronous handle. Those are the class the kernel tests
and an independent review exist to catch, and they were caught.

Claims 4 and 5 are the sharpest, because they were written *into a milestone's sequencing rationale*
and claim 4 reversed that milestone's conclusion about when to schedule the work.

## The diagnosis

**Rules with a mechanical trigger fired reliably in that session. Rules requiring recognition that
they applied did not -- and the most carefully argued rules in the file are the ones breached.**

Not once forgotten: `--no-pager`, LF line endings, writing scratch output under `.scratch/`,
`cargo fmt` before a commit, the scratch-file commit message. Each fires on a discrete, visible act:
*running a git command*, *creating a file*, *committing*.

Breached repeatedly: CONTRACT INTEGRITY and FAIL FAST -- the two most heavily reasoned sections.
Their triggers are properties of text being generated, which is the worst possible moment to
introspect, because generation is the thing in flight.

A secondary effect: attention to any one principle decays as a session fills. The sweep census in
`M21.1` was run unprompted early on; twenty-odd tool calls later a file count was read off a summary
line without a thought. Same rule, same session.

## The engineer's objection to the obvious fix

The obvious fix is to write the trigger beside the principle. The objection to that is precise and
was raised before any of this was proposed:

> The problem with *me* writing the triggers rather than the principles is that the triggers are
> invariably narrower than the principle so then when the trigger is too narrow, it becomes like a
> legal argument about definitional terms rather than intent.

That is the rules-lawyering failure mode, and it is real. A trigger narrow enough to be mechanical
is narrow enough to be argued around. So a trigger cannot *replace* a principle; at best it samples
it.

## The proposal, such as it is

The idea is to stop arguing about whether a trigger is too narrow and measure it instead.

**Why it is tractable at all:** the space a principle describes cannot be enumerated -- that is what
makes it a principle. But the incidents that actually occurred can be. Each incident is a sampled
point in that space, and a trigger either would or would not have fired on it.

**The metric.** Coverage of *past* incidents is worthless as a health measure: it can be driven to
100% by adding one trigger per incident, which is the legalism trap with a green dashboard. The
measure that means something is the **escape rate on incidents recorded after a trigger was
written**. A trigger set that only catches what it was built for is narrow, by measurement rather
than by argument; one that catches novel incidents generalises.

**The twin failure is already covered by a rule this repository holds.**
[README-sabotage.md](../tools/README-sabotage.md) requires a manifest to carry an
`expect: "survives"` control, on the grounds that without one it "can only tell you the tests are
sensitive, never that they are sensitive to the right things". The same applies: the corpus needs
text that *looks* like a violation and is not. Then a trigger that is too narrow shows up as escapes,
and one that is too broad shows up as hits on the controls.

**Shape**, modelled on the sabotage harness because that pattern is already trusted here:

- principles: the existing `##` sections, referenced by heading and never copied;
- a trigger list: id, the tell, which principles it serves, the date it was added;
- an incident list: id, date, the actual offending text, the principle breached, how it was caught;
- a script that replays incidents against triggers, runs the controls, and reports the escape rate
  split by whether each incident predates or postdates the trigger.

## What was already conceded about it

Stated when the proposal was made, not extracted afterwards:

- **A script checks committed text; the triggers that matter fire during generation.** It can
  measure trigger quality. It cannot make a trigger fire. This is the same limit the sabotage
  harness has -- it tells you the tests are sensitive, it does not write them.
- **A subset does graduate to real enforcement.** "`because` followed by a reference to another file
  or item, inside a planning document" is greppable on committed text. That subset is a genuine gain
  regardless of whether the escape-rate signal ever proves useful.
- **The corpus would start at eight incidents, self-reported, from one session.** It says nothing for
  several sessions.
- **It is more machinery in a repository that already carries a great deal.**

## Kill criterion, agreed in advance

If after an agreed number of sessions every new incident still escapes every trigger, the approach
has failed and is deleted rather than extended. Written down first precisely because the instinct to
systematise is what produced a 146 KiB instruction file whose best-argued sections went unheeded.

## What happened instead, and what is actually in effect

One change landed, and it is not this: CONTRACT INTEGRITY rule 6 gained a clause covering
characterisations of sibling checklist items, with claims 4 and 5 above cited as its worked
examples, and naming **because** followed by a reference to another file or item as the tell.

That is a single trigger written beside a single principle. Whether that generalises, or merely
relocates the problem, is the question this note exists to reopen later.
