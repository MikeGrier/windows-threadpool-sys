# Design notes: win-authorization-sys

## WA-D-1: the crate is the Learn section, and its first code was moved in, not written

The engineer, 2026-10-18: the crate is named `win-authorization-sys`, and the function and the types that
belong move with it. It is the crate the rule of the
[root DESIGN-NOTES.md](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under)
gives the Authorization functions: `GetSecurityInfo`, `ConvertStringSecurityDescriptorToSecurityDescriptor`
and the descriptor calls are all listed in [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) under
*Security and Identity > Authorization*.

`SecurityDescriptor`, `SecurityAttributes`, `SecurityInformation`, `AclState` and the capture errors were the
`security` module of [windows-namespace-request-sys](../windows-namespace-request-sys/DESIGN-NOTES.md), which
kept the code only because a request takes a descriptor. It re-exports this crate at `security` and at the
root, so no path it published changed, and the files were moved with `git mv` so their history follows.

Two things changed in the move and nothing else. `SecurityCaptureError::into_source` was `pub(crate)` and is
`pub`, since the request crate is now another crate and reports a failed copy under its own failure. And the
tests that open a file through a request, which are the request's behaviour and not the descriptor's, stayed
there (`security_open_tests.rs`).
