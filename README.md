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

The public API targets Windows. The workspace is also checked on non-Windows
hosts so that package metadata and platform-gated code remain healthy.

## Build

Requires Rust `1.97` or newer.

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Release

Merging conventional commits to `main` allows `release-please` to open or update
release PRs. Merging a release PR creates a `v<version>` tag, which triggers the
crates.io publish workflow.
