# Design notes: win-pipes-sys

## <a id="wp-d-1"></a>WP-D-1: the crate is the Learn section's pipe functions, and `create_named_pipe` is the first of them

The engineer, 2026-10-18, asked which crate `CreateNamedPipeW` should be exposed from, was told, and said "yes this
is the right thing to do. go ahead."

**Where it goes, and why this name.** [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) lists
`CreateNamedPipe` once, under *Desktop App Technologies > System Services > Interprocess Communications > Pipes >
Pipe Reference > Pipe Functions*, so there is no choice among sections to make. The
[root rule](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under) puts a
wrapper in the crate for the section that documents it, and no existing crate's scope contains pipes
(`win-sync-sys`, `win-handles-sys` and `win-volume-sys` are other sections). Its granularity clause makes a crate
a second-level topic, which here would be *Interprocess Communications*, and allows a topic too large for one
crate to split. That topic also holds mailslots, file mapping, data copy and more, so the crate is cut at the
pipes, named for the subject and by the `win-` rule: `win-pipes-sys`. Mailslots are listed in a section of their
own and are not here; a mailslot crate or a shared one is a judgement for when one is needed, written in that
crate's notes.

**The arguments are passed as given.** The page was read on 2026-10-18 (`ms.date` 08/03/2022). It states several
conditions on the arguments alone: exactly one of the three direction modes, a byte type not combined with
message reading, an instance count of 1 through 255, and a name of the form `\\.\pipe\pipename` of up to 256
characters. None is checked here. The crate is thin and adds no policy, the call refuses what it refuses, and its
error reaches the caller as the call gave it, which is why the modes are bit sets and not types that make a
combination unrepresentable. The tests assert the two refusals the page names with a code (`ERROR_ACCESS_DENIED`
for a second instance of a pipe made with the first-instance flag, `ERROR_INVALID_PARAMETER` for more than 255
instances) and assert the rest only to be the call's own.

**The one argument not passed as given** is a name with an interior NUL, which a C string cannot hold: the call
would read the name up to the NUL and make a pipe the caller did not name. It is refused as `InvalidInput`
before the call, as the request crate refuses it for a path
([NR-3.5](../windows-namespace-request-sys/COMPLETED-CHECKLIST.md)). The name is an `OsStr`, so a name that is
not valid UTF-16 reaches the call as it is and not with a replacement character.

**The security attributes are `win-authorization-sys`'s `SecurityAttributes`**, the type the descriptor and the
inheritance flag live in, taken as `Option<&SecurityAttributes>`; `None` passes no attributes, which the page says
gives the default descriptor and a handle that cannot be inherited.

**The two bits that are one.** The page lists `FILE_FLAG_FIRST_PIPE_INSTANCE` and `WRITE_OWNER` both at
`0x00080000`. `PipeOpenMode` names both, and they are equal; a test records it. What a caller who sets the bit gets
is whatever the call does with it, which this crate does not decide.

**Observed on 2026-10-18, Windows 11 build 26300, local, unelevated, and not a ruling:** a client that opens an
inbound pipe for reading and writing is refused with `ERROR_ACCESS_DENIED`, and one that opens an outbound pipe the
same way is too, as the page says the client must specify the matching access; a pipe created with the message
and non-blocking modes reports exactly those bits through `GetNamedPipeInfo` and `GetNamedPipeHandleStateW`, and
one created with the byte modes reports none; an overlapped pipe starts a read with no data and returns
`ERROR_IO_PENDING`, and cancelling it ends it with `ERROR_OPERATION_ABORTED`; dropping the server end of the only
instance makes the name not found. What `FILE_FLAG_WRITE_THROUGH`, `PIPE_REJECT_REMOTE_CLIENTS` and the three
security access modes do was not observed, only their values; the sabotage manifest says so.

## <a id="wp-d-2"></a>WP-D-2: a connect is two functions, because the call has two forms and no documented call says which a handle is

The engineer, 2026-10-18, "implement wp-1.3". Pages read the same day: `ConnectNamedPipe` and `DisconnectNamedPipe`,
both `ms.date` 12/05/2018.

**The forms.** `ConnectNamedPipe` takes an optional `OVERLAPPED`. The page says that for a handle "opened with
`FILE_FLAG_OVERLAPPED`, the `lpOverlapped` parameter must not be NULL", that with one and a NULL "the function can
incorrectly report that the connect operation is complete", and that for a handle not opened that way "the function
does not return until a client is connected or an error occurs". So there is a synchronous form, with no
`OVERLAPPED`, and an overlapped one, with one, and which applies is a property of how the handle was created.

**No documented call reports it.** `GetNamedPipeInfo` and `GetNamedPipeHandleState` give the type, the read mode,
the wait mode and the instance counts, and not whether the handle is overlapped; the call that does is undocumented
and is not available to this repository ([DOCUMENTED APIS ONLY](../../.github/copilot-instructions.md)). So the
synchronous function cannot refuse an overlapped pipe, and its documentation puts that on the caller, quoting the
page's own consequence. **Rejected: one synchronous function that always passes an `OVERLAPPED` of its own with an
event and waits**, as `win-volume-sys` does for device controls. That depends on what the call does with a non-NULL
`OVERLAPPED` on a handle that is not overlapped, which the page does not say, and this crate binds to the page and
not to what the call happens to do.

**The overlapped form is `unsafe`.** `connect_named_pipe_overlapped(pipe, overlapped)` hands the kernel a pointer
that it writes to when the operation completes, so the caller must keep it valid and unmoved until then, and the
pipe open. The crate cannot check either. It takes a `BorrowedHandle` and a `*mut OVERLAPPED`, the shape
`windows-overlapped-io-sys`'s `submit` closure gives its caller.

**What the call returns is named, and not interpreted.** Two zero returns are not errors, and are results:
`ERROR_PIPE_CONNECTED`, which the page says is "a good connection between client and server, even though the
function returns zero" (`ConnectOutcome::AlreadyConnected`), and, in the overlapped form, `ERROR_IO_PENDING`
(`ConnectStart::Pending`). A nonzero return is `ConnectOutcome::Succeeded` and is not said to mean "connected": the
page says that for a handle in nonblocking mode it means only that "the pipe is now available to be connected to a
new client". Every other zero is the call's code, among them `ERROR_PIPE_LISTENING` and `ERROR_NO_DATA`, which the
page names for nonblocking mode and for a previous client that closed.

**Observed on 2026-10-18, Windows 11 build 26300, local, unelevated, and not a ruling:** connecting twice with the
client still open reports an already connected client both times; connecting after the previous client closed, with
no disconnect, is `ERROR_NO_DATA`, as the page says; after `DisconnectNamedPipe` a client that opens before the
server connects again is refused with `ERROR_PIPE_BUSY`, which the page does not say, and one that opens while the
server is connecting is accepted; a client forced off by a disconnect fails on its next write; in nonblocking mode
with no client the call is `ERROR_PIPE_LISTENING`. A NULL `OVERLAPPED` on an overlapped pipe was not made except by
the sabotage that does it, which hangs the suite.

**Not here:** `FlushFileBuffers`, which the `DisconnectNamedPipe` page says lets a server "make sure data is not
lost by calling" it before disconnecting. It is documented under File Management Functions with `CreateFile`, so it
is not this crate's.
