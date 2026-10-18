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

## <a id="wp-d-3"></a>WP-D-3: `create_pipe` returns a struct with `read` and `write` fields, and the call's error edge is not reached by any test

The engineer, 2026-10-18, "implement wp-1.4". Page read the same day: `CreatePipe`, `ms.date` 12/05/2018.

**The result is a struct, not a tuple.** The call writes two handles through two output pointers, and a tuple
would leave which is which to position, where both are `OwnedHandle`s and a swap compiles. `AnonymousPipe` has a
`read` field and a `write` field. The fields are public and the type has no other behaviour: a caller who wants one
end closed moves it out and drops it, and `File::from` takes either.

**One attributes parameter, so both ends share it.** The page's `lpPipeAttributes` "determines whether the returned
handle can be inherited", and the call has no second one, so the two ends are inheritable together or not at all. The
handle-flag calls that would change one end afterwards are `win-handles-sys`'s, and `SetHandleInformation` is queued
there.

**`size` is passed as given.** The page calls it "only a suggestion" and says zero selects "the default buffer size".
The crate does not interpret it, and no test asserts what it did, because the page does not say. A sabotage that
passes zero in its place is a declared survivor for that reason.

**No handle is taken from a failed call.** The page: "If `CreatePipe` fails, the contents of the output parameters
are indeterminate." The wrapper returns the call's error before it reads either local.

**Not reached.** No test traverses that error edge. The largest `nSize`, `u32::MAX`, succeeded on the measuring
machine (2026-10-18, Windows 11 build 26300, unelevated), called from Python, so no argument this crate can pass
makes the call fail there. The test that passes it asserts only that either result is a result. What would reach it is a condition that exhausts the system, which a unit test does
not make. The sabotage that replaces the failure test with `false` is a declared survivor and says so.

**Observed, not a ruling:** the first version of the test that tries every size wrote on the thread that then read,
and did not return for some size in its list, which is the page's account of a full buffer; it now writes from its
own thread, and which sizes held the write was not recorded. After the write end is dropped a read reads zero bytes, which `File` reports as the end of the stream;
after the read end is dropped a write fails with `ErrorKind::BrokenPipe`; a write on the read end and a read on the
write end both fail.

## <a id="wp-d-4"></a>WP-D-4: the state calls take the caller's request and return what was asked, because the pages make some optional outputs an error

The engineer, 2026-10-18, "implement wp-1.5". Pages read the same day: `GetNamedPipeInfo` and
`SetNamedPipeHandleState`, `ms.date` 12/05/2018, and `GetNamedPipeHandleStateW`, `ms.date` 08/05/2022.

**`get_named_pipe_info` asks for everything.** All four of its outputs are optional and none is an error to ask
for, so it requests all four and returns a `PipeInfo` with the flags and the three numbers as the call gave them. The
flags are `PipeInfoFlags`, a bit set for the reason `PipeMode` is one; two of the page's four values are zero, so
`is_server_end` and `is_message_type` are the questions a value can answer, as the page words them ("if this value
is not specified, the handle refers to the client end").

**`get_named_pipe_handle_state` does not ask for everything.** The page says the collection count and the timeout
"must be NULL if the specified pipe handle is to the server end of a named pipe or if client and server processes
are on the same computer", and the user name "must be NULL if the specified pipe handle is to the client end", so a
call that asked for all of it would fail on every local pipe. The caller says what to ask for in a `PipeStateQuery`,
nothing by default, and the result has `Some` for what was asked and `None` for the rest. A caller who asks for what
the call refuses gets the call's error and not a refusal from the crate, which would be this crate's narrowing of a
condition the page states and the call enforces.

**The user name is a capacity the caller gives.** The page gives the buffer's size in "TCHARs" and no figure. The
query carries the capacity in UTF-16 units, the result carries the string up to its first NUL, and a capacity that
does not hold it is the call's error. This crate does not choose a size for the caller.

**The state is `PipeMode`.** `lpState` holds `PIPE_NOWAIT` and `PIPE_READMODE_MESSAGE`, which `PipeMode` already
names, and `lpMode` of the set takes the same two kinds of flag. The set passes the bits as given: a bit that is not a
read or wait mode is the call's refusal.

**The set is a struct of options.** `PipeStateUpdate` has three optional fields, two of them `u32`, which positional
`Option` arguments would leave to be told apart by order; nothing is changed by default.

**Observed on 2026-10-18, Windows 11 build 26300, local, unelevated, and not a ruling.**

- The read end of an anonymous pipe reports as the server end and the write end as the client end; the default buffer
  size, a `CreatePipe` size of zero, is reported as 4096 in both directions, where a named pipe made with zero reports
  zero.
- The user name of a client that asked for `SECURITY_IMPERSONATION`, and of one that asked for no quality of
  service, is returned once the client has written and the server has read. Before that, for the second the call fails
  with `ERROR_CANNOT_IMPERSONATE` (1368), and for the first it succeeded. A client that asked for `SECURITY_ANONYMOUS`
  is refused after the read.
- A capacity equal to the name's length in units is refused with `ERROR_INSUFFICIENT_BUFFER` (122); one more is
  accepted.
- A client of a message pipe begins in byte-read mode while the server end begins in the mode it was made with; the
  mode of each handle is its own.
- The mode bits `PIPE_TYPE_MESSAGE` and `PIPE_REJECT_REMOTE_CLIENTS` given to the set are refused with
  `ERROR_INVALID_PARAMETER`.
- A client opened read-only without `FILE_WRITE_ATTRIBUTES` is refused by the set with `ERROR_ACCESS_DENIED`, as the
  page says. The page's matching condition for the info call on a write-only pipe was not seen to fail without
  `FILE_READ_ATTRIBUTES`, and only the satisfied direction is tested.

**Not reached.** The two collection values are returned and set only for a client of a pipe on another computer, which
nothing here has. The two sabotages that exchange their pointers are declared survivors. What would reach them is a remote server, and a consumer with one would measure them there.
