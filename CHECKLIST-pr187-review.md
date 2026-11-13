# PR 187 review and CI

- [ ] **PR187-1** -- Correct the dependency-minimum requirement in the thread-pool
  [CHECKLIST.md](crates/windows-threadpool-sys/CHECKLIST.md), pin the compiler diagnostics for
  the sealed-wait compile-fail examples in both crates, verify the CI guard, and reply to
  [review 5896658744](https://github.com/MikeGrier/windows-threadpool-sys/pull/187#pullrequestreview-5896658744).
  Verify: [check-compile-fail-codes.ps1](tools/check-compile-fail-codes.ps1), both crates'
  doctests with `RUSTC_BOOTSTRAP=1` (including thread-pool no-default-feature doctests),
  formatting, and all-target Clippy. Preserve the existing runtime behavior and tests.