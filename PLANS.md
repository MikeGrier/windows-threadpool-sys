# Plans

## windows-overlapped-io-sys (prerequisite)

1. Finalize the ownership, association, cancellation, and rundown invariants
	shared by every overlapped backend. The toolchain and platform baseline is
	fixed: Rust 1.97 / edition 2024, minimum Windows Server 2025 / Windows 11 as
	validated by GitHub CI.
2. Implement typed endpoint owners, provenance-controlled association, and
	pinned per-request operation storage with completion identity over
	`Win32_System_IO`.
3. Build the raw IOCP backend and exercise it against the full behavioral
	matrix: immediate failure, immediate success including skip-on-success,
	pending completion, targeted and whole-endpoint cancellation races,
	completion identity under load, shutdown with operations outstanding, and
	results retained after shutdown.
4. Add the event / `GetOverlappedResult` backend and the gated feature layout
	for file, socket, and device operation families.
5. Define and freeze the backend seam that `windows-threadpool-sys` implements.

## windows-threadpool-sys

6. Implement SDK-equivalent callback environment initialization and mutation
	over `windows-sys::Win32::System::Threading::TP_CALLBACK_ENVIRON_V3`.
7. Implement and test one end-to-end work submission abstraction, then use it to
	validate the callback ownership model before extending to timers and waits.
8. Implement the `TP_IO` backend over the shared seam, proving balanced
	`StartThreadpoolIo` accounting across the same behavioral matrix, before
	exposing any public thread pool I/O API.
