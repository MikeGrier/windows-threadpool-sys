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

**The `@stable` action alone is not enough in CI.** `rustup` resolves a
directory's toolchain override ([rust-toolchain.toml](rust-toolchain.toml))
*before* consulting `rustup default`, so once this file exists, every plain
`cargo`/`rustc` invocation from inside the checked-out repo -- including the
ones `dtolnay/rust-toolchain@stable` sets up to run next -- would otherwise
silently use the pinned MSRV instead of the floating stable release it just
installed. The `build-test`/`fmt`/`clippy`/`docs`/`portable` jobs in
[.github/workflows/ci.yml](.github/workflows/ci.yml) counter this with a
job-level `env: RUSTUP_TOOLCHAIN: stable`, which sits above the directory
override in `rustup`'s resolution order and forces those jobs onto the actual
floating toolchain. The `msrv` job intentionally has no such override, since it
is meant to run the pinned version.

## Release process

`release-please` owns version changes, tags, and changelog updates. Each crate
is versioned and released independently; a `<crate>-v<version>` tag (for example
`windows-overlapped-io-sys-v0.1.0`) triggers the crates.io publish workflow after
verifying that the tag matches that crate's package version.

Some crates depend on workspace siblings by version as well as by path, so
`cargo publish`'s build verification resolves those dependencies from crates.io
rather than the workspace checkout: `windows-threadpool-sys` depends on
`windows-overlapped-io-sys`, and `windows-file-watcher` depends on **both** of
those. Because `release-please` opens a separate pull request per crate, sibling
tags can be pushed in any order, so
[.github/workflows/publish-crate.yml](.github/workflows/publish-crate.yml)
blocks each publish until *every* workspace-sibling dependency the crate
declares is live on crates.io at the required version. The effective publish
order is therefore always dependency-first regardless of tag timing, and the
check is a no-op for a crate with no workspace-sibling dependencies
(`windows-overlapped-io-sys`, `wtf-string`).

Publishing requires these repository secrets:

- `RELEASE_PLEASE_TOKEN` for release pull requests, tags, and follow-up workflow
  runs.
- `CARGO_REGISTRY_TOKEN` with crates.io publish permission.
