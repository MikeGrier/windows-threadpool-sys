# win-sync-sys

Memory-safe Rust over the Win32 synchronization API: the objects Windows provides for one thread
to signal another, and for a thread to wait. Thin over Win32 and adding no policy -- the `-sys`
suffix is that promise, as the workspace's naming decision defines it.

## What is here

- **`Event`** -- an owned event object, created with `CreateEventW`, that can be set and reset
  without `unsafe`. A clone (`try_clone`) is a second handle to the same object, so one part of a
  program can keep an event to signal while another hands it to something that waits on it.

```rust
use std::os::windows::io::OwnedHandle;
use win_sync_sys::{Event, ResetMode};

let ready = Event::new(ResetMode::Auto, false)?;

// A second handle to the same event, for another thread to signal through.
let signaller = ready.try_clone()?;
std::thread::spawn(move || signaller.set())
    .join()
    .expect("the signalling thread")?;

// `ready` is now signalled. Anything that waits on a handle can take it from
// here; an `Event` converts into an `OwnedHandle` and lends one as well.
let handle: OwnedHandle = ready.into();
# drop(handle);
# Ok::<(), std::io::Error>(())
```

`windows-threadpool-sys` accepts an `Event` as a thread-pool wait target without `unsafe`, because
an `Event` is never a mutex -- the one kind of object the pool cannot wait on.

## What is not here

- **`PulseEvent`, deliberately.** Its own documentation calls it unreliable and says it should not
  be used: a thread waiting on the event can be briefly taken out of its wait by a kernel-mode APC,
  and a pulse that lands in that moment releases nothing, because a pulse releases only the threads
  waiting at the instant it happens. The waiter misses a signal it was waiting for, and nothing
  reports that it did. Microsoft's replacement is a condition variable.
- **Named events and `OpenEvent`.** Every event here is unnamed. Whether named objects belong in
  this crate is not yet decided.
- **The other kernel objects and the wait functions** -- semaphores, mutexes, waitable timers, and
  waiting on any of them -- are planned in this crate's `CHECKLIST.md`. Until then, wait on an
  event through its handle.
- **The in-process primitives** -- SRW locks, condition variables, critical sections, one-time
  initialization, barriers and `WaitOnAddress` -- are parked rather than rejected.

The reasoning behind each is in this crate's `DESIGN-NOTES.md`.
