# Checklist: windows-file-enumeration-sys

The crate's implementation milestones are M5 through M7 in the workspace
[CHECKLIST.md](../../CHECKLIST.md). This file holds work owned by the crate
itself. Status is tracked in [PLANS.md](PLANS.md). Nothing is pending; `REVIEW-1` is parked in
[PARKED.md](PARKED.md).

## PATH -- One path contract across the namespace crates

- [x] **PATH-1** -- This crate takes the request crate's path contract, `AnchoredPath`, and its older copy of the preparation is deleted. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#path-1)

  **Next:** `REVIEW-1`, in [PARKED.md](PARKED.md), is re-read against the new contract. After that nothing further in this
  crate; the order across the crates is at the top of
  [durable-ioring's CHECKLIST.md](../durable-ioring/COMPLETED-CHECKLIST.md#open-surface-order).

## ERR -- One error code type across the crates

- [x] **ERR-1** -- `Win32Error` is `win_error_codes::Win32Error`, re-exported under the same name. -> [completed 2026-10-15](COMPLETED-CHECKLIST.md#err-1)
