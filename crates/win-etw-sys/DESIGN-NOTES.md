# Design notes: win-etw-sys

## <a id="we-d-1"></a>WE-D-1: the crate is named for Event Tracing, and the provider functions are filed in it although the snapshot does not list them

**The name is the engineer's, 2026-10-20.** The assistant proposed `win-etw-sys` for the wrapper of the event-tracing
provider functions that durable-ioring's ETW sink needs, and the engineer answered: "win-etw-sys is good". The workspace's
`win-` prefix rule applies, as the crate is a safe layer more or less directly over Win32
([DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix)). The decision that led to it is
[DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76).

**Where the functions are documented, which [DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76) required this step to
find before naming a section.** Read 2026-10-20. The page of each function the crate wraps (`EventRegister`,
`EventUnregister`, `EventEnabled`, `EventProviderEnabled`, `EventWrite`) and of each structure (`EVENT_DESCRIPTOR`,
`EVENT_DATA_DESCRIPTOR`) is in the `evntprov` header's reference, with `tech.root: ETW` and `toc_rel: ../_etw/toc.json`,
and the overview page for the technology, *Event Tracing* (`api/_etw`), lists them among its functions. So the section is
*Windows Events > Event Tracing*.

**The snapshot does not list them.** [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) (2026-10-17) files under
*Desktop App Technologies > Diagnostics > Windows Events > Event Tracing > Event Tracing Reference* only the macros of
`evntprov.h` (`EventDataDescCreate`, `EventDescCreate` and the `EventDesc*` family, under *Event Tracing Macros*) and, under
*Event Tracing Functions*, one function, `CveEventWrite`. The provider functions themselves, and `EVENT_DESCRIPTOR` and
`EVENT_DATA_DESCRIPTOR`, appear nowhere in it, though the overview page lists the functions and the pages exist. By the
[placement rule](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under) the section
is read from the snapshot, which here is silent, so this note records the section from the pages and the overview, and
leaves the snapshot as it is: a regeneration is `tools/snapshot-learn-toc.ps1`'s, and what the snapshot leaves out of the
functions listing is the snapshot's to explain.

**What a `-sys` crate here is.** It is memory-safe over the call and adds no policy: it passes the arguments as given,
reports what the call reports, and refuses ahead of the call only what the call's page makes impossible to pass
([WE-D-2](#we-d-2)).

**Scheduled work.** None for this crate beyond what [DI-12.1.5](../durable-ioring/CHECKLIST.md) and
[DI-12.1.6](../durable-ioring/CHECKLIST.md) ask of it as their caller.

## <a id="we-d-2"></a>WE-D-2: a registration owns the handle, an event's data is typed fields, and the call's failures and the crate's refusals are kept apart

Read 2026-10-20 from the pages for `EventRegister`, `EventUnregister`, `EventEnabled`, `EventProviderEnabled` and
`EventWrite` (each `ms.date` 2018-12-05, `updated_at` 2024-02-22), and for `EVENT_DESCRIPTOR` and `EVENT_DATA_DESCRIPTOR`.

**What is wrapped, and what is not.** The step's four: register, unregister, whether a session is listening
(`EventEnabled` for a whole descriptor and `EventProviderEnabled` for a level and a keyword, which the page pairs as
"when a provider has access to an event's complete EVENT_DESCRIPTOR, the provider should use EventEnabled. When a provider
has access only to the event's Level and Keyword, the provider should use EventProviderEnabled"), and write an event. Not
wrapped: the enable callback `EventRegister` takes, `EventWriteTransfer` and `EventWriteEx` (activity ids and filters),
`EventSetInformation`, `EventActivityIdControl` and `EventDataDescCreate`, which the crate's own field type replaces. The
step does not call for them and no design here does; no work is scheduled for them.

**A registration owns the handle.** `EventRegister`'s page says the handle is "a process-scope operation (the handle is
valid only within the process)" and must be freed "before your provider unloads or exits". `Registration` is the handle,
unregistered when it is dropped, or by `unregister`, which reports what the call returned. A registration is made only
from a successful call, so the zero handle the page says a failed one sets is never a value of the type, and nothing here
relies on the page's remark that the calls "safely accept a zero-valued RegHandle". It is `Send` and `Sync`: the handle is
an integer and the calls take it by value.

**Registering is not installing a manifest.** The page: "It is not necessary to install a manifest to write events or
capture traces (though installing the manifest may be necessary for decoding the provider's events or for the provider to
work with Event Log)." So the crate has no manifest and no install; this is the same fact the engineer ruled on
([DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76)) and the page confirms.

**An event's data is typed fields, and the layout is theirs.** `EventWrite` takes descriptors that each "describe one block
of memory", "concatenated in order with no padding or alignment to form the event content". So `Field` is the bytes of a
value (every integer width, signed or not, the two floats, a pointer-width integer, a GUID laid out as the structure, raw
bytes, raw UTF-16 units) and two terminated strings, which are their units and then a zero as two descriptors, the second
pointing at a static. A four-byte `boolean` is a constructor and not a type, because a template's `Boolean` is a four-byte
integer. The crate does not know a template: what a decoder reads the blocks back as is the manifest's, and the crate
neither adds a length nor a type tag. A zero already inside a terminated string ends it for a decoder, which is the
decoder's rule and not the call's, so the units are written as given.

**The descriptors are built without `unsafe` for the caller, and the call is made under one.** The descriptors hold
addresses, so a descriptor list borrows the fields it points into and the call is made while it does; the `unsafe` is
the one call, with its argument each valid for as long as the borrow.

**The crate's two refusals, and what they rest on.** A list of fields of more blocks than the call's count, a `ULONG`, can
hold is `WriteError::TooManyBlocks`, and a block of more bytes than a descriptor's `Size`, a `ULONG`, can hold is
`WriteError::BlockTooLong`; in each the call is not made. The citation is the structures' types: `Size` is a `ULONG` and
`UserDataCount` is a `ULONG`. They are limits of the call's signature and not conditions the pages call invalid, so they are
the crate's own refusals, kept apart from `WriteError::Call`, the call's failure with its code. No number a page gives is
checked ahead of the call: "The maximum number is 128" for `UserDataCount`, and "up to 1,024 providers" for a process, are
passed to the call to judge.

**What the platform did with those two numbers, which is evidence and not contract.** On the machine these were written
on, a process was allowed 2,022 registrations and then refused with 14, `ERROR_OUTOFMEMORY`, where the page says 1,024;
`tests/limit.rs` asserts no number, only that the refusal is reached as the call's code and that the process recovers when
its registrations are dropped. This is a discrepancy beside the page's sentence, and the crate's behavior does not depend
on either figure. A write of 129 blocks is the call's to judge and is in the session tests, which recorded that the call refused it with 87.

**Failures are the call's code, and most callers ignore them.** The pages say the codes are "primarily intended for use in
debugging and diagnostic scenarios" and that production code "should usually ignore" them. The crate returns them and
decides nothing about what a caller does with one.

**The tests.** The pure parts are in memory: a GUID's layout and text, each field's bytes as read through the very
descriptors that would be passed to the call, the order and count of blocks, and the two refusals. What the pages say of a
registration no session listens to is observed: nothing is enabled and a write succeeds, as `EventWrite`'s page says ("If
no trace sessions are recording this event, this function will do nothing and return **ERROR_SUCCESS**"). What reaches a
session is [WE-D-3](#we-d-3).

**Scheduled work.** None.

## <a id="we-d-3"></a>WE-D-3: a `test-util` feature starts a trace session, reads it back, and the tests that use it need privilege

**Why a session, and why here.** The way to see that an event reached ETW is to start a session, enable the provider in it,
write, stop the session and read what it recorded, all by documented functions. The session is a test facility and not a
wrapper of the controller's functions, so it is behind the off-by-default `test-util` feature, as `win-debugging-sys`'
debugger is ([WD-D-3](../win-debugging-sys/DESIGN-NOTES.md#wd-d-3)), and it is one copy here that the crate that writes the
provider's events can use too ([DI-12.1.5](../durable-ioring/CHECKLIST.md)): the crate that wraps the calls owns the unsafe
code around them.

**What it is.** `Session::start` starts a session that logs to a file in the temporary directory, under a name no other
session of the process has (`StartTraceW`); `enable` enables a provider in it for a level and a keyword
(`EnableTraceEx2`); `stop` stops it (`ControlTraceW`), reads the file back with the event-record callback (`OpenTraceW`,
`ProcessTrace`, `CloseTrace`) and returns the events of the providers enabled, with each one's provider, descriptor, data and
process id. A session that is dropped without being stopped is stopped, and its file removed, because a session is
system-wide and outlives the process that started it.

**It needs privilege, so its tests are ignored by default.** Starting a session needs an elevated process or a user in the
Performance Log Users group. The development machine this was written on is neither, and `StartTraceW` was refused there
with `ERROR_ACCESS_DENIED`, so `tests/session.rs` is `#[ignore]` as this workspace's other environment-dependent tests are,
with a step in CI that runs it with `--include-ignored` on the runner, which is an administrator; a failure there is a
result. The error `Session::start` returns says what was missing.

**What it observed, run elevated on 2026-10-20, which is evidence and not contract.** All thirteen tests pass, twelve runs
in a row. A session recorded each event written to a provider enabled in it with the descriptor as given, the data as the
concatenated blocks, the writing process's id, and in the order written, from four threads sharing one registration as from
one; and it recorded nothing for a provider it had not enabled. A session enabled at a level did not enable events above it,
and one with a keyword enabled only events that share a bit with it; both are as the page for the structure says. Two
sessions each recorded the same event. A write of 128 blocks was recorded as one event of those bytes, and 129 was refused
by the call with 87, `ERROR_INVALID_PARAMETER`, which is the page's maximum of 128 for `UserDataCount`. An event of 70,000
bytes was refused with 234, `ERROR_MORE_DATA`, one of the two codes the page lists for an event too large, in a session of
64 KB buffers, and nothing was recorded for it.

**Two things the first run corrected.** An event with no fields, passed as a count of zero and the pointer an empty vector
holds, was refused with 87: the page says "UserData may be NULL if UserDataCount is zero", and the crate now passes null
for no fields, with a test of the array it builds. And a provider that was registered before a session enabled it was not
enabled at the moment `enable` returned: it followed within the bound a test waits for it, and followed a stopped session
the same way, so the tests wait for the state and do not read it at once. A provider registered after the session enabled it
was enabled from the first call.

**Privilege.** The development machine is not elevated, so a session cannot be started there; the tests were run from a
process elevated by a prompt the engineer approved, and CI runs them on the runner, which is an administrator.

**Scheduled work.** None.
