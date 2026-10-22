# Checklist: win-handles-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by
`durable-ioring`'s `DI-14.8.3`, moving `win-sync-sys`'s `duplicate_handle_with_access` and
`Access` here. The area is the five functions Microsoft Learn lists under *Handle and Object Functions*
in the Win32 reference. The items below say which are built.

- [x] **WH-1.1** -- `GetHandleInformation` as a safe query. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wh-11)

- [x] **WH-1.2** -- `SetHandleInformation`, the write half of the query, taking the same typed flags and a mask of which are changed. -> [completed 2026-10-21](COMPLETED-CHECKLIST.md#wh-12)

- [x] **WH-1.3** -- `CompareObjectHandles`: whether two handles name the same kernel object, as `compare_object_handles`, and this crate's tests no longer call it raw. -> [completed 2026-10-21](COMPLETED-CHECKLIST.md#wh-13)

- [ ] **WH-1.4** -- `CloseHandle`, taking an `OwnedHandle` and reporting the error the call can return, which
  dropping one cannot. It is the last of the five the section lists.
