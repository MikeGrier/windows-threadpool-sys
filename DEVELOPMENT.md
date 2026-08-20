# Development

Development guidance for `windows-threadpool-sys`.

The implementation targets Windows and should use `windows-sys` for raw API
declarations. Keep `unsafe` code at narrow FFI boundaries, document its safety
invariants, and represent callback and native-object ownership explicitly in
safe Rust types.

## Commands

Run the standard checks from the workspace root:

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --all-features --locked
cargo test --workspace --all-features --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
```

This is primarily a Windows-only workspace: every Windows crate's behavior is
exercised on Windows, and CI builds, tests, and lints those crates exclusively
on Windows. Keep platform-specific implementation details behind `cfg(windows)`.
`wtf-string` is the exception: its portable core has no `cfg(windows)` gating,
so CI additionally builds, tests, and lints it on Linux and macOS (the
`portable` job) to keep that non-Windows support verified.

## Toolchain

[rust-toolchain.toml](rust-toolchain.toml) pins local development to the MSRV
(1.98.0, with `clippy`/`rustfmt`), so a plain `cargo`/`rustc`/`rustup` invocation
in this repo always matches the floor every crate's `rust-version` declares --
no per-machine `rustup default` setup needed. CI is deliberately different: the
`build-test`/`fmt`/`clippy`/`docs` jobs pin their toolchain action to `@stable`
(floating forward with each new Rust release) specifically to catch new lints
and forward-compatibility drift ahead of time, while the separate `msrv` job
pins `@1.98.0` to guard the floor. A local MSRV-pinned toolchain can therefore
miss a lint that only exists in a newer stable compiler and only shows up in
CI; when that happens, fix the lint and move on -- it is not a sign the pin is
wrong.

## Release process

`release-please` owns version changes, tags, and changelog updates. Each crate
is versioned and released independently; a `<crate>-v<version>` tag (for example
`windows-overlapped-io-sys-v0.1.0`) triggers the crates.io publish workflow after
verifying that the tag matches that crate's package version.

`windows-threadpool-sys` depends on `windows-overlapped-io-sys` by version as well as
by path, so `cargo publish`'s build verification resolves that dependency from
crates.io rather than the workspace checkout. Because the two crates' tags can be
pushed in either order, the publish workflow blocks the `windows-threadpool-sys`
job until its required `windows-overlapped-io-sys` version is live on crates.io,
so the overlapped-I/O crate always finishes publishing first regardless of tag
timing.

Publishing requires these repository secrets:

- `RELEASE_PLEASE_TOKEN` for release pull requests, tags, and follow-up workflow
  runs.
- `CARGO_REGISTRY_TOKEN` with crates.io publish permission.
