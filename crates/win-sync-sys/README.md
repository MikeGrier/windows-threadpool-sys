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

- **`wait_for_single_object`** -- `WaitForSingleObject` over any handle, an `Event` included, with
  a timeout of `None` for no limit or a `Duration`. It reports how the wait ended as a
  `WaitOutcome`, so a mutex abandoned by its owner is not mistaken for a signal. A satisfied wait
  changes the object as the Win32 call does: an auto-reset event is consumed.

```rust
use std::time::Duration;
use win_sync_sys::{Event, ResetMode, WaitOutcome, wait_for_single_object};

let event = Event::new(ResetMode::Auto, false)?;
assert_eq!(
    wait_for_single_object(&event, Some(Duration::ZERO))?,
    WaitOutcome::TimedOut,
);

event.set()?;
assert_eq!(wait_for_single_object(&event, None)?, WaitOutcome::Signalled);
// The wait took the signal.
assert_eq!(
    wait_for_single_object(&event, Some(Duration::ZERO))?,
    WaitOutcome::TimedOut,
);
# Ok::<(), std::io::Error>(())
```

`windows-threadpool-sys` accepts an `Event` as a thread-pool wait target without `unsafe`, because
an `Event` is never a mutex -- the one kind of object the pool cannot wait on.

- **`WaitOnlyEvent`** and **`duplicate_handle_with_access`** -- a handle to the same object that can
  do less. `Event::try_clone_wait_only` gives a handle to the event that can be waited on and can
  neither set nor reset it: the kernel refuses a `SetEvent` or `ResetEvent` on it.
  `duplicate_handle_with_access` is `DuplicateHandle` with the access mask chosen, over any handle
  and returning an `OwnedHandle`. **Windows does not confine a duplicate to the source's access**,
  so a holder that sets out to can duplicate a wait-only handle into a wider one; the narrowing
  guards against a handle used for what it was not handed out for, and is not a boundary within
  the process.

```rust
use std::time::Duration;
use win_sync_sys::{Access, Event, ResetMode, WaitOutcome, duplicate_handle_with_access, wait_for_single_object};

let event = Event::new(ResetMode::Manual, false)?;

// Hand this to something that should wait for the event and never signal it.
let waiter = event.try_clone_wait_only()?;
assert_eq!(wait_for_single_object(&waiter, Some(Duration::ZERO))?, WaitOutcome::TimedOut);
event.set()?;
assert_eq!(wait_for_single_object(&waiter, None)?, WaitOutcome::Signalled);

// The same for a handle that is not an `Event`, such as one received from elsewhere.
let adopted = duplicate_handle_with_access(&event, Access::SYNCHRONIZE)?;
assert_eq!(wait_for_single_object(&adopted, None)?, WaitOutcome::Signalled);
# Ok::<(), std::io::Error>(())
```

## What is not here

- **`PulseEvent`, deliberately.** Its own documentation calls it unreliable and says it should not
  be used: a thread waiting on the event can be briefly taken out of its wait by a kernel-mode APC,
  and a pulse that lands in that moment releases nothing, because a pulse releases only the threads
  waiting at the instant it happens. The waiter misses a signal it was waiting for, and nothing
  reports that it did. Microsoft's replacement is a condition variable.
- **Named events and `OpenEvent`.** Every event here is unnamed. Whether named objects belong in
  this crate is not yet decided.
- **The other kernel objects and the rest of the wait functions** -- semaphores, mutexes, waitable
  timers, waiting on several objects at once, an alertable wait and `QueueUserAPC` -- are planned in
  this crate's `CHECKLIST.md`.
- **The in-process primitives** -- SRW locks, condition variables, critical sections, one-time
  initialization, barriers and `WaitOnAddress` -- are parked rather than rejected.

The reasoning behind each is in this crate's `DESIGN-NOTES.md`.
