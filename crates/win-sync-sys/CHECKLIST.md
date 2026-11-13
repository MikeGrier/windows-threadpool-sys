# Checklist: win-sync-sys

Memory-safe Rust over the Win32 synchronization API. See [README.md](README.md) for what the crate is
and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WS-M3 -- A wait-only event safe code cannot reach the handle of

- [x] **WS-3.1** -- Add `SealedWaitOnlyEvent` and record its Rust handle-access boundary. -> [completed 2026-11-13](COMPLETED-CHECKLIST.md#ws-31)

- [ ] **WS-3.2** -- **The waits accept it, alone or mixed with other handles.** In
  [wait.rs](src/wait.rs), [wait_many.rs](src/wait_many.rs) and [signal.rs](src/signal.rs):
  - a public, sealed `Waitable` trait (private supertrait, so no other crate implements it), whose
    one method lends the handle and is `unsafe` (a public trait's safe method is callable by anyone,
    which would reopen the leak); a blanket impl for every `T: AsHandle`, and impls for
    `SealedWaitOnlyEvent`, `&SealedWaitOnlyEvent` and `WaitObject`;
  - `WaitObject<'a>`, the mixture type, the counterpart of the `BorrowedHandle` slice the docs offer
    for a mixture today: `Copy`, `From<BorrowedHandle<'a>>`, made from a sealed event by
    `SealedWaitOnlyEvent::wait_object(&self)`, with **no** `AsHandle`;
  - `wait_for_single_object`, `wait_for_single_object_ex`, `wait_for_any`, `wait_for_all` and
    `signal_object_and_wait`'s `wait` take `impl Waitable` in place of `impl AsHandle`. Every
    existing call still compiles through the blanket impl (checked in a throwaway crate on
    2026-11-13: `&OwnedHandle`, `BorrowedHandle`, slices of each, and a slice mixing
    `sealed.wait_object()` with `handle.as_handle().into()`); confirm the workspace's dependents
    build unchanged.
  - Tests: a mixed `wait_for_any` reports the index of whichever is signalled, the sealed event or
    an ordinary one; `wait_for_all` over a mixture waits for both; an auto-reset sealed event is
    consumed by the wait; a timeout over a mixture takes nothing. `compile_fail` doctests: calling
    the trait's method outside `unsafe`, and `as_handle` on a `WaitObject`.
  - Docs: each wait function's "objects are" paragraph names the sealed event and `WaitObject`;
    [README.md](README.md).

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/windows-threadpool-sys` ->
  > `M-T17` -> `M-T17.1` (a threadless wait on a sealed event). See
  > [CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md#m-t17----threadless-waits-on-a-sealed-wait-only-event).

Verify, for each item: this crate's tests (`cargo_test -p win-sync-sys`), its doctests, and
`run-sabotage.ps1 -Since origin/main` on [sabotage.json](sabotage.json) once the item adds entries.
The release is [M-T17.2](../windows-threadpool-sys/CHECKLIST.md#m-t17----threadless-waits-on-a-sealed-wait-only-event),
which carries both crates.
