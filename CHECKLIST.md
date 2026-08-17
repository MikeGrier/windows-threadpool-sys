# Checklist: workspace

Workspace-level and cross-crate work only. Per-crate work is tracked in
[crates/windows-overlapped-io-sys/CHECKLIST.md](crates/windows-overlapped-io-sys/CHECKLIST.md) and
[crates/windows-threadpool-sys/CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md).

## M1 — Workspace and release

- [x] Specialize the crate name, metadata, documentation, and release config.

- [x] Split the workspace into `windows-overlapped-io-sys` and `windows-threadpool-sys` with independent,
	component-tagged publishing.

- [ ] Reserve the `windows-overlapped-io-sys` name on crates.io.

- [ ] Confirm CI and crates.io publishing secrets are configured for both crates.

## M2 — Shared invariants

- [x] Select the initial `windows-sys` feature set and document the FFI boundary.

- [x] Choose the minimum supported Windows version for the pair (Windows Server 2025 / Windows 11, per CI).

- [ ] Specify the ownership, cancellation, and callback lifetime invariants shared by both crates. See the
	workspace [DESIGN-NOTES.md](DESIGN-NOTES.md) and
	[crates/windows-overlapped-io-sys/DESIGN-NOTES.md](crates/windows-overlapped-io-sys/DESIGN-NOTES.md).
