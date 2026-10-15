# Checklist: windows-overlapped-io-sys

Completed milestones are archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md), and design decisions
are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

## M14 -- Finish the contract audit against the ten specification-gap categories

[DESIGN-NOTES.md](DESIGN-NOTES.md) -> "Specifying this contract" audits this crate against
[the ten categories](../../DESIGN-NOTES.md#specifying-a-delivery-contract) and reaches five of them: category
3 (`Issued`), category 10 (`post`/`post_raw`), categories 4/5 (`OperationId` generations, handled correctly),
and the previously-unstated completion-ordering rule. Categories 1, 2, 6, 8, and 9 were **not examined**, and
that absence is recorded as "not looked at" rather than "does not apply" -- the distinction the taxonomy
exists to keep visible.

- [ ] **M14.1** -- Audit the remaining five categories against the endpoint/association/submission surface:
  independent options that read as one concept (category 1, a strong candidate given `NotificationModes`'
  two independent flags and their asymmetric handle-versus-socket treatment); unconditional-read-as-
  probabilistic (2); which state a transition is entered from (6); values deliberately never correlated (8);
  and boundary-type fidelity at the consumer (9). State each answer, including "unspecified, deliberately".

- [ ] **M14.2** -- Sweep for the [`has_room`](../../DESIGN-NOTES.md#the-has_room-finding) shape: any advisory
  predicate this crate exposes that another subsystem's reliability gate depends on, checked for whether its
  stated contract holds under the condition its caller actually uses it in. `outstanding()` is the obvious
  candidate -- its meaning already changed once (a dequeued-but-held completion no longer counts), and
  `run_down` is a reliability gate that reads it.
