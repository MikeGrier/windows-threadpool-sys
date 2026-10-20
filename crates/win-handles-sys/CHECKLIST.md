# Checklist: win-handles-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by
`durable-ioring`'s `DI-14.8.3`, moving `win-sync-sys`'s `duplicate_handle_with_access` and
`Access` here. The area is the five functions Microsoft Learn lists under *Handle and Object Functions*
in the Win32 reference; one is built, and the rest are queued.

- [x] **WH-1.1** -- `GetHandleInformation` as a safe query. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wh-11)

- [ ] **WH-1.2** -- `SetHandleInformation`, the write half of the query above, taking the same typed flags and
  a mask of which are changed. It is documented under the same Learn section, so it belongs here.

- [ ] **WH-1.3** -- `CompareObjectHandles`: whether two handles name the same kernel object. It is listed in the
  same section, and the tests of this crate already call it raw (`same_object` in `duplicate/tests.rs`), so a
  safe form lets them stop.

- [ ] **WH-1.4** -- `CloseHandle`, taking an `OwnedHandle` and reporting the error the call can return, which
  dropping one cannot. It is the last of the five the section lists.
