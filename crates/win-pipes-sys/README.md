# win-pipes-sys

Memory-safe Rust over the Win32 pipe API, the area Microsoft Learn files under *System Services > Interprocess
Communications > Pipes*. Thin over Win32, adding no policy.

- `create_named_pipe` is `CreateNamedPipeW`: it creates an instance of a named pipe and returns the server end as
  an `OwnedHandle`, so the instance is closed when the handle is dropped and nothing else has to.
- `PipeOpenMode` is `dwOpenMode` and `PipeMode` is `dwPipeMode`: newtypes over the Win32 bits, combined with
  `|`, with every mode the call's page lists named. A combination the page calls invalid is not prevented here;
  the call refuses it, and its error reaches the caller as the call gave it.
- `NamedPipeOptions` carries the rest of the parameters, none defaulted.
- `connect_named_pipe` and `connect_named_pipe_overlapped` are `ConnectNamedPipe` in its two documented forms: with no
  `OVERLAPPED`, which waits, and with one, which starts the operation and returns. The synchronous form must not be
  given a pipe created overlapped, and no documented call reports how a handle was created, so that is the caller's to
  keep; the overlapped form is `unsafe`, because the `OVERLAPPED` must outlive the operation. A client that connected
  before the call, which the call reports as zero with `ERROR_PIPE_CONNECTED`, is a result (`AlreadyConnected`) and
  not an error. See [WP-D-2](DESIGN-NOTES.md#wp-d-2).
- `disconnect_named_pipe` is `DisconnectNamedPipe`.
- `create_pipe` is `CreatePipe`: it creates an anonymous pipe and returns the two ends as an `AnonymousPipe` with
  `read` and `write` fields, each an `OwnedHandle`. The one attributes parameter applies to both ends. See
  [WP-D-3](DESIGN-NOTES.md#wp-d-3).
- `get_named_pipe_info`, `get_named_pipe_handle_state` and `set_named_pipe_handle_state` are `GetNamedPipeInfo`,
  `GetNamedPipeHandleStateW` and `SetNamedPipeHandleState`. The state read asks only for what a `PipeStateQuery`
  names, because the pages make some of the call's optional outputs an error on some pipes; the state set changes
  only what a `PipeStateUpdate` carries. The modes in and out are `PipeMode`. See
  [WP-D-4](DESIGN-NOTES.md#wp-d-4).

The area has more in it: the rest of the *Pipe Functions* the section lists (`PeekNamedPipe` and the
others) are queued in [CHECKLIST.md](CHECKLIST.md), and are not here yet.

```rust
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use win_pipes_sys::{NamedPipeOptions, PipeMode, PipeOpenMode, create_named_pipe};

let name = format!(r"\\.\pipe\win-pipes-sys-readme-{}", std::process::id());
let options = NamedPipeOptions {
    open_mode: PipeOpenMode::ACCESS_DUPLEX,
    pipe_mode: PipeMode::TYPE_BYTE | PipeMode::WAIT,
    max_instances: 1,
    out_buffer_size: 4096,
    in_buffer_size: 4096,
    default_timeout_ms: 0,
};

let mut server = File::from(create_named_pipe(&name, &options, None)?);
let mut client = OpenOptions::new().read(true).write(true).open(&name)?;

client.write_all(b"ping")?;
let mut received = [0u8; 4];
server.read_exact(&mut received)?;
assert_eq!(&received, b"ping");
# Ok::<(), std::io::Error>(())
```

The crate was made so that the tests of this workspace that each call `CreateNamedPipeW` with an `unsafe` block of
their own have one safe wrapper to share.
