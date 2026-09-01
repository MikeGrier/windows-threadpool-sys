# Completed checklist: windows-impersonation-token-sys

## Moved 2026-09-01 -- Mutation-test formatting coverage

### <a id="mt-1"></a>MT-1 -- Add exhaustive unit tests for the actionable formatting mutants. *(completed 2026-09-01 12:59:22 UTC-04:00)*

Added coverage for both `ApplyFailure` display prefixes and for the exact
non-exhaustive, handle-redacting `ImpersonationToken` debug representation.

The remaining `THREAD_TOKEN_CAPTURE_ACCESS` mutation from bitwise OR to bitwise
XOR is behaviorally equivalent: `TOKEN_DUPLICATE` and `TOKEN_QUERY` are disjoint
single-bit flags, so both operators produce the same access mask. No runtime
unit test can distinguish identical values.
