# Copilot Instructions

Use LF line endings.

## Repository purpose

This repository contains `windows-threadpool-sys`, a Rust crate providing
memory-safe access to the Windows operating system's thread pool APIs. Raw API
declarations come from `windows-sys`; this crate owns the higher-level resource,
callback, cancellation, and lifetime model.

## Working rules

- Prefer small, reviewable changes.
- Keep `unsafe` code inside narrow FFI boundaries and document the invariants
  that make each boundary sound.
- Do not expose a safe API until callback execution, cancellation, and native
  object destruction have a defined ownership model.
- Use `windows-sys` bindings instead of declaring Windows APIs locally.
- Gate Windows-specific implementation with `cfg(windows)` while keeping
  non-Windows workspace checks operational.
- Keep documentation, release automation, and publish automation aligned with
  the crate metadata.
- If you add or remove workspace members, update any workflow or release config
  that assumes a single publishable crate.

## Validation

Run the standard workspace checks from the repository root:

```sh
cargo fmt --all --check
cargo build --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

## Release automation

- `release-please` manages version bumps, tags, and changelog updates.
- The publish workflow expects a `v<version>` tag that matches the crate
  version.
- Publishing requires the `RELEASE_PLEASE_TOKEN` and
  `CARGO_REGISTRY_TOKEN` repository secrets.

## Planning docs

- Use `DESIGN-NOTES.md` for durable design decisions.
- Use `CHECKLIST.md` for outstanding work.
- Use `PLANS.md` for short-lived implementation plans.
