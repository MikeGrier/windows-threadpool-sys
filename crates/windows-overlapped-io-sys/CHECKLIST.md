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

## M16 -- More observer queries

M15 shipped the observer with one query, `file_len`. This milestone widens it to the other read-only
handle queries ([DESIGN-NOTES.md](DESIGN-NOTES.md) -> "observers share the handle", the bullet on what
qualifies as an observer query). Each query is behind `fs`, returns a structured result rather than one
method per field, and lands with tests of its own result. Independent of M14; it may be taken first.

- [x] **M16.1** -- `standard_info()` reads the whole `FileStandardInfo`; `file_len()` is its shortcut. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m161)

- [x] **M16.2** -- `file_id()` reads the volume serial and 128-bit file ID. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m162)

- [x] **M16.3** -- `file_type()` classifies any handle, in the core rather than behind `fs`. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m163)

- [x] **M16.4** -- `storage_info()` and `alignment_info()` read the sector geometry and buffer alignment. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m164)

- [x] **M16.5** -- The page size is asked of the OS, and `PageBuffers` aligns to what the device reports. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m165)

- [x] **M16.6** -- `remote_protocol_info()` reads the remote protocol, version, flags and SMB2 capabilities. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m166)

- [x] **M16.7** -- `basic_info()` reads the four times and the attributes, measured current through the handle. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m167)

- [x] **M16.8** -- `final_path()` and `final_path_as()` read the current path as `Wtf16String`, in every volume and name form. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m168)

- [x] **M16.9** -- `volume_info()` reads the label, serial, component limit, file-system flags and name. -> [completed 2026-10-14](COMPLETED-CHECKLIST.md#m169)

- [ ] **M16.10** -- **Every query against every backend, with an operation pending.** One table of the
  queries M16.1-M16.9 added, run through an observer while an operation pends on `AssociatedEndpoint`,
  `ThreadpoolIo` and `BlockingEndpoint`, as M15.5 did for `file_len`; each pending operation must still
  complete or cancel as it would without the queries.
