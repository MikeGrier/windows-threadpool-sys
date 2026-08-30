# Design instructions: windows-ioring-sys

Binding design rules for this crate, in addition to the repository-wide
instructions. Locate them by the nearest-ancestor rule, the same as
[DESIGN-NOTES.md](DESIGN-NOTES.md).

## The borrow question

**Any change that adds a public function returning a borrow, or widens what an
existing one returns, must answer this question in the pull request:**

> 1. **What can safe code do with this**, and does the registration or the
>    kernel still hold anything it could invalidate?
> 2. **How long does the borrow last**, and what can safe code start *while it
>    is alive* that would invalidate it?

Both questions, every time. The second is not a refinement of the first -- they
have different answers, and asking only the first is how [D-45](DESIGN-NOTES.md#d-45)
survived an audit of the whole surface. `RegisteredBuffers::get` answered
question 1 correctly (it refuses while the kernel is writing) and question 2 was
never put to it: the check held at the instant of the call, while the returned
slice lived as long as the borrow, and `&self` let a caller start the very
operation the check exists to exclude.

The mechanical form of question 2 is: **take the returned borrow, then try to
call every other method that could start work against the same object.** If any
of them compiles, the check is a point-in-time check guarding a
lifetime-shaped hazard, and the receiver probably needs to be `&mut self`.

A borrow here means a reference, or a lifetime-carrying wrapper such as
`Batch<'_>` or `RingScope<'_>`. Widening includes returning a more capable type
over the same data -- `&mut [u8]` to `&mut Vec<u8>`, or a wrapper to the thing
it wrapped.

### Why this specific question, and why it recurs

This crate has shipped four defects of exactly one shape: a public method whose
return type permitted an operation nobody intended.

| | The return type | What it wrongly permitted |
|---|---|---|
| [D-35](DESIGN-NOTES.md#d-35) | `&mut Vec<u8>` | `reserve`, `resize`, and whole-value assignment, where only byte writes were intended |
| [D-36](DESIGN-NOTES.md#d-36) | `&[u8]`, unchecked | reading a buffer the kernel might still be writing into |
| [D-43](DESIGN-NOTES.md#d-43) | `&Mutex<IoRing>` | replacing the ring, silently stopping delivery |
| [D-45](DESIGN-NOTES.md#d-45) | `&[u8]` from `&self` | holding the borrow across a submit that makes the kernel write into that buffer |

None of these was careless. All four arrived through ordinary, well-reviewed
changes, and three of the four were caught only by a later review that happened
to ask this question. **What was missing was not diligence -- it was a specific
question being asked at a specific moment.** That is worth stating plainly,
because "review harder" is not a mechanism and would not have caught any of
them.

It is also the population no runtime technique reaches. A fuzzer, an oracle, a
guard-page allocator and a chaos harness all observe what code *does*; these are
defects in what code is *permitted* to do, and nothing has to execute for the
hole to exist. Review is therefore a **primary** technique for this crate rather
than a backstop, which is why it gets a written procedure instead of depending
on who happens to read the diff.

### What counts as answering it

A sentence per new or widened item, in the PR description, covering three
things. The audit in
[DESIGN-NOTES.md](DESIGN-NOTES.md#borrow-surface-audit-m181) is nineteen worked
examples of the form.

1. **What safe code can reach through it.** Not what callers are expected to do
   -- what the type *allows*. `&mut Vec<u8>` allows reallocation whether or not
   any caller reallocates.
2. **What the kernel or a registration still holds** that such an operation
   could invalidate, or that nothing does.
3. **Why the returned type is the narrowest one that serves the caller**, or
   what forced a wider one.

"No hole" is a complete answer and must still be written down. Sixteen of the
audit's nineteen rows say exactly that. An audit that records only its findings
cannot later be distinguished from one that stopped early.

### The check that makes it recur

[BORROW-SURFACE.txt](BORROW-SURFACE.txt) is a generated inventory of every
public function in `src/` whose return type carries a borrow.
`tools/check-borrow-surface.ps1` regenerates it and fails when it disagrees with
the source, so CI stops on any addition or widening. Refresh it with
`-Update` **after** answering the question, not before.

The check verifies *shape*, not correctness: it cannot tell a safe accessor from
a dangerous one, and it is not trying to. Its only job is to make the question
unavoidable at the moment the surface changes -- which is the moment all four
defects above went past.

Two consequences worth being explicit about:

- **A failure is not an accusation.** Most changes to this surface are fine. The
  check is asking for a sentence, not blocking a design.
- **Do not silence it by narrowing the check.** If it fires on something
  uninteresting, that is a cheap sentence in the audit table. A check tuned
  until it stops firing is a check that has stopped working, and this one exists
  precisely because the previous mechanism -- remembering -- failed four times.

## Design tiers

This component keeps Tier 1 only: [DESIGN-NOTES.md](DESIGN-NOTES.md) holds the
current decisions, with the rationale folded into each decision rather than
split into a separate `DESIGN-RATIONALE.md`. Extended design conversations go to
`design-sessions/`. Add Tier 2 if and when the decision entries start carrying
more history than decision.
