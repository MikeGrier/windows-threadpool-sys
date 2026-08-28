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

## Status

Early. The design decisions are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md);
the implementation is in progress against milestones M22 and M23 of the workspace
[CHECKLIST.md](../../CHECKLIST.md). Not yet ready for a crates.io release.
