# Development

This repository is a generic template for publishing a Rust crate to crates.io.

## Commands

Run the standard checks from the workspace root:

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Release setup

Before publishing from a repository created from this template:

1. Replace the placeholder crate name and metadata in the Cargo manifests and
   workflow files.
2. Create the `RELEASE_PLEASE_TOKEN` repository secret so release tags trigger
   downstream workflows.
3. Create the `CARGO_REGISTRY_TOKEN` repository secret with crates.io publish
   permission.
