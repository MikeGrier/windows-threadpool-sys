# Checklist: windows-threadpool-sys

Design decisions for this crate are in the workspace-root
[DESIGN-NOTES.md](../../DESIGN-NOTES.md). This crate builds on the submission seam owned by
[windows-overlapped-io-sys](../windows-overlapped-io-sys/CHECKLIST.md). Completed milestones are archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

All planned milestones are complete; there are no pending work items.

The crate now covers the callback environment, owned private pools, cleanup groups whose members the borrow
checker protects, work objects, one-shot and periodic timers as distinct types, waits that own their handle and
rearm per activation, and the `TP_IO` completion backend over the shared overlapped submission seam.

This file reopens when new work (a new object type, a new capability, or hardening) is planned.
