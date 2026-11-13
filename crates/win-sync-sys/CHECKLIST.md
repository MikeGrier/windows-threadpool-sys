# Checklist: win-sync-sys

Memory-safe Rust over the Win32 synchronization API. See [README.md](README.md) for what the crate is
and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WS-M1+ -- The rest of the kernel objects

Sequenced after `Event` by the engineer ([WS-D-2](DESIGN-NOTES.md#ws-d-2)); graduates to numbered
milestones when the next of these is taken up.

- [x] **WS-1+.1** -- The rest of the wait functions: alertable and multiple-object waits, `SignalObjectAndWait` and `QueueUserAPC`. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ws-1+1)

- [ ] **WS-1+.2** -- **`Semaphore`.** Created with an initial and a maximum count; `release(n)`
  returning the previous count. Gets `windows-threadpool-sys`' `From` conversion, as `Event` does.

- [ ] **WS-1+.3** -- **`Mutex`.** Thread-owned, so its release guard is not `Send`; a wait can
  report it abandoned. Gets **no** conversion into the thread pool's wait target
  ([WS-D-5](DESIGN-NOTES.md#ws-d-5)), and a `compile_fail` doctest should say so.

- [ ] **WS-1+.4** -- **`WaitableTimer`.** Set, including the high-resolution flag, and cancel. A
  kernel object one waits on, unlike a thread-pool timer, which runs a callback. Gets the thread
  pool's `From` conversion.

- [x] **WS-1+.5** -- A handle duplicated with a chosen access, and a wait-only event built on it. -> [completed 2026-10-12](COMPLETED-CHECKLIST.md#ws-1+5)

- [ ] **WS-1+.6** -- **Reach 0.1.0 at the next release, and re-pin the dependents with it.** The
  first release was 0.0.2, not the 0.1.0 the crate declared: the manifest was seeded at 0.0.1 and
  `bump-patch-for-minor-pre-major` turns a feature into a patch. Cargo reads `^0.0.2` as exactly that
  version, so the next release forces `windows-threadpool-sys` and `win-durable-ioring` to re-pin
  either way, and 0.1.0 is the number the crate meant to carry. The next commit that touches this
  crate and should release carries a `Release-As: 0.1.0` footer, **on a commit that touches only this
  crate**: release-please applies the footer to every package the commit touches, so one that also
  edited the dependents would request 0.1.0 of them too (a downgrade for `windows-threadpool-sys`).
  The dependents' version requirements move separately: read the release PR to see whether the
  workspace plugin rewrites them, and if it does not, set them in a commit of their own after it. The
  dependency-version check ([check-dependency-versions.cjs](../../.github/release/check-dependency-versions.cjs)) does not cover this,
  since it asks only that a version exist. Gated on this crate's next release.

  **Amended 2026-10-20: no `Release-As` footer is needed for the release that carries `WS-2.2`.** That release
  includes a breaking commit (`feat(win-sync)!`), and with `bump-minor-pre-major` a breaking change takes 0.0.2
  to 0.1.0 on its own. A pin there would also be refused by
  [check-commit-scope.ps1](../../tools/check-commit-scope.ps1), which rejects a pin followed by a break in the
  same release. What is left of this item is the dependents' re-pin after the release.

WS-M2 -- an event's clones share one handle -- is complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md#ws-22).

## WS-M3 -- A wait-only event safe code cannot reach the handle of

Requested 2026-11-13 by canopy (MikeGrier/canopy, its `RG-4`): canopy's doorbell is a protocol
over a manual-reset event, and its TLA+ model (`Doorbell.tla`) assumes nothing outside the protocol
changes the event's level. `WaitOnlyEvent` cannot set or reset, but it lends its handle, and safe
code can duplicate a borrowed handle with more access (`duplicate_handle_with_access`) and signal
through that. The engineer, 2026-11-13: the guarantee wanted is in the Rust types, not the
operating system's -- inside a process no DACL is a boundary, since code that handles raw `HANDLE`
values can reach the creator's full-access handle by value. With a type whose handle safe code
cannot reach, the model's environment is the holders of the `Event` plus `unsafe` blocks, which can
be enumerated. **The main deliverable is the multiple-object wait** (`WS-3.2`): a consumer waits on
the doorbell together with its own handles.

- [ ] **WS-3.1** -- **`SealedWaitOnlyEvent`, and the decision.** In [event.rs](src/event.rs) beside
  `WaitOnlyEvent`, which stays as it is (a wait-only handle a consumer can borrow is still an
  option):
  - made by `Event::duplicate_sealed_wait_only()` and by `From<WaitOnlyEvent>` (sealing only removes
    capabilities, so it is safe); `Clone` sharing one handle, `Send`, `Sync`, as `WaitOnlyEvent` is;
  - **no** `AsHandle`, `AsRawHandle`, `Into<SharedHandle>` or `try_into_owned`;
  - the only ways to its handle are `unsafe fn as_waitable_handle(&self) -> BorrowedHandle<'_>` and
    `unsafe fn into_shared_handle(self) -> SharedHandle` (the second is what
    `windows-threadpool-sys` builds a wait target from). Their contract: the caller only waits on
    the handle, never changes the event's state through it, never duplicates it with more access,
    and never lets it reach code that might. State that analyses built on this type rely on the
    contract -- that, not memory safety, is why it is `unsafe`.
  - Documentation of exactly what a holder can do, so a model can encode it: wait, and nothing else
    -- except that a satisfied wait on an **auto-reset** event consumes the signal, which is a reset
    a holder causes in safe code. What it does not cover: the `Event`'s holders, and `unsafe` code.
  - `compile_fail` doctests: `as_handle`, `as_raw_handle`, `SharedHandle::from` and
    `try_into_owned` are unavailable on it. Unit tests in the style of the wait-only ones in
    [event/tests.rs](src/event/tests.rs) (same object as the `Event`, not the same handle; refuses
    `SetEvent` / `ResetEvent` through the unsafe accessor; clones share; lifetime after the `Event`
    drops).
  - Decision `WS-D-11` in [DESIGN-NOTES.md](DESIGN-NOTES.md): why a type and not a DACL. Measured
    2026-11-13 and set aside: a security descriptor granting `SYNCHRONIZE` plus an `OWNER RIGHTS`
    (`OW`) ACE makes the kernel refuse widening a wait-only duplicate, but not use of the creator's
    full-access handle, which in-process code can reach by value.

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

## M-inf -- Parked

- [ ] **WS-inf.1** -- **The in-process primitives**: SRW locks with condition variables (including
  waiting with the lock held shared, which std's `Condvar` cannot), critical sections with spin
  counts, one-time initialization, barriers, and `WaitOnAddress` on 1, 2, 4 and 8-byte values.
  Omitted to start with by the engineer ([WS-D-1](DESIGN-NOTES.md#ws-d-1)); the design question
  they bring is pinning, since none of them may move once in use.

- [ ] **WS-inf.2** -- **Named objects, `OpenEvent`, and private namespaces.** Not decided
  ([WS-D-1](DESIGN-NOTES.md#ws-d-1)). If taken up they belong here, and adding them changes nothing
  that exists; the open part is where an owned security-attributes type lives, since the one in
  `windows-namespace-request-sys` sits in a crate about the file-system namespace. Duplicating a
  handle into another process, which `duplicate_handle_with_access` does not do
  ([WS-D-7](DESIGN-NOTES.md#ws-d-7)), crosses the same boundary and belongs here.

- [ ] **WS-inf.4** -- **`QueueUserAPC2` and the special user-mode APC.** From
  [WS-D-8](DESIGN-NOTES.md#ws-d-8): `QUEUE_USER_APC_FLAGS_SPECIAL_USER_APC` delivers the routine
  without the target being in an alertable wait, which changes what `queue_user_apc`'s documentation
  says about when a routine runs and what it may assume of the thread's state. Parked behind the
  Windows versions that have it and behind a decision on how a routine that can interrupt a thread
  anywhere is expressed, since [WS-1+.1](COMPLETED-CHECKLIST.md#ws-1+1)'s routine may assume the
  thread is at a wait.

- [ ] **WS-inf.3** -- **Whether the workspace's private events move onto `Event`.** Each is the
  engineer's decision, taken per crate. Found by sweeping non-test sources for raw `CreateEventW`,
  `SetEvent` and `ResetEvent` on 2026-10-07:
  - `windows-ioring-sys`: the ring's completion event (`ring/bookkeeping.rs`) and the resolver's
    event (`sys/resolver.rs`);
  - `windows-waitable-queues`: the doorbell (`doorbell.rs`);
  - `windows-file-watcher`: `queue.rs`;
  - `windows-file-enumeration-sys`: the completion ring's doorbell (`completion_ring.rs`), which
    already creates through `WaitableHandle::event` and sets and resets the handle raw -- the shape
    a kept `Event` clone replaces;
  - `windows-platform-probes`: `doorbell_cost.rs` and `pool_growth.rs`.
