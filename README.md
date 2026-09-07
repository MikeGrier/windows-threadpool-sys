# windows-threadpool-sys

Memory-safe Rust access to the Windows thread pool APIs.

The Windows thread pool lets applications dispatch work and wait on operating
system objects without permanently dedicating application threads to those
waits. This is particularly useful for Windows services and components that
need to become inexpensive when idle.

`windows-threadpool-sys` will build on the raw API declarations from
[`windows-sys`](https://crates.io/crates/windows-sys) and provide a Rust
programming model with explicit callback, cancellation, and resource lifetime
rules.

## Status

The crate is in its initial development stage and does not yet expose its
thread pool API. The intended scope and motivation are recorded in
[`DESIGN-NOTES.md`](DESIGN-NOTES.md), and current implementation work is tracked
in [`CHECKLIST.md`](CHECKLIST.md).

## Platform support

Primarily Windows only: the public API and implementation of this crate target
Windows, CI builds/tests/lints it exclusively on Windows, and platform-specific
code lives behind `cfg(windows)`. The workspace's `wtf-string` crate is the
exception -- its portable core has no `cfg(windows)` gating, so CI additionally
builds, tests, and lints it on Linux and macOS.

## Crate naming

The `-sys` suffix here does **not** carry its usual ecosystem meaning. A `-sys`
crate is normally raw FFI declarations with no abstraction over them; every
`windows-*-sys` crate in this workspace instead wraps a great deal of `unsafe`,
and takes its declarations from
[`windows-sys`](https://crates.io/crates/windows-sys) rather than making its own.

What the suffix marks is a **layer**:

- **`windows-*-sys`** -- makes an existing Win32 API memory-safe *without adding
  policy*. It schedules nothing, picks no delivery model, and reports the
  platform's outcomes unnormalised.
- **no suffix** -- decides something Win32 has no equivalent of.
  `windows-waitable-queues` is the worked example: it chooses a slot protocol, an
  overflow policy, and a signalling discipline, so calling it `-sys` would
  misdescribe how much it decides on the caller's behalf.

So the presence of `unsafe` wrappers is not evidence either way -- it is the job
description of the first kind. The distinction is *policy*, and the suffix is the
only signal a reader has for how much a crate decides for them.

This section is the statement of record. The convention governs published crate
names, so it belongs where a reader meets the crates rather than only in the
design notes.

## Build

Requires Rust `1.98` or newer.

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
```

## Release

Merging conventional commits to `main` allows `release-please` to open or update
release PRs. Merging a release PR creates a `v<version>` tag, which triggers the
crates.io publish workflow.
