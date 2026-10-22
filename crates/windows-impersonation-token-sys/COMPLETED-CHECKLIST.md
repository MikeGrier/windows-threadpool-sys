# Completed checklist: windows-impersonation-token-sys

## Moved 2026-09-01 -- Mutation-test formatting coverage

### <a id="mt-1"></a>MT-1 -- Add exhaustive unit tests for the actionable formatting mutants. *(completed 2026-09-01 12:59:22 UTC-04:00)*

Added coverage for both `ApplyFailure` display prefixes and for the exact
non-exhaustive, handle-redacting `ImpersonationToken` debug representation.

The remaining `THREAD_TOKEN_CAPTURE_ACCESS` mutation from bitwise OR to bitwise
XOR is behaviorally equivalent: `TOKEN_DUPLICATE` and `TOKEN_QUERY` are disjoint
single-bit flags, so both operators produce the same access mask. No runtime
unit test can distinguish identical values.

## Moved 2026-10-18 22:38:57 -04:00 -- The captured token is a `SharedHandle`

### <a id="it-inf1"></a>IT-inf.1 -- `ImpersonationToken` holds its captured token as a `SharedHandle` instead of an `Arc<OwnedHandle>`. *(completed 2026-10-18 22:38:57 -04:00)*

Parked 2026-10-07; the engineer asked for it on 2026-10-18 ("yes also fix windows-impersonation-token-sys's
alignment also"), with the request crate's [D-32](../windows-namespace-request-sys/DESIGN-NOTES.md#d-32), which
moved that crate onto `SharedHandle`. The change is to the private field: the hand-written `Clone` is a derive,
the handle stays unexposed ([D-6](DESIGN-NOTES.md#d-6)), and equality is still refused
([D-8](DESIGN-NOTES.md#d-8)), whose text now names `SharedHandle::same_handle` where it named `Arc::ptr_eq`. A unit
test asserts that a clone shares the captured token and that a second capture does not. The capture's
`DuplicateTokenEx` is unchanged: it makes a new token object with narrowed access, which is the reason it exists
and not a duplicate kept out of habit.
