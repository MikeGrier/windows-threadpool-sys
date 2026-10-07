// Copyright (c) 2026 Mike Grier
//! A ring for Windows file I/O with durability scheduled into its work.
//!
//! What it is, and for whom: [COMPONENT.md](https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/COMPONENT.md).
//! The guarantees it makes are its contract,
//! [CONTRACT.md](https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/CONTRACT.md),
//! and the API that carries them is designed in
//! [API.md](https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/API.md).
#![cfg(windows)]
// DI-D-4: no unsafe code, from the first commit. A Win32 call no lower crate wraps safely is added
// to that crate, never written here.
#![forbid(unsafe_code)]
#![warn(missing_docs)]
