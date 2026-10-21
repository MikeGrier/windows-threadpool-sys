# Design notes: win-debugging-sys

## <a id="wd-d-1"></a>WD-D-1: the crate is named for the debugging section, and `OutputDebugStringW` is the first function filed in it

**The name is the engineer's, 2026-10-20.** The assistant proposed `win-debug-sys` for the crate that holds the
`OutputDebugStringW` call the debugger sink of `win-durable-ioring` needs, and the engineer answered: "how about
win-debugging-sys?" The workspace's `win-` prefix rule applies, as the crate is a safe layer more or less directly over
Win32 ([DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix)). The decision that led to it is
[DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76).

**The section.** By [the placement rule](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under),
the snapshot [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) (2026-10-17) files `OutputDebugString` once,
under *Desktop App Technologies > Diagnostics > Debugging and Error Handling > Basic Debugging > Debugging Reference >
Debugging Functions*. That is this crate's section. The same section lists the debugger's side of the exchange --
`WaitForDebugEventEx`, `ContinueDebugEvent`, `DebugActiveProcess`, `ReadProcessMemory` and the rest -- so those are this
crate's by the rule, and none is wrapped yet ([WD-D-2](#wd-d-2) says what the tests use in their place). Neither
Microsoft's section title nor its header appears in the name or in an identifier, as the rule requires.

**What a `-sys` crate here is.** It is memory-safe over the call and adds no policy: it passes the arguments as given,
reports what the call reports, and refuses ahead of the call only what the call's page makes impossible to pass.

**Scheduled work.** None for this crate. The first caller is the debugger sink in `win-durable-ioring`
([DI-12.1.3](../durable-ioring/CHECKLIST.md)). Wrapping the debugger's side is not scheduled: no step needs it, and
whether the crate should grow it is the engineer's to say.

## <a id="wd-d-2"></a>WD-D-2: `output_debug_string` takes UTF-16 units and `output_debug_str` a `str`, a NUL is the crate's one refusal, and the call is observed by a debugger

Read 2026-10-20 from the pages for `OutputDebugStringW` (`ms.date` 2024-02-02, `updated_at` 2025-08-27),
`OUTPUT_DEBUG_STRING_INFO` (`ms.date` 2018-12-05, `updated_at` 2024-02-22) and `WaitForDebugEventEx` (`ms.date`
2018-12-05, `updated_at` 2024-02-22).

**The call cannot fail.** Its one parameter is "The null-terminated string to be displayed", its return value is "None",
and the remarks say that with no debugger and no active system debugger the call "does nothing". So the functions return
`Result<(), OutputDebugStringError>` only for the crate's own refusal, and a call that is made has no failure to report
and no way to say whether anything was shown.

**The shape.** `output_debug_string(&[u16])` is the call, with the units as given: an unpaired surrogate is a unit, as it
is in a Windows string, and the call may show it however the debugger does. `output_debug_str(&str)` is the same over the
units a `str` encodes to, for the caller that has text and not units, which a diagnostic sink does. Both take a copy of
the units with a terminator added, because a slice is not NUL-terminated and the call reads to a NUL. The copy costs an
allocation, and is what makes the read stop where the slice does.

**The one refusal, and what it rests on.** A string with a NUL is refused as `OutputDebugStringError::EmbeddedNul`, naming
the first NUL's place in units, and the call is not made. The citation is the page's "null-terminated string": the call
would take the string to end at that NUL and show only what came before it, with nothing to tell the caller so. The page
does not call such a string invalid, so this is not a prediction of the platform's rejection and is not framed as one: it
is the crate's own narrowing, in an error of its own that no call's failure shares. The alternatives were to cut the
string at the NUL, or to leave the NULs out; each is the crate deciding what the caller meant, and the refusal leaves
that to the caller. The index is in units and not bytes or characters, since units are what both functions take, and a
`str`'s is therefore not its byte offset.

**The call is observed by a debugger, over the documented route.** `tests/debugger.rs` starts the test binary as a process
being debugged (`CreateProcessW` with `DEBUG_ONLY_THIS_PROCESS`), runs a test in it that sends strings, and receives each
with `WaitForDebugEventEx` and `ReadProcessMemory`. The route is documented end to end: the page for the call says a
debugger must wait with `WaitForDebugEventEx` to be sent Unicode, and the page for the structure says the string is read
out of the process with `ReadProcessMemory`. The other way to read debug output, a program that reads what every process
sends through a shared section, is not described by the pages for these functions, and is not used
([DESIGN-NOTES.md](../../DESIGN-NOTES.md#no-undocumented-apis)).

**What it observed, which is evidence and not contract.** A debugger that waited with `WaitForDebugEventEx` was sent each
string as Unicode, unit for unit, an unpaired surrogate and a character outside the basic plane included, and the empty
string arrived as an event whose string is empty. A string refused for a NUL produced no event, not even for what came
before the NUL. A string of 100,000 units arrived cut to the length the page for the structure gives: "the lower 16 bits of
the length of the string in bytes", with the terminator counted. The
tests state each of these as what happened on the machine they ran on, and the crate's documentation does not promise
any of it: the page says the string is shown by the debugger, not how it arrives.

**What the debugger side uses.** The tests call `windows-sys` directly for the debugger's functions, under their own
`unsafe`, because wrapping them is not this step's ([WD-D-1](#wd-d-1)). They are a dev-dependency feature set and no part
of the crate's own.

**Scheduled work.** None: the crate's one function is done, and its first caller is
[DI-12.1.3](../durable-ioring/CHECKLIST.md).
