# Checklist: win-handles-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by
[DI-14.8.3](../durable-ioring/CHECKLIST.md), moving `win-sync-sys`'s `duplicate_handle_with_access` and
`Access` here. The area is the five functions Microsoft Learn lists under *Handle and Object Functions*
([WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt)); one is built, and the rest are queued.

- [ ] **WH-1.1** -- `GetHandleInformation` as a safe query: whether a handle is inheritable and whether it is
  protected from close, as a typed set of the two flags the page documents. This is
  [DI-14.8.4](../durable-ioring/CHECKLIST.md), which carries its tests.

- [ ] **WH-1.2** -- `SetHandleInformation`, the write half of the query above, taking the same typed flags and
  a mask of which are changed. It is documented under the same Learn section, so it belongs here.

- [ ] **WH-1.3** -- `CompareObjectHandles`: whether two handles name the same kernel object. It is listed in the
  same section, and the tests of this crate already call it raw (`same_object` in `duplicate/tests.rs`), so a
  safe form lets them stop.

- [ ] **WH-1.4** -- `CloseHandle`, taking an `OwnedHandle` and reporting the error the call can return, which
  dropping one cannot. It is the last of the five the section lists.
