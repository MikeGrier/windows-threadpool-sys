# windows-thread-ambient-sys

Capture a Windows thread's ambient state and apply it on another thread.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Why

Some Windows behaviour is not a parameter of the call you make. It is ambient
state hanging off the calling thread:

- an **impersonation token** decides whose rights an open is checked against, and
  even which drive letters resolve;
- the **thread error mode** decides whether a hard device error raises a modal
  dialog;
- **WOW64 filesystem redirection** decides which of two directories a 32-bit
  process actually reaches.

None of it travels with work handed to another thread. A thread-pool worker
inherits none of it: measured, `OpenThreadToken` on a worker returns
`ERROR_NO_TOKEN` while the submitting thread genuinely held a token, and the
worker's error mode is `0` -- so an absent removable drive can put a modal dialog
on process-shared infrastructure.

## Scope

The crate carries thread-scoped ambient state that changes what a Win32 call
does. It does not carry call parameters, does not open files, and does not know
what any particular Windows operation is.

It also holds no policy. Every aspect is offered for capture *and* for explicit
declaration. A consumer running on shared threads will want to force the
dialog-suppressing error-mode bits; a consumer with a private thread is entitled
to the opposite choice, and does not have to fight this layer to make it.

## Two sets, because the aspects do not relate to the caller the same way

Aspects that can be **read** off the calling thread are *captured*, and the
caller chooses which to collect. Aspects that cannot be read -- WOW64
redirection has no getter at all, and I/O priority has no documented one -- are
*declared* instead: the caller states the value it wants installed. A declared
aspect has nothing to collect, so it is not part of any capture set, and leaving
it unspecified means the target thread's own value is untouched.

## Examples

### Carry a caller's context onto a worker

```rust
use std::thread;

use windows_thread_ambient_sys::declared::MemoryPriority;
use windows_thread_ambient_sys::{AmbientState, CaptureSet, Declared};

// Captured on the submitting thread, where a failure is still the caller's to
// see rather than arriving later from a worker. Declared aspects are stated
// rather than read; unspecified ones leave the worker's own values alone.
let state = AmbientState::capture(CaptureSet::DEFAULT)?
    .with_declared(Declared::none().with_memory_priority(MemoryPriority::Low));

let applied = thread::spawn(move || {
    // Applied outermost-first and released in exact reverse, with
    // impersonation innermost because its window is the narrowest.
    state.with_applied(|| "ran as the submitter")
})
.join()
.expect("the worker did not panic")?;

assert_eq!(*applied.value(), "ran as the submitter");

// A restoration failure does not discard the value; it is reported alongside
// it, so a caller can retire a contaminated thread without losing the result.
assert!(applied.restore().is_clean());
# Ok::<(), Box<dyn std::error::Error>>(())
```

To *override* the error mode rather than transplant it -- forcing the
dialog-suppressing bits on a shared worker -- leave `CaptureSet::ERROR_MODE` out
of the capture set and wrap the call in your own `ThreadErrorMode::apply` guard,
which then sits outermost.

### Not captured is not the same as captured and absent

```rust
use windows_thread_ambient_sys::Captured;

let omitted: Captured<u32> = Captured::NotCaptured;
let asked_and_empty: Captured<u32> = Captured::Absent;

// Both yield nothing, which is what `Option` would collapse them to...
assert_eq!(omitted.present(), None);
assert_eq!(asked_and_empty.present(), None);

// ...but only one of them is a decision, and that stays recoverable.
assert!(!omitted.was_captured());
assert!(asked_and_empty.was_captured());
```

### An invalid error-mode bit is not representable

```rust
use windows_thread_ambient_sys::ThreadErrorMode;

// 0x0004 is SEM_NOALIGNMENTFAULTEXCEPT. Measured, Windows rejects it per
// thread *and* an invalid bit fails the whole call -- so a caller combining it
// with valid bits would install none of them. The type refuses it instead.
let refused = ThreadErrorMode::from_bits(0x0001 | 0x0004)
    .expect_err("the alignment bit is not settable per thread");
assert_eq!(refused.bits(), 0x0004);
```

## What is carried

| Aspect | How it relates to the caller | Notes |
|---|---|---|
| Impersonation | captured | Consumed from [windows-impersonation-token-sys](../windows-impersonation-token-sys/README.md); its fail-fast restore is inherited unchanged |
| Thread error mode | captured **and** declarable | The only aspect in both sets, so a consumer may transplant it or impose its own |
| TxF transaction | captured | Outside the default set: deprecated, and a captured transaction can be committed or rolled back beneath the worker |
| WOW64 redirection | declared | Has no getter at all, so there is nothing to capture |
| Memory priority | declared | Readable, but remoting a caller's priority without being asked is a policy choice |
| Background mode | declared | Moves CPU, I/O and memory priority together, which the name says out loud |

## Status

The aspects and the composite are complete: capture, declaration, ordered
application, exact-reverse release, and the restore report. Milestones M22 and
M23 of [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) are
finished; the design decisions behind them are in
[DESIGN-NOTES.md](DESIGN-NOTES.md). Not yet released to crates.io.

The examples above are compiled as doctests, so a contract change breaks the
build rather than leaving the README teaching the old answer. The composite is
additionally proved on a real Windows thread-pool worker in
[tests/thread_pool.rs](tests/thread_pool.rs) -- including the negative that
motivates the crate, that an aspect which was *not* captured does not arrive on
the worker.
