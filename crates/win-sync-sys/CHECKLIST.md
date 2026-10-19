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

## WS-M2 -- An event's clones share one handle

- [ ] **WS-2.1** -- **`Event` holds a `SharedHandle`, and a clone shares it rather than duplicating.**
  **Proposed by the assistant on 2026-10-18 as the engineer's ruling of that day applied here; the engineer
  confirms before it is started.** The ruling, recorded in the request crate's
  [D-32](../windows-namespace-request-sys/DESIGN-NOTES.md#d-32): a handle duplicated for no reason but that the
  code began that way is legacy ("I want us to move forward as much as possible towards better interfaces and
  this counts"). `Event::try_clone` duplicates with the same access, which sharing gives without a new handle;
  `try_clone_wait_only` duplicates to narrow the access, which sharing cannot give, so it stays. The change:
  `Event` is `Clone` and `try_clone` is deleted; `WaitOnlyEvent` the same, since its clone keeps its access;
  `From<Event> for OwnedHandle` and `From<WaitOnlyEvent> for OwnedHandle` give way to a form that succeeds only
  for the last holder, as `SharedHandle::try_into_owned` does. The decision is recorded in
  [DESIGN-NOTES.md](DESIGN-NOTES.md) when it is made. Consumers that move with it: `windows-threadpool-sys`
  (`From<Event> for WaitableHandle`, [M-T15.1](../windows-threadpool-sys/CHECKLIST.md)), `win-durable-ioring`'s
  event clones, and the enumeration crate's `SAFE-3`. Tests: a clone shares the handle; setting through one is seen
  through the other; the event lives until the last clone drops; recovering an `OwnedHandle` fails while a clone
  is held.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/windows-threadpool-sys` -> `M-T15` ->
  > `M-T15.1`. See [CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md).

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
