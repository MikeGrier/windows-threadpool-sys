# Checklist

## Workspace and release

- [x] Specialize the crate name, metadata, documentation, and release config.
- [x] Split the workspace into `windows-overlapped-io-sys` and `windows-threadpool-sys` with independent,
	component-tagged publishing.
- [ ] Reserve the `windows-overlapped-io-sys` name on crates.io.
- [ ] Confirm CI and crates.io publishing secrets are configured for both crates.

## Shared

- [x] Select the initial `windows-sys` feature set and document the FFI boundary.
- [x] Choose the minimum supported Windows version for the pair (Windows Server 2025 / Windows 11, per CI).
- [ ] Specify ownership, cancellation, and callback lifetime invariants.

## windows-overlapped-io-sys

- [x] Specify the rounded-out overlapped-I/O requirements and the crate boundary.
- [ ] Design the endpoint ownership, provenance, and sealed-association types.
- [ ] Design pinned operation storage, completion identity, and the result model.
- [ ] Implement and test the raw IOCP backend across the behavioral matrix.
- [ ] Implement and test the event / `GetOverlappedResult` backend.
- [ ] Define the backend seam consumed by the thread-pool `TP_IO` implementation.
- [ ] Add the gated `windows-sys` feature layout for file, socket, and device operations.

## windows-threadpool-sys

- [ ] Implement and test SDK-equivalent callback environment helpers.
- [ ] Implement the `TP_IO` backend and `StartThreadpoolIo` accounting over the shared seam.
- [ ] Implement safe work, timer, wait, and I/O abstractions.
- [ ] Test callback completion, cancellation, and destruction on Windows.
- [ ] Add API examples and generated documentation.
