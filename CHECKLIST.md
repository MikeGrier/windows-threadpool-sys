# Checklist: workspace

Workspace-level and cross-crate work only. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md). Completed groups are
archived in [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).

## M2 — Shared invariants

- [x] Select the initial `windows-sys` feature set and document the FFI boundary.

- [x] Choose the minimum supported Windows version for the pair (Windows Server 2025 / Windows 11, per CI).

- [ ] Specify the ownership, cancellation, and callback lifetime invariants shared by both crates. See the
	workspace [DESIGN-NOTES.md](DESIGN-NOTES.md) and
	[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md).
