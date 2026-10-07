# Checklist: windows-namespace-request-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md), and how they were
reached in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). This crate's *creation* is
tracked separately, in the workspace
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestones
M24-M26; that file is feature-scoped and is deleted when its feature completes,
so durable follow-up work for the crate belongs here instead.

Completed work is in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## NR-M-inf -- The horizon (ungated)

- [ ] **NR-inf.1** -- **A request shares the caller's handle as a `SharedHandle`
  instead of duplicating it.** The engineer's direction, 2026-10-07, and a working
  position rather than a decision: this crate should use
  [win-shared-os-owned-handle](../win-shared-os-owned-handle/README.md)'s
  `SharedHandle` (its `SH-1+.1`), and the `DuplicateHandle` capture of
  [D-4](DESIGN-NOTES.md#d-4) and [D-7](DESIGN-NOTES.md#d-7) was non-trivial work
  that is probably not worth it over an `Arc<OwnedHandle>`. **Deferred by the
  engineer because it is not a drop-in replacement**; nothing else gates it. What
  it changes, each to be settled when this is picked up:
  - **Construction.** Capture takes a `BorrowedHandle` from a caller who keeps
    ownership. A shared form needs a `SharedHandle` the caller already holds, and
    a caller with only a borrow cannot make one without giving up its handle.
    Whether duplication survives as a second capture path is open.
  - **D-7's refusals.** Re-check which of the null, `INVALID_HANDLE_VALUE` and
    pseudo-handle refusals still apply to a value that arrives already owned.
  - **Inheritance.** A duplicate is made non-inheritable (D-7); a shared handle
    keeps whatever the opener chose.
  - **Getting the handle back.** `into_owned_handle` and `From<CapturedHandle>
    for OwnedHandle` are infallible today; `SharedHandle::try_into_owned` fails
    while another clone exists. `try_clone` would become infallible.
  - **Unchanged:** D-4's measured state-sharing. Shared or duplicated, the
    request names the caller's kernel object.
  - A breaking change to the published crate.
