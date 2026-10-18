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

## WA-D-2: `get_security_info`, and what was measured

`GetSecurityInfo` is `get_security_info(handle, ObjectType, SecurityInformation)`, returning an owned
`SecurityDescriptor` (`DI-14.8.2`). Windows allocates the descriptor with `LocalAlloc`; it is captured into the
owned self-relative form and the allocation freed on every path, by the same guard `from_sddl` uses. The call
returns its error as its return value and not through `GetLastError`, so the code is carried as it came,
including a code of 2^31 or more (`SecurityCaptureFailure::ReadSecurity`).

Measured on Windows 11 build 26300, local NTFS, and none of it a ruling:

- **The kind does not change what a handle reads.** An event read as `KERNEL_OBJECT`, as `FILE` and as
  `WINDOW_OBJECT` gives equal descriptors. `UNKNOWN` is refused with `ERROR_INVALID_PARAMETER`. The kinds are
  all named, with a raw constructor, and none is narrowed: which a handle accepts is Windows' to say.
- **An object with no descriptor reads as a descriptor with no parts**, not as an error and not as a NULL DACL.
  An event made with default security and no name reads that way (the result is the 20-byte header), which is
  why the test for a kernel object uses a named event given a descriptor. The same result is a selection of
  nothing (`SecurityInformation::NONE`), which succeeds.
- **A part the handle cannot read is Windows' refusal.** A file opened with the right to read attributes only is
  refused the DACL with `ERROR_ACCESS_DENIED`, and a SACL on a handle without `ACCESS_SYSTEM_SECURITY` is
  refused the same way, before the privilege is looked at. Nothing retries with less.
- **A file's stored DACL has its generic rights mapped to specific ones**, so a descriptor given with `GA` reads
  back as `FA` for a file and `0x1f0003` for an event; a protected DACL reads back protected.

A null descriptor on success is guarded and not reachable by any test, since Windows documents a descriptor on
success; it is the one error edge here with no test, and is said so where it is defined.
