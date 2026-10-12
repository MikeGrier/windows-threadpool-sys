# Completed checklist: win-sync-sys

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-07 18:05:45 -04:00 -- WS-1.1: the crate, with `Event`

WS-M1 (`Event`, and the thread pool built on it) completed with this item and with
`windows-threadpool-sys`' `M-T14.1`, archived in that crate's
[COMPLETED-CHECKLIST.md](../windows-threadpool-sys/COMPLETED-CHECKLIST.md#m-t141); the
milestone heading and this item's stub left [CHECKLIST.md](CHECKLIST.md) together.

### <a id="ws-11"></a>WS-1.1 -- The crate exists, with an `Event` that sets and resets without `unsafe`, its tests and its sabotages. *(completed 2026-10-07 18:05:45 -04:00)*

The decisions it implements are [WS-D-3](DESIGN-NOTES.md#ws-d-3) and
[WS-D-4](DESIGN-NOTES.md#ws-d-4); the README is the crate documentation, so its example runs as a
doctest. Two details the item did not specify: the clone tests assert object identity directly
with `CompareObjectHandles` as well as by behaviour, with a second event as the comparison's
negative case; and the sabotage control is an equivalent change on code every `set` test runs
(`== FALSE` to `== 0`), rather than a change on an untested path. A harness run recorded every
entry in [sabotage.json](sabotage.json) as declared.

The item as it stood at completion:

- [x] **WS-1.1** -- **The crate, with `Event`.** Scaffold `win-sync-sys` as a workspace member,
  registered with release-please and the publish workflow like the other published crates, and
  build `Event` ([WS-D-4](DESIGN-NOTES.md#ws-d-4)):
  - `Event::new(ResetMode, initially_signalled)`, creating an unnamed event;
  - `set`, `reset` and `try_clone`, taking `&self` and returning `io::Result`;
  - `AsHandle`, `AsRawHandle`, and `From<Event> for OwnedHandle`;
  - `unsafe Event::from_owned_handle`, with WS-D-4's two conditions as its safety contract.

  No `PulseEvent`, with the reason in the crate's documentation as well as in
  [WS-D-3](DESIGN-NOTES.md#ws-d-3). The README is the crate documentation and its examples run as
  doctests. **Tests:** both reset modes and both initial states; that an auto-reset event releases
  exactly one waiter per `set`, while a manual-reset one stays signalled until `reset`; that a set
  event stays set across repeated `set`s rather than counting them; that a clone is the same object
  in both directions and outlives the original; `set` and `reset` from another thread; and `set`
  and `reset` failing on a handle without `EVENT_MODIFY_STATE`, which is how their error path is
  reached. `new`'s and `try_clone`'s error paths are reachable only by exhausting a kernel resource;
  say so at the definition. **Sabotage:** a `sabotage.json` whose entries swap set for reset,
  invert the reset mode, drop the initial state, make `reset` a no-op, swallow `set`'s error, and
  make `try_clone` create a new event; with a control that survives.

## Moved 2026-10-12 13:14:09 -04:00 -- WS-2.1, completing WS-M2: the single-object wait

### <a id="ws-21"></a>WS-2.1 -- `wait_for_single_object`: `WaitForSingleObject` over any handle, the first slice of the wait functions. *(completed 2026-10-12 13:14:09 -04:00)*

Taken up from durable-ioring's `DI-7.3`, whose pumped delivery has the consumer's own thread wait on
a backend's notification, a handle, in a crate that forbids `unsafe`. Per
[WS-D-6](DESIGN-NOTES.md#ws-d-6). `wait_for_single_object` and `WaitOutcome` in `src/wait.rs`;
a timeout is rounded up to whole milliseconds, one the call cannot express is refused as invalid
input, and the mapping of the call's return code is a function of its own so every code, a
failed wait and a code no wait gives included, is tested without making one happen. Tests: fourteen
unit tests, over events of both reset modes, an `OwnedHandle` and a `BorrowedHandle`, a signal set
from another thread under a wait with no limit, a handle without `SYNCHRONIZE` access for the
failing wait, and a mutex abandoned by an exited thread; and a running doctest in the README. A
closed handle was not tested as a failing wait, since safe code cannot name one; the handle without
access traverses the same edge. Ten sabotages caught, one of them as a hang rather than a failed
assertion, and a control survives. The rest of `WS-1+.1` stays parked. The item as it stood at
completion:

**`wait_for_single_object`**: `WaitForSingleObject`, non-alertable, over any `AsHandle` -- an
`Event`, an `OwnedHandle`, a `BorrowedHandle` -- with a timeout of `None` for no limit or a
`Duration`, and `WaitOutcome` (`Signalled`, `Abandoned`, `TimedOut`; non-exhaustive, so the
alertable wait's outcome can join it). A timeout is rounded up to whole milliseconds, so a nonzero
wait is never a poll, and one the call cannot express (`INFINITE` is `u32::MAX`, so more than
`u32::MAX - 1` milliseconds) is refused as invalid input rather than shortened. A failed wait is the
error from `GetLastError`. Tests, each direction asserted: a signalled event is taken and an
auto-reset one is consumed by it while a manual-reset one is not; a wait times out on an unsignalled
event after at least its timeout, and a zero timeout never blocks; a wait wakes when another thread
sets the event; a handle that cannot be waited on fails, and a closed one; a mutex held by an exited
thread is `Abandoned`; the conversion of every timeout. Sabotages for each, in
[sabotage.json](sabotage.json). A decision row records that this is the first slice of `WS-1+.1`,
and `Event` still has no `wait` of its own, the wait being over any handle.

> **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M7` -> `DI-7.3` (pumped delivery). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## Moved 2026-10-12 15:58:00 -04:00 -- WS-1+.5: a handle duplicated with a chosen access

### <a id="ws-1+5"></a>WS-1+.5 -- A handle duplicated with a chosen access, and a wait-only event built on it. *(completed 2026-10-12 15:58:00 -04:00)*

Per [WS-D-7](DESIGN-NOTES.md#ws-d-7), the assistant's choices queued for the engineer's review in
durable-ioring's `DI-9.7`. Built: `duplicate_handle_with_access(handle, Access)` over any handle,
returning an `OwnedHandle`; `Access`, a typed mask with `SYNCHRONIZE` and `EVENT_MODIFY_STATE` named,
`|`, `from_bits` and `bits`; `Event::try_clone_wait_only`, returning `WaitOnlyEvent`, which has no
`set` or `reset`, lends its handle, converts into an `OwnedHandle`, and clones to another wait-only
handle. The README documents it with a doctest.

**A claim in the item was false, and a test found it.** The item as written said the function would
narrow; the first run of a test written to expect Windows to refuse a wider duplicate found that it
does not. A duplicate of a `SYNCHRONIZE`-only handle asking for `EVENT_MODIFY_STATE` succeeds and can
set the event. The documentation of the function, of `WaitOnlyEvent`, of the README and of durable-ioring's
`DI-7+.1` now say what the guarantee is, and a test records the platform behaviour so a Windows that
changed it would show. The tests observe a duplicate's powers through raw `SetEvent`, `ResetEvent`,
`WaitForSingleObject` and `CompareObjectHandles`, never through this crate's own, so a defect in the
duplication cannot hide behind what it duplicated. Nine sabotages and a control are in
[sabotage.json](sabotage.json), and one existing entry was re-anchored because the new type's
`try_clone` made its line anchor ambiguous.

The item as it stood at completion:

- [x] **WS-1+.5** -- **Duplicate a handle with chosen access.** `DuplicateHandle` with an explicit
  access mask rather than the same access, on `Event` and on an `OwnedHandle`, so a holder can be
  given a duplicate that can wait but not set or reset (`SYNCHRONIZE` alone). The first consumer is
  [DI-7+.1](../durable-ioring/CHECKLIST.md), a read-only duplicate of the ring's own completion
  event; the primitive is not specific to it. The open design question is the type: a duplicate
  that cannot signal should not offer `set` or `reset`, so it is probably a distinct wait-only type
  rather than an `Event` that fails at run time.
