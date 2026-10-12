# Checklist: win-sync-sys

Memory-safe Rust over the Win32 synchronization API. See [README.md](README.md) for what the crate is
and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WS-M2 -- Waiting on one object

Taken up 2026-10-12 from durable-ioring's `DI-7.3`, whose pumped delivery has the consumer's own
thread wait on a backend's notification: a handle this workspace's safe code cannot wait on, since a
consumer that forbids `unsafe` has nothing to call. It is the first slice of `WS-1+.1`, which
[WS-D-2](DESIGN-NOTES.md#ws-d-2) sequenced after `Event`; the rest of that item stays parked below.

- [ ] **WS-2.1** -- **`wait_for_single_object`**: `WaitForSingleObject`, non-alertable, over any
  `AsHandle` -- an `Event`, an `OwnedHandle`, a `BorrowedHandle` -- with a timeout of `None` for no
  limit or a `Duration`, and `WaitOutcome` (`Signalled`, `Abandoned`, `TimedOut`; non-exhaustive, so
  the alertable wait's outcome can join it). A timeout is rounded up to whole milliseconds, so a
  nonzero wait is never a poll, and one the call cannot express (`INFINITE` is `u32::MAX`, so more
  than `u32::MAX - 1` milliseconds) is refused as invalid input rather than shortened. A failed wait
  is the error from `GetLastError`. Tests, each direction asserted: a signalled event is taken and
  an auto-reset one is consumed by it while a manual-reset one is not; a wait times out on an
  unsignalled event after at least its timeout, and a zero timeout never blocks; a wait wakes when
  another thread sets the event; a handle that cannot be waited on fails, and a closed one; a mutex
  held by an exited thread is `Abandoned`; the conversion of every timeout. Sabotages for each, in
  [sabotage.json](sabotage.json). A decision row records that this is the first slice of
  `WS-1+.1`, and `Event` still has no `wait` of its own, the wait being over any handle.

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/durable-ioring` -> `DI-M7` -> `DI-7.3` (pumped delivery). See [CHECKLIST.md](../durable-ioring/CHECKLIST.md).

## WS-M1+ -- The rest of the kernel objects

Sequenced after `Event` by the engineer ([WS-D-2](DESIGN-NOTES.md#ws-d-2)); graduates to numbered
milestones when the next of these is taken up.

- [ ] **WS-1+.1** -- **The rest of the wait functions.** `WaitForSingleObjectEx`,
  `WaitForMultipleObjects(Ex)` with its 64-handle limit and both any and all, and
  `SignalObjectAndWait`, over a trait every object type here implements and `BorrowedHandle`
  implements too. The outcome as an enum: signalled or abandoned with an index, timed out, or ended
  by an APC in an alertable wait. `QueueUserAPC` belongs with it, as what ends an alertable wait.
  `WaitForSingleObject` itself is `WS-2.1`, above, taken up first.

- [ ] **WS-1+.2** -- **`Semaphore`.** Created with an initial and a maximum count; `release(n)`
  returning the previous count. Gets `windows-threadpool-sys`' `From` conversion, as `Event` does.

- [ ] **WS-1+.3** -- **`Mutex`.** Thread-owned, so its release guard is not `Send`; a wait can
  report it abandoned. Gets **no** conversion into the thread pool's wait target
  ([WS-D-5](DESIGN-NOTES.md#ws-d-5)), and a `compile_fail` doctest should say so.

- [ ] **WS-1+.4** -- **`WaitableTimer`.** Set, including the high-resolution flag, and cancel. A
  kernel object one waits on, unlike a thread-pool timer, which runs a callback. Gets the thread
  pool's `From` conversion.

## M-inf -- Parked

- [ ] **WS-inf.1** -- **The in-process primitives**: SRW locks with condition variables (including
  waiting with the lock held shared, which std's `Condvar` cannot), critical sections with spin
  counts, one-time initialization, barriers, and `WaitOnAddress` on 1, 2, 4 and 8-byte values.
  Omitted to start with by the engineer ([WS-D-1](DESIGN-NOTES.md#ws-d-1)); the design question
  they bring is pinning, since none of them may move once in use.

- [ ] **WS-inf.2** -- **Named objects, `OpenEvent`, and private namespaces.** Not decided
  ([WS-D-1](DESIGN-NOTES.md#ws-d-1)). If taken up they belong here, and adding them changes nothing
  that exists; the open part is where an owned security-attributes type lives, since the one in
  `windows-namespace-request-sys` sits in a crate about the file-system namespace.

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
