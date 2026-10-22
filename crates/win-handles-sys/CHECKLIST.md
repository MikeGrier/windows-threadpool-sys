# Checklist: win-handles-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by
`durable-ioring`'s `DI-14.8.3`, moving `win-sync-sys`'s `duplicate_handle_with_access` and
`Access` here. The area is the five functions Microsoft Learn lists under *Handle and Object Functions*
in the Win32 reference. The items below say which are built.

Nothing is pending. `WH-1.1` to `WH-1.4`, the five functions, are complete and archived in
[COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md).
