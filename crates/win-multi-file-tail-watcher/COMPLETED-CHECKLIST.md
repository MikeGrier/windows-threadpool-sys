# Completed checklist: win-multi-file-tail-watcher

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-15 13:59:42 -04:00 -- TW-1.1: the crate

### <a id="tw-11"></a>TW-1.1 -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. *(completed 2026-10-15 13:59:42 -04:00)*

Registered as `win-time-sys` was: a workspace member, in release-please's configuration and manifest
(at `0.0.0`, so the first release is `0.1.0`), and in the publish workflow's tag filter,
manual-dispatch choices and workspace-sibling list. `README.md` replaces `COMPONENT.md` and is the
crate documentation. Dependencies are the three the item named: [wtf-string](../wtf-string/),
[windows-overlapped-io-sys](../windows-overlapped-io-sys/) with `default-features = false` and `fs`
only (the feature that holds the observer's length query, so none of its other optional
dependencies come along), and `windows-sys` with no Win32 feature, since each arrives with the call
that needs it. Each path dependency carries its version, as the repository's dependency-version
check requires.

`lib.rs` denies `missing_docs`, `unsafe_op_in_unsafe_fn` and
`clippy::undocumented_unsafe_blocks`. Sabotage: a public function with no documentation, and an
`unsafe` block with no safety comment, each failed `cargo clippy`; the edit was reverted. The crate
has no code and so no unit tests. `tools/check-workflow-refs.ps1`,
`.github/release/check-dependency-versions.cjs` and `tools/check-publishable.ps1` all pass with it
registered; the last two need the release scripts' locked dependencies (`npm ci` in
`.github/release`).
