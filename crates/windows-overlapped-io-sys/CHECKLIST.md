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

## M15 -- Owned versus shared handles on endpoints

Requested by [win-multi-file-tail-watcher](../win-multi-file-tail-watcher/DESIGN-NOTES.md#tw-d-10), which
probes each file's length with no lock held while the same handle is, from its reads iteration on, an
endpoint. M15.1 settled the shape: the endpoint shares its handle with *observers* that can query but never
become a second endpoint ([DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the handle; only the
endpoint does I/O"). Independent of M14; it may be taken first.

- [x] **M15.1** -- Investigated owned versus shared endpoint handles; the engineer chose observers. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m151)

- [x] **M15.2** -- Endpoints keep their handle in shared storage and hand out observers. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m152)

- [x] **M15.3** -- The observer reads the file's length, and offers an `unsafe` borrow for other queries. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m153)

- [x] **M15.4** -- `ThreadpoolIo` keeps the endpoint's shared storage; landed with M15.2. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m154)

- [x] **M15.5** -- Tests: compile-fail doctests, observers across every backend, and cancellation and rundown with observers alive. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m155)

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/win-multi-file-tail-watcher`
  > -> `M1` -> `TW-1.2` (open each file as an endpoint and probe through an observer). See
  > [CHECKLIST.md](../win-multi-file-tail-watcher/CHECKLIST.md). M15.6 below does not gate it: the
  > watcher's endpoints are unassociated, then (M4+) `ThreadpoolIo`, neither of which waits on the
  > handle's event.

- [ ] **M15.6** -- **An observer's query must not strand `BlockingEndpoint`'s wait.** Measured
  2026-10-14: after an overlapped read completes on a file handle, the handle is signalled; one
  `file_len()` through an observer leaves it unsignalled (`WaitForSingleObject` -> `WAIT_TIMEOUT`).
  A synchronous query on the file object resets its event. `BlockingEndpoint::run` passes a null
  `hEvent`, so `GetOverlappedResult` waits on exactly that event: a query landing between the
  operation completing and the waiter blocking leaves it waiting forever. `EndpointObserver::handle`'s
  `# Safety` section invites the same queries. `BlockingEndpoint::observer`'s rustdoc and
  [DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the handle" now state the hazard; revise
  both with the fix. **Design decision pending with the engineer.** Pin the measured reset in a test
  whichever way it goes.
