# Checklist: win-sync-sys

Memory-safe Rust over the Win32 synchronization API. See [README.md](README.md) for what the crate is
and [DESIGN-NOTES.md](DESIGN-NOTES.md) for its decisions.

## WS-M1 -- `Event`, and the thread pool built on it

- [x] **WS-1.1** -- The crate exists, with an `Event` that sets and resets without `unsafe`, its tests and its sabotages. -> [completed 2026-10-07](COMPLETED-CHECKLIST.md#ws-11)

  > **-> CROSS-COMPONENT HANDOFF:** next work is in component `crates/windows-threadpool-sys` -> `M-T14` -> `M-T14.1` (`WaitableHandle` built on `Event`). See [CHECKLIST.md](../windows-threadpool-sys/CHECKLIST.md).

## WS-M1+ -- The rest of the kernel objects

Sequenced after `Event` by the engineer ([WS-D-2](DESIGN-NOTES.md#ws-d-2)); graduates to numbered
milestones when the first of these is taken up.

- [ ] **WS-1+.1** -- **The wait functions.** `WaitForSingleObject(Ex)`, `WaitForMultipleObjects(Ex)`
  with its 64-handle limit and both any and all, and `SignalObjectAndWait`, over a trait every
  object type here implements and `BorrowedHandle` implements too. The outcome as an enum:
  signalled or abandoned with an index, timed out, or ended by an APC in an alertable wait.
  `QueueUserAPC` belongs with it, as what ends an alertable wait.

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
  ([WS-D-1](DESIGN-NOTES.md#ws-d-1)); whether private namespaces belong here or with
  `windows-namespace-request-sys` is part of the question.

- [ ] **WS-inf.3** -- **Whether the workspace's private events move onto `Event`**: the ring
  crate's completion event (`windows-ioring-sys`' `ring/bookkeeping.rs`), `windows-waitable-queues`'
  doorbell, and `windows-file-watcher`'s `queue.rs`. Each is the engineer's decision, taken per
  crate.
