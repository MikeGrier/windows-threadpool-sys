# Design notes: win-handles-sys

## WH-D-1: the crate is the Learn section, and its first code was moved in, not written

The engineer, 2026-10-18: the crate is named `win-handles-sys`, and the function and the types that belong move
with it. It is the crate for `DuplicateHandle`, `GetHandleInformation`, `SetHandleInformation`,
`CompareObjectHandles` and `CloseHandle`, placed by the Microsoft Learn section that documents them: the Win32
reference lists them together under *System Services > Windows System Information > Handles and Objects*.

`duplicate_handle_with_access` and `Access` were the `duplicate` module of
[win-sync-sys](../win-sync-sys/DESIGN-NOTES.md) (`WS-D-7`), which held them because its first consumer was an
event. From the release of `win-sync-sys` that depends on this crate, it re-exports this crate at the paths it
had, so no published path changes; a release of it from before then carries its own copy, whose types are
distinct from these.

`Access` keeps both constants it had, `SYNCHRONIZE` and `EVENT_MODIFY_STATE`, though the second is an event's
right and not a handle's. An associated constant can be declared only where the type is, so leaving it in
`win-sync-sys` would have made `Access::EVENT_MODIFY_STATE` a path that stopped working; it stays, and a right
of another object type is `Access::from_bits`. That is a compatibility choice, and the type is where a later
decision to name more rights, or to move the event's, would be made.

The doc comment on `Access` was the one line changed in the move besides the provenance header: it said the
constants were "this crate's own objects", which is no longer true of this crate.

## WH-D-2: `get_handle_information` returns the flags as Windows gives them

`GetHandleInformation` is `get_handle_information(handle)`, returning `HandleFlags` (`DI-14.8.4`): a newtype over
the Win32 bits with `INHERIT` and `PROTECT_FROM_CLOSE` named, `contains`, `|`, and `from_bits` and `bits` for the
rest. A bit Windows defines that is not named is kept and not an error, since this crate does not narrow what
the call reports. The handle is `impl AsHandle`, so it is open for the borrow and the call cannot be refused for
a handle that is not; the error path is the one edge with no test, and the definition says so (its sabotage entry
is a declared blind spot).

Measured on Windows 11 build 26300, and none of it a ruling: a handle `std` opens carries no flag; an event made
with an inheritable attribute carries `INHERIT` and only it; a duplicate made by `duplicate_handle_with_access` is
not inheritable even when its source is; and protecting a handle from close reads back as `PROTECT_FROM_CLOSE`
and clears when the protection is removed. The tests set the flags through raw `SetHandleInformation`, so the
query is never checked against itself, and the protection is cleared by a guard before the handle is dropped,
since closing a protected handle is refused.
