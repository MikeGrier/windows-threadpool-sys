# windows-threadpool-sys

Memory-safe Rust access to the Windows thread pool APIs.

The Windows thread pool integrates work, timers, waits, and asynchronous I/O
with the operating system's own scheduling facilities. Its distinguishing
property is that an idle workload costs no threads at all: a process waiting on
timers, events, or I/O holds no dedicated thread stacks. This crate wraps those
facilities while making callback and resource lifetimes explicit in Rust.

## The object types

Each thread-pool object is an owned Rust type whose `Drop` performs the
documented teardown for that object, so a callback can never outlive the state
it captured.

| Type | Wraps | Runs the callback when |
|---|---|---|
| `work::ThreadpoolWork` | `TP_WORK` | you submit it |
| `timer::ThreadpoolTimer` | `TP_TIMER` | a due time arrives, once per arming |
| `timer::ThreadpoolPeriodicTimer` | `TP_TIMER` | every period, until stopped |
| `wait::ThreadpoolWait` | `TP_WAIT` | a handle signals or a wait times out |
| `io::ThreadpoolIo` | `TP_IO` | an overlapped operation completes |

One-shot and periodic timers are separate types on purpose. The platform models
both with one object and a `period` argument, which hides the property that
matters most when writing the callback: a `ThreadpoolPeriodicTimer` may queue its next
tick while the previous one is still running, so its callback must tolerate
overlapping with itself. A `ThreadpoolTimer` never overlaps, and re-arming it from inside
its own callback gives repetition whose gap is measured from the end of each
firing.

Three supporting types shape where those callbacks run and how they are torn
down: `pool::ThreadpoolPool` is an owned private pool,
`callback_env::CallbackEnviron` is the environment that selects a pool and a
callback priority when an object is created, and `cleanup_group::CleanupGroup`
releases many objects in one step instead of dropping each individually. The
callback environment provides Rust equivalents of the SDK's header-only inline
helpers, which `windows-sys` cannot emit.

A cleanup group creates its own members, so the borrow checker prevents using
one after the group has released it. Thread-pool I/O is deliberately excluded: a
`TP_IO` object must not be closed while an overlapped operation is outstanding,
and a bulk release cannot satisfy that precondition.

## Example

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use windows_threadpool_sys::work::ThreadpoolWork;

let count = Arc::new(AtomicUsize::new(0));
let counter = Arc::clone(&count);

let work = ThreadpoolWork::new(move || {
    counter.fetch_add(1, Ordering::SeqCst);
}, None).expect("create work");

for _ in 0..4 {
    work.submit();
}
work.wait();

assert_eq!(count.load(Ordering::SeqCst), 4);
```

More examples, including timers, waits, private pools, and overlapped I/O, are
in the [API documentation](https://docs.rs/windows-threadpool-sys).

## Callback rules

Callbacks run on shared, process-managed threads, so every object type holds its
callback to the same contract: restore any thread state you change, do not
terminate the thread, and do not block waiting on your own object's rundown. A
callback may panic without breaking the pool — every trampoline catches
unwinding at the FFI boundary, because unwinding into the pool's frame is
undefined — but the panic is contained rather than reported, so a callback that
cares should catch its own errors.

## Safety highlights

Each type carries the invariant its SDK object needs, rather than leaving it to
the caller to remember:

- **Waits own their handle.** A waitable handle must stay valid while a wait is
  pending, so `ThreadpoolWait` takes an `OwnedHandle` and hands it back only as
  a borrow. The handle cannot be closed underneath a pending wait.
- **Callbacks get a token for the operations only they can perform.**
  `TimerFiring::rearm_after` re-arms a one-shot from inside its own firing,
  `PeriodicTick::stop` lets a periodic timer end itself, and
  `WaitActivation::rearm` re-arms a wait — which the SDK requires per activation.
- **Cleanup-group members are protected at compile time.** Members borrow the
  group and `close_members` takes `&mut self`, so using a member after the group
  released it is a borrow-check error rather than a documented rule.
- **Panics cannot unwind into the pool.** Every callback trampoline catches
  unwinding at the FFI boundary, and the accounting stays exact when it does.
- **Teardown is ordered.** Every object disarms or cancels before draining
  callbacks, then releases its callback context last, so a callback can never
  outlive the state it captured.

## Relationship to `windows-overlapped-io-sys`

Thread-pool I/O is one of three completion backends for the overlapped model
defined by [`windows-overlapped-io-sys`](../windows-overlapped-io-sys). This
crate implements the `TP_IO` backend over that crate's endpoint ownership and
pinned operation storage, adding the balanced `StartThreadpoolIo` accounting
that only the thread pool requires. The pool's internal completion port is never
exposed.

## Timer stress suite

[`tests/timer_stress.rs`](tests/timer_stress.rs) applies sustained load to the
timer types: self-re-arming chains, arming churn from many threads, teardown
racing live callbacks, deliberately overlapping periodic ticks, cleanup groups
holding armed members, and a mixed scenario running all of it at once.

It is **opt-in** and deliberately excluded from CI, where it would be slow and
where a contended shared runner makes timing-sensitive scenarios unreliable.
Nothing runs unless `WINDOWS_THREADPOOL_STRESS` is set:

```powershell
$env:WINDOWS_THREADPOOL_STRESS = "1"
cargo test -p windows-threadpool-sys --test timer_stress -- --nocapture
```

`WINDOWS_THREADPOOL_STRESS_SCALE` multiplies every load count, so the same
scenarios run harder without editing them:

```powershell
$env:WINDOWS_THREADPOOL_STRESS_SCALE = "10"
```

The suite still compiles and lints in CI, so it cannot rot; only the load is
skipped. A full run at scale 1 takes about a minute.

Two things worth knowing before reading the output. Pool timers fire on the
system timer tick (~15.6ms measured), so a zero-delay re-arm chain advances at
roughly 64 links a second however trivial the callback -- scenarios are sized
for wall-clock time rather than round iteration counts. And a loop that arms and
disarms without pausing outruns the pool entirely, never reaching a tick with the
timer armed; scenarios that need firings pause past a tick and assert a floor, so
they cannot silently degenerate into testing the arming calls alone.

Assertions are limited to what is invariant under load: non-overlap where the
type guarantees it, quiescence after a drain, and the absence of a hang or a
crash. Rates, latencies, and exact firing counts are reported rather than
asserted, because under load those describe the machine rather than the code.

## Status

Work, timers, waits, private pools, cleanup groups, and thread-pool I/O are
implemented and tested. `CallbackEnviron::set_cleanup_group` remains `unsafe` as
a raw seam for foreign cleanup groups; use `CleanupGroup` for a safe one.

This crate is Windows-only. Every item is behind `cfg(windows)` semantics, and
CI builds, tests, and lints exclusively on Windows.
