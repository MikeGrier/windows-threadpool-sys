# Copilot Instructions

Use LF line endings.

## Repository purpose

This repository is a **template** for a Rust crate that will be published to
crates.io. Keep the automation working while replacing the placeholder crate
with project-specific code.

## Working rules

- Prefer small, reviewable changes.
- Do not leave placeholder names like `your-crate-name`, `OWNER`, or
  `REPOSITORY` behind when specializing the template for a real project.
- Keep documentation, release automation, and publish automation aligned with
  the actual crate metadata.
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
