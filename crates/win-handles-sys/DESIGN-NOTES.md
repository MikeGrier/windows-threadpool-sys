# Design notes: win-handles-sys

## WH-D-1: the crate is the Learn section, and its first code was moved in, not written

The engineer, 2026-10-18: the crate is named `win-handles-sys`, and the function and the types that belong move
with it. It is the crate the rule of the
[root DESIGN-NOTES.md](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under)
gives `DuplicateHandle`, `GetHandleInformation`, `SetHandleInformation`, `CompareObjectHandles` and
`CloseHandle`, which [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) lists together under *System
Services > Windows System Information > Handles and Objects*.

`duplicate_handle_with_access` and `Access` were the `duplicate` module of
[win-sync-sys](../win-sync-sys/DESIGN-NOTES.md) (`WS-D-7`), which held them because its first consumer was an
event. It re-exports this crate at the paths it had, so no published path changed.

`Access` keeps both constants it had, `SYNCHRONIZE` and `EVENT_MODIFY_STATE`, though the second is an event's
right and not a handle's. An associated constant can be declared only where the type is, so leaving it in
`win-sync-sys` would have made `Access::EVENT_MODIFY_STATE` a path that stopped working; it stays, and a right
of another object type is `Access::from_bits`. That is a compatibility choice, and the type is where a later
decision to name more rights, or to move the event's, would be made.

The doc comment on `Access` was the one line changed in the move besides the provenance header: it said the
constants were "this crate's own objects", which is no longer true of this crate.
