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

## WH-D-5: `close_handle` takes the handle and does not give it back, and reports what `CloseHandle` returned

**Read 2026-10-21** from the page for `CloseHandle` (`ms.date` 2018-12-05, `updated_at` 2024-02-22). The parameter is "A valid handle
to an open object"; a failure is a zero return with extended error information from `GetLastError`; "If the application is running
under a debugger, the function will throw an exception if it receives either a handle value that is not valid or a pseudo-handle
value." The page lists the kinds of object the call closes, and says not to use it for a socket, for a handle from `FindFirstFile`,
or for a registry key, of which it says "CloseHandle does not close the handle to the registry key, but does not return an error to
indicate this failure". It says nothing of what state a handle is in after a failed close.

**The function.** `close_handle(handle: OwnedHandle) -> io::Result<()>` takes the handle out of its owner with `into_raw_handle`, so
the owner's drop cannot close it again, and makes the call. It is what dropping an `OwnedHandle` does, with the result kept: a drop
cannot report a failure, which is the one thing this adds. A `File` and the other handle owners convert to an `OwnedHandle` by
`From`, so a caller writes `close_handle(file.into())`.

**The handle is taken whether or not the call succeeds, and is not given back.** The page is silent on the state after a failure, so
a failure cannot be turned into a handle the caller may keep: one that was not open, dropped, would close a value that may by then be
another handle's, which is the double close the type exists to prevent. That is an analytic ground and not a measurement, and it
forecloses a caller retrying a close of a handle protected from close without the flag cleared. The way to meet that case is to clear
the flag with `set_handle_information` before the close, which the function's documentation says. A handle that did fail to close and
stayed open stays open for the life of the process.

**Measured on Windows 11 build 26300 with no debugger attached, 2026-10-21, none of it a ruling.** Closing an open event succeeded.
Closing the last handle to a named event removed the object, and closing one of two left it, as the page says of "the last handle".
Closing a value twice, a null handle and a value that was never a handle failed with `ERROR_INVALID_HANDLE` (6), and a handle
protected from close failed with the same code and was still open, with its flag set, afterward; clearing the flag let it close.
The pseudo-handles `-1` and `-2` "succeeded" with no error, which the page does not describe and no `OwnedHandle` holds. A
debugger's exception, which the page describes for an invalid or a pseudo handle, was not measured, nor whether one is raised for a
protected handle; the test of a protected handle is for a run with none attached.

**The tests** never ask a closed handle's value anything, since another test's thread may have been given it: what a close did is
read from named events, whose object is removed with its last handle, and a protected handle is reclaimed by its raw value only
because the close failed and left it open. The error path is the protected handle, the one failure a safe caller can reach. The
sweep ([sabotage.json](sabotage.json)) catches a failed close read as success, a close that does nothing, a success read as failure,
and an error that is not the call's; its control writes the test of the result against a literal zero. A handle closed twice is not
injected: the second close would act on a value another thread may have been given.

**Scheduled work.** None; the checklist is complete.

## WH-D-6: `duplicate_handle` chooses the access and the inheritance, and is the one `DuplicateHandle` call

**Read 2026-10-21** from the page for `DuplicateHandle` (`ms.date` 2018-12-05, `updated_at` 2025-05-21). `dwDesiredAccess` "is ignored
if the *dwOptions* parameter specifies the DUPLICATE_SAME_ACCESS flag"; `bInheritHandle`, if `TRUE`, makes the duplicate one "that can
be inherited by new processes created by the target process", and if `FALSE` one that cannot; and a process "can use DuplicateHandle
to create a noninheritable duplicate of an inheritable handle, or a handle with different access than the original handle". Both
parameters are on the page for a same-process duplicate, and nothing on it calls either value invalid.

**The function.** `duplicate_handle(handle, DuplicateAccess, Inheritance) -> io::Result<OwnedHandle>`. `DuplicateAccess::Same` is
`DUPLICATE_SAME_ACCESS`; `DuplicateAccess::Chosen(Access)` is a mask, as `duplicate_handle_with_access` always was. `Inheritance` is
`NotInheritable` or `Inheritable`. Each is an enum and not a `bool` or a pair of flags, so a call reads at the site without the
reader looking up which argument is which, and `Same` is a case of its own and not a mask of nothing, which would be a handle that
can do nothing. Both stay in the process: a duplicate into another process is not offered here.

**Why the crate grew it.** Three crates each wrote their own same-process `DuplicateHandle` -- `windows-file-watcher`'s queue and
`windows-waitable-queues`' doorbell, both with the source's access and not inheritable, and `win-job-launcher`'s redirected standard
handles, with the source's access and inheritable. None could use `duplicate_handle_with_access`, which took a mask and was never
inheritable. Two of the three do what `BorrowedHandle::try_clone_to_owned` does; they use this function anyway, so that the
repository's duplicate is one call with one set of tests and one place to read what its attributes are, and not a fact about a
standard library's internals that three crates happen to share.

**`duplicate_handle_with_access` is the same call.** It passes `Chosen` and `NotInheritable` to `duplicate_handle`, so the process has
one `DuplicateHandle` site and the two cannot disagree. Its signature and behaviour are unchanged, and the entries of
[sabotage.json](sabotage.json) that named its body name the general function's.

**Nothing is refused ahead of the call.** A mask Windows refuses (every bit set) is the call's error, `ERROR_ACCESS_DENIED`, as it
was. The test of that goes through both inheritance values. There is no failing `Same` case a safe caller can build, since the handle
is an open `AsHandle`.

**The tests and the sweep** read a duplicate's access by what it can do to an event (wait, set, reset), and its inheritance by
`get_handle_information`, never by anything this function reports. A same-access duplicate is made of a full, a wait-only and a
set-only source, and an inheritable source is duplicated to a plain handle and back. The sweep catches a same-access duplicate with
no access, an inheritable one that is not, and the with-access form passing the wrong access or inheritance; its control writes a
nonzero mask beside the same-access flag, which the page says is ignored. Whether an inheritable duplicate is received by a child
process is the crate that creates one's to test, which `win-job-launcher` does.

**Scheduled work.** The three crates' use of it is [M44](../../CHECKLIST.md): `M44.2`, `M44.3` and `M44.4`.
