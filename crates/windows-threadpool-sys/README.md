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

Two supporting types shape where and how those callbacks run: `pool::ThreadpoolPool`
is an owned private pool, and `callback_env::CallbackEnviron` is the environment
that selects a pool and a callback priority when an object is created. The
latter provides Rust equivalents of the SDK's callback-environment helpers,
which are header-only inline functions that `windows-sys` cannot emit.

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

## Relationship to `windows-overlapped-io-sys`

Thread-pool I/O is one of three completion backends for the overlapped model
defined by [`windows-overlapped-io-sys`](../windows-overlapped-io-sys). This
crate implements the `TP_IO` backend over that crate's endpoint ownership and
pinned operation storage, adding the balanced `StartThreadpoolIo` accounting
that only the thread pool requires. The pool's internal completion port is never
exposed.

## Status

Work, timers, waits, private pools, and thread-pool I/O are implemented and
tested. Cleanup groups are not yet modelled safely, so
`CallbackEnviron::set_cleanup_group` is `unsafe`; its documentation explains what
is missing.

This crate is Windows-only. Every item is behind `cfg(windows)` semantics, and
CI builds, tests, and lints exclusively on Windows.
