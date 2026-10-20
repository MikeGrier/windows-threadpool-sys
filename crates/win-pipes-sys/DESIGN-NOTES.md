# Design notes: win-pipes-sys

## <a id="wp-d-1"></a>WP-D-1: the crate is the Learn section's pipe functions, and `create_named_pipe` is the first of them

The engineer, 2026-10-18, asked which crate `CreateNamedPipeW` should be exposed from, was told, and said "yes this
is the right thing to do. go ahead."

**Where it goes, and why this name.** The Microsoft Learn Win32 reference lists
`CreateNamedPipe` once, under *Desktop App Technologies > System Services > Interprocess Communications > Pipes >
Pipe Reference > Pipe Functions*, so there is no choice among sections to make. The workspace places a
wrapper in the crate for the Learn section that documents it, and no existing crate's scope contains pipes
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

**Amended by [WP-D-7](#wp-d-7): the safe synchronous connect takes a `SynchronousPipe`, and the form for any handle is `unsafe`.**

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
synchronous function cannot refuse an overlapped pipe when it is given one; [WP-D-7](#wp-d-7) carries the fact in a
type from where the handle is opened instead. **Rejected: one synchronous function that always passes an `OVERLAPPED` of its own with an
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

## <a id="wp-d-5"></a>WP-D-5: a reply longer than the buffer is a result, the time-out is a newtype, and the pages' account of three behaviours differs from what was observed

**Amended by [WP-D-7](#wp-d-7): the safe synchronous transaction takes a `SynchronousPipe`, and the form for any handle is `unsafe`.**

The engineer, 2026-10-18, "implement wp-1.6". Pages read the same day: `PeekNamedPipe` and `TransactNamedPipe`,
`ms.date` 12/05/2018; `CallNamedPipeW`, `ms.date` 04/19/2021; `WaitNamedPipeW`, `ms.date` 08/05/2022.

**`ERROR_MORE_DATA` is a result, as `ERROR_PIPE_CONNECTED` is in [WP-D-2](#wp-d-2).** Both pages define it: the reply is
longer than the buffer, the call returns zero with that code, and the page says what becomes of the rest (for
`TransactNamedPipe` it stays in the pipe, for `CallNamedPipeW` it is discarded). The call did what it was asked and
filled the buffer, so `Transaction` carries `bytes_read` and `more_data` for both. The overlapped form carries it as
`TransactStart::MoreData`, for a call that reports it at once; one that reports it when the operation completes is
the operation's result, which the caller reads as for any overlapped operation.

**The synchronous transact has the connect's hazard, and the same answer.** The page says that with a NULL `OVERLAPPED`
on a handle opened with `FILE_FLAG_OVERLAPPED` "the function can incorrectly report that the operation is complete",
and no documented call reports how a handle was opened, so it has the same answer, [WP-D-7](#wp-d-7). The overlapped
form is `unsafe` and takes raw buffer pointers and lengths, because the buffers must outlive the operation, which a
borrow cannot say. It does not ask for the byte count: the page says an overlapped call's `lpBytesRead` "can be NULL" and the
count comes from the overlapped result or the completion port.

**Buffer lengths.** The calls take `u32` lengths. An input longer than `u32::MAX` is `InvalidInput` and the call is not
made, because telling the call a shorter length would write part of the input and report success. An output buffer
longer than that is passed as `u32::MAX` long, which can only read into less, and the byte count says how much it
did. Both rules are in one place, `input_len` and `output_len`, and a unit test pins each across the boundary.

**The name's one check is in one place.** `create_named_pipe`, `call_named_pipe` and `wait_named_pipe` take their name
through `wide_name`, which holds the interior-NUL refusal; the first of them used to hold it inline, and the three
would otherwise have been three copies of one rule.

**`PipeTimeout` is a newtype over the page's `u32`.** The special values share the number space with milliseconds:
`NMPWAIT_NOWAIT` is `1`, so a `CallNamedPipeW` time-out of one millisecond is the page's "does not wait", and
`NMPWAIT_WAIT_FOREVER` is `0xffffffff`. A number type with named constants leaves the collision as the page has it and
does not move a number out of the way. `NO_WAIT` is listed on the `CallNamedPipeW` page and not on `WaitNamedPipeW`'s;
the type carries all three and the pages decide where each applies.

**A wait that times out is an error, as the page says.** `WaitNamedPipeW` "will fail with the error `ERROR_SEM_TIMEOUT`",
and `wait_named_pipe` returns it as the call's error. Whether a time-out is an outcome and not a failure is the
caller's reading of that code.

**Observed on 2026-10-18, Windows 11 build 26300, local, unelevated, and not a ruling. Three differ from the pages.**

- `PeekNamedPipe`'s `lpBytesLeftThisMessage`: the page says it "will be zero for byte-type named pipes or for anonymous
  pipes". For an anonymous pipe peeked with a buffer shorter than the data it was `0 - bytes_read` as a `u32`
  (4294967293 for a 3-byte read of 5), and zero when no buffer was given. The wrapper returns the call's value and does
  not correct it.
- `WaitNamedPipeW`: the page says an instance is available when "the pipe's server process has a pending
  `ConnectNamedPipe` operation on the pipe". An instance that had been created and had not started to connect was
  reported available at once.
- `CallNamedPipeW`: with an instance that is busy and a time-out of 150 ms the call returned `ERROR_PIPE_BUSY` (231) after
  about that long, and `NMPWAIT_NOWAIT` returned the same error after about one timer tick; the page's description of
  the equivalent `WaitNamedPipe` is `ERROR_SEM_TIMEOUT`, which `wait_named_pipe` itself returned.

Also observed, and not in conflict with a page:

- A handle in byte-read mode given to `TransactNamedPipe` fails with `ERROR_BAD_PIPE` (230).
- After `ERROR_MORE_DATA` the output buffer was filled, `lpBytesRead` was the buffer's size, and the remainder was
  readable with `ReadFile` and counted by `PeekNamedPipe`.
- A wait of one millisecond on a busy instance returned after about one timer tick, near 16 ms, and so the tests assert
  lower bounds of two thirds of the time asked and no upper bound.
- `PeekNamedPipe` fails once the write end is closed and nothing is left, and still shows what was written before.
- Whether an overlapped transaction with a short buffer reports `ERROR_MORE_DATA` at once or on completion was not
  recorded; its test accepts both and asserts the operation's result.

**Not reached.** The remote forms the pages describe (a `\\server\pipe\name`, whose redirector opens a pipe in message
mode, and a transaction larger than the 64 KiB the page guarantees) are not exercised: nothing here has a remote
server, and the sizes tested stay below 64 KiB.

## <a id="wp-d-6"></a>WP-D-6: the peer queries share one shape in one place, and the computer name's decoding is tested where a local pipe cannot reach it

The engineer, 2026-10-18, "implement wp-1.7". Pages read the same day: the four identifier calls, `ms.date`
12/05/2018, and `GetNamedPipeClientComputerNameW`, `ms.date` 08/05/2022. The pages are short: each takes "a handle to
an instance of a named pipe", which "must be created by the `CreateNamedPipe` function", and returns the identifier or
name "for the specified named pipe".

**One site for the four identifiers.** The four calls write a `ULONG` through a pointer and are otherwise the same, so
one private `identifier` function holds the call, the local, the failure test and the error, and each public function
names which call to pass. A swap of client for server is then one token, and the tests tell the two apart with a client
in another process and with an instance that has no client.

**The result is `u32`, as the page's `PULONG` is.** A process identifier and a session identifier are returned as the
call gave them; nothing is wrapped in a type that would say more than the page does.

**The computer name takes a capacity in bytes.** The page gives `ClientComputerNameLength` as "the size of the
`ClientComputerName` buffer, in bytes" and no figure for how long a name can be, so the caller says how much room to
give the call. The buffer is made of whole UTF-16 units that fit in that many bytes and the call is told the size of the
buffer it was given, which is never more than the buffer holds; a capacity of one byte gives a buffer of none. The result
is the string up to its first NUL.

**The decoding is shared and tested on its own.** `from_wide_until_nul` is the one place that reads a
NUL-terminated string out of a buffer the caller sized: the computer name uses it, and so does the user name of
[WP-D-4](#wp-d-4), which had the same lines inline. Both it and the buffer sizing are unit-tested directly, because the
call that would exercise them with a real name is not reachable here (below).

**Observed on 2026-10-18, Windows 11 build 26300, local, unelevated, and not a ruling.**

- The four identifier calls succeed on the handle of a client as well as on a server's, against the pages' "must be created
  by the `CreateNamedPipe` function". Both ends of an anonymous pipe's handles are accepted, and a handle that is not a
  pipe fails.
- For an instance that has no client the server's process and session identifiers are returned and the client's two fail
  with `ERROR_NOT_FOUND` (1168).
- For a client in another process, here `cmd` reading a line from the pipe, the client's process identifier was that
  process's, and its session identifier was the one `ProcessIdToSessionId` gave for it. The server's was this process's.
- `GetNamedPipeClientComputerNameW` failed with `ERROR_PIPE_LOCAL` (229) for every capacity from zero to 4096 bytes, from
  either end's handle, for a client on the same computer.

**Not reached.** A computer name for a client on another computer, a client in a session other than the server's, and a
server in another process. The sabotages that tell the call a wrong size and that discard the decoded name are declared
survivors for the first; what would reach it is a client on another computer.

## <a id="wp-d-7"></a>WP-D-7: the synchronous connect and transaction are safe only on a `SynchronousPipe`, a handle known from its opening not to be overlapped

The engineer, 2026-10-19, on review finding that the safe synchronous forms were unsound: "both remedies are
required. The current code path must be marked unsafe, and to make it safe we need to have a mechanism to
structurally guarantee that a named pipe was not opened for overlapped access for safe i/o to be performed."

**The hazard is memory, not a wrong answer.** `ConnectNamedPipe` and `TransactNamedPipe` with a NULL `OVERLAPPED`, on
a handle opened with `FILE_FLAG_OVERLAPPED`, "can incorrectly report that the [connect] operation is complete"
(both pages, `ms.date` 12/05/2018). For a transaction that means the call returns while the read of the reply is
still outstanding, the caller's `output` borrow ends, and the kernel later writes the reply into memory the caller
has reused. [WP-D-2](#wp-d-2) and [WP-D-5](#wp-d-5) had put the condition on the caller in prose, which is the shape
of an `unsafe` precondition and was attached to a safe function.

**Both remedies.**

- `connect_named_pipe_unchecked(BorrowedHandle)` and `transact_named_pipe_unchecked(BorrowedHandle, ..)` are the
  previous functions, `unsafe`, with the condition as their safety contract.
- `connect_named_pipe(&SynchronousPipe)` and `transact_named_pipe(&SynchronousPipe, ..)` are safe because of the type
  they take, and call the unchecked forms.

**What a `SynchronousPipe` is.** An owned handle that was not opened with `FILE_FLAG_OVERLAPPED`. No documented call
reports that of an open handle (WP-D-2), so the type is the record of it, made where the handle is opened:

- `SynchronousPipe::create` is `create_named_pipe` for options without `PipeOpenMode::OVERLAPPED`.
- `SynchronousPipe::open_client` opens a client end: `CreateFileW` with `OPEN_EXISTING`, the caller's access, share
  mode and flags in `PipeClientOptions`, and no template.
- `from_owned_unchecked` is `unsafe`, for a handle opened some other way; its contract names the three openings
  that qualify, among them an anonymous pipe's end ("Anonymous Pipe Operations", `ms.date` 05/31/2018:
  "Asynchronous (overlapped) read and write operations are not supported by anonymous pipes").
- `try_clone` duplicates the handle and is one too: "The duplicate handle refers to the same object as the original
  handle" (`DuplicateHandle`, `ms.date` 12/05/2018), and how a handle was opened is a property of what it was opened to.
- `into_owned` and `From<SynchronousPipe> for OwnedHandle` give the handle back, for reads and writes through `File`.

**The refusals are this crate's narrowing, not the calls'.** Both pages accept the overlapped flag. `create` and
`open_client` refuse it with `InvalidInput` and no OS code, before any call, because a handle with it would not be
what the type says. A name with an interior NUL is refused the same way, as everywhere in the crate.

**Why the client open is here.** The type needs an opener that is known not to set the flag. `std`'s `OpenOptions`
was rejected: it documents what it sets, not that it never sets `FILE_FLAG_OVERLAPPED`, and its `custom_flags` says
custom flags "can only set flags, not remove flags set by Rust's options", so a `SynchronousPipe` made from it would
rest on what `std` currently does. The opener here takes only a pipe's client end (`OPEN_EXISTING`, since a client
opens an instance that exists) and states every argument it passes.

**Observed on 2026-10-19, Windows 11 build 26300, local, unelevated, and not a ruling.** A message pipe's client in
message-read mode, a transaction waiting for a reply the server sends only after a `GetNamedPipeInfo` on a duplicate of
the client has started. With the client a `SynchronousPipe`, the query waited and the transaction read the whole
reply. With the client opened with `FILE_FLAG_OVERLAPPED` (the sabotage "review: a SynchronousPipe client is opened
overlapped" in [sabotage.json](sabotage.json)), the transaction returned success with `bytes_read` 0, which is the
reviewer's observation reproduced. The test is
`transact::tests::a_query_on_the_same_pipe_during_a_transaction_does_not_end_it_early`.
