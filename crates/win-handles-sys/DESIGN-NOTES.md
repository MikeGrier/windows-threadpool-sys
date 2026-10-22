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

## WH-D-3: `set_handle_information` takes the page's mask and flags, and refuses nothing

**Read 2026-10-21** from the page for `SetHandleInformation` (`ms.date` 2018-12-05, `updated_at` 2024-02-22), beside
`GetHandleInformation`'s (`ms.date` 2018-12-05). The call takes a handle, `dwMask` ("A mask that specifies the bit flags to be
changed") and `dwFlags` ("This parameter can be 0 or one or more of the following values"), and its one remark is "To set or
clear the associated bit flag in dwFlags, you must set a change mask bit flag in dwMask".

**The function is the call.** `set_handle_information(handle, mask, flags)` takes the same `HandleFlags` the query returns, in
the call's order, and returns `io::Result<()>`; a flag in the mask is set if `flags` has it and cleared if it does not. The handle
is `impl AsHandle`, as the query's is. The crate adds no convenience over it: a caller who wants to set one flag passes it as the
mask and as the flags, which the example shows.

**Nothing is refused ahead of the call.** The page calls no value of either argument invalid, so none is refused. A flag in
`flags` and not in `mask` is the page's "you must set a change mask bit": the page says what changes, and a flag that is passed
and not applied is not an invalid argument, so it is passed through and documented. A bit the page does not name is the call's
to judge, as it is for the query's result ([WH-D-2](#wh-d-2)), and is not narrowed.

**Measured on Windows 11 build 26300, 2026-10-21, none of it a ruling.** Through raw calls on an event handle, over every start
state, mask and flags of the two named flags (sixty-four), the result was `(start & !mask) | (flags & mask)` each time. A bit the
page does not name, in the mask, in the flags, or in both and with every bit set, was accepted and changed no named flag. The
flags belong to the handle: setting one on a duplicate left the source unchanged, and the other way round. The current
process's pseudo-handle, which `std` lets a `BorrowedHandle` hold, was refused by the set with `ERROR_ACCESS_DENIED` (5), where
the query on it succeeded and read no flag; so the set's error path has a test, which the query's has not. `CloseHandle` on a
handle protected from close, with no debugger attached, returned `FALSE` with `ERROR_INVALID_HANDLE` (6) and left the handle
open with the flag still set, which is what the page says of the flag ("calling the CloseHandle function will not close the
object handle"); so an `OwnedHandle` dropped while protected stays open, which the function's documentation says. No test closes a
protected handle, because a debugger makes that raise an exception, which was not measured.

**The tests** read what the setter did through raw `GetHandleInformation`, as the query's set it through the raw setter, so
neither is checked against the other, and a protection is cleared by a guard before its handle is dropped. The sweep
([sabotage.json](sabotage.json)) catches a mask not passed, flags not passed, the two passed in each other's places, and a refused
call read as success. Its control passes `flags & mask`, which the platform applies anyway. Its declared blind spot is narrowing a
bit the platform does not name before the call: the platform gives such a bit no effect, so the narrowing cannot be seen, and
the claim that the crate does not narrow is the documentation's.

**Scheduled work.** None; `WH-1.3` and `WH-1.4` are the checklist's.

## WH-D-4: `compare_object_handles` returns the call's answer as a `bool`

**Read 2026-10-21** from the page for `CompareObjectHandles` (`ms.date` 2018-12-05, `updated_at` 2024-08-01). It returns "A
Boolean value that indicates if the two handles refer to the same underlying kernel object. TRUE if the same, otherwise FALSE."
and lists no failure and no use of `GetLastError`. Its remarks say it works "without imposing a requirement that specific access
rights be granted to either handle", give "whether a process handle is a handle to the current process" as the use, and its example
says of two objects of different types that the comparison "is legal to make, though the function will always return FALSE and
indicate a last error status of ERROR_NOT_SAME_OBJECT". The page gives Windows 10 and Windows Server 2016 as the first that have
the call, and the binding reaches it through the `api-ms-win-core-handle-l1-1-0` API set, not `kernel32`.

**The function is the call, and it returns a `bool`.** `compare_object_handles(first, second)` takes two `impl AsHandle` and is
`#[must_use]`. The answer is the page's: the page documents a `TRUE` and a `FALSE` and no failure, and both handles are open for
their borrows, so the one way a comparison could fail, a handle that is not open, cannot be made through the types.

**Why not `io::Result<bool>`.** The last error does separate the two kinds of `FALSE`, as measured below, and a function that
read it would return an error for a handle that is not open. That would put this crate's classification of a code on top of a page
that ties `ERROR_NOT_SAME_OBJECT` to different types only, to guard a case the borrows have excluded, and a caller would meet a
`Result` that never fails. The page's own contract is a `bool`. Changing it later to a `Result` would be a breaking change, which
is the price of this choice.

**Measured on Windows 11 build 26300, 2026-10-21, none of it a ruling.** A handle against itself, a duplicate against its source
in either order, a duplicate that grants no access, a named event made twice, and the current process's pseudo-handle against a real
handle to it in either order were the same object. Two anonymous events, an event and a mutex, an event and the process
pseudo-handle, and the process's pseudo-handle against the thread's were not. A file and its duplicate were the same object, and
one file opened twice was two, since each open makes a file object of its own. The `FALSE` for two valid handles of different
objects, of one type or two, set `ERROR_NOT_SAME_OBJECT` (1656). The `FALSE` for a null handle, a value that was never a handle,
and the value of a handle just closed set `ERROR_INVALID_HANDLE` (6), and the pseudo-handle `INVALID_HANDLE_VALUE` against itself
was the same object. These are the unit tests' cases, except the invalid handles, which the types do not let a test make.

**The tests** make their objects with raw calls and their duplicates with `std`'s `try_clone`, and not with this crate's
`duplicate_handle_with_access`, so the two are not checked against each other; the duplicate's own tests now ask
`compare_object_handles` what they used to ask the raw call. A test of all seven pairs in three groups of duplicates holds the
answer, its symmetry and its transitivity at once. The sweep ([sabotage.json](sabotage.json)) catches an inverted answer, one that
is always yes, one that is always no, and the first handle compared with itself; its control writes the test of the result
against a literal zero.

**Scheduled work.** None; `WH-1.4` is the checklist's.
