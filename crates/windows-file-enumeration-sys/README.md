# windows-file-enumeration-sys

Memory-safe asynchronous enumeration of one Windows directory with bounded
submission and completion rings.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Status

The public API, session, native enumeration engine, and real-Windows
integration suite are complete, including a Globazog adapter demonstration
discharging the D-15 acceptance gate. Publication validation is tracked by
FE-16 in the workspace [CHECKLIST.md](../../CHECKLIST.md).

## Scope

This crate owns flat one-directory enumeration:

- begin and control operations enter through a bounded multi-producer submission
  ring;
- entries and exactly one terminal outcome per accepted request leave through a
  bounded single-receiver completion ring;
- directory handles are opened under an explicitly captured
  `ImpersonationToken`;
- native paths and names retain WTF-16 fidelity; and
- caller-owned `GetFileInformationByHandleEx` buffers provide lossless bounded
  staging under completion-ring backpressure.

It is not a recursive traversal engine. A traversal layer composes multiple flat
requests without moving recursion, breadth/depth policy, or tree-wide scheduling
into this crate.

## Examples

### Ordinary submission

One directory, one session, drained to its terminal outcome:

```rust,no_run
use windows_file_enumeration_sys::{Completion, EnumerationRequest, Session};

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let (session, receiver) = Session::new(8, 8)?;
let request = EnumerationRequest::for_path("C:/logs".as_ref())?;
session.try_begin(request)?.detach();

while let Some(completion) = receiver.recv() {
    match completion {
        Completion::Entry { entry, .. } => println!("{}", entry.name()),
        Completion::Terminal { outcome, .. } => {
            println!("finished: {outcome:?}");
            break;
        }
    }
}
# Ok(())
# }
```

### Traversal-style submission

A traversal layer captures one security context and reuses it across every
directory in the tree with `Session::try_begin_with_token`, instead of paying a
fresh capture per directory:

```rust,no_run
use windows_file_enumeration_sys::{EnumerationRequest, Session};
use windows_impersonation_token_sys::ImpersonationToken;

# fn main() -> Result<(), Box<dyn std::error::Error>> {
let (session, receiver) = Session::new(8, 8)?;
let token = ImpersonationToken::capture()?;

for directory in ["C:/logs", "C:/logs/archive"] {
    let request = EnumerationRequest::for_path(directory.as_ref())?;
    session
        .try_begin_with_token(request, token.clone())?
        .detach();
}
# drop(receiver);
# Ok(())
# }
```

The canonical contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md), with historical
reasoning in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The originating
discussion is in the workspace
[design session](../../design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).


## License

MIT. Copyright (c) Mike Grier.
