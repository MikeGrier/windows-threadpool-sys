# windows-namespace-request-sys

Owned, marshalable parameter sets for synchronous Win32 namespace calls.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Why

Win32's namespace and metadata surface -- opening, querying, closing -- is
synchronous-only. There is no overlapped `CreateFileW`, no overlapped
`GetFileInformationByHandleEx`. A call that blocks on a dead network path blocks
the thread that made it, and on a shared thread that is somebody else's problem
too.

This crate makes such a call **capturable as a value**: an owned parameter set
built on one thread and executed faithfully on another. It schedules nothing --
it is the catalogue-plus-faithful-execution layer, testable with no ring, no
pool, and no async anywhere near it.

## What a request is, and is not

A request carries call **parameters**, not the thread state the call runs under.
Impersonation and the rest belong to
[windows-thread-ambient-sys](../windows-thread-ambient-sys/README.md), which this
crate does not depend on. The two are siblings rather than a stack: a request can
be executed with no captured context at all, and a context is useful to work that
never opens a file. Whoever owns both pairs them at the submission site.

A request also chooses **no delivery model**. An opened handle comes back plain
and unassociated, because associating it with a completion port irreversibly
forecloses `IoRing` use of it, and that choice belongs to a layer that knows the
handle's destination.

## A path is copied; a handle is duplicated

Several entries take a handle rather than a path, and a request owns a
**duplicate** of any handle it names. The distinction is easy to get backwards: a
path is a value and is copied, while a handle is a reference to a kernel object,
so duplicating it *shares that object* rather than cloning it.

A request is therefore self-contained with respect to **lifetime** -- it cannot
be left pointing at a handle its originator closed -- and **not** isolated with
respect to **state**. Measured: a duplicated handle continues the source's
directory enumeration rather than starting its own, closing the duplicate leaves
the source usable, and single-shot metadata queries disturb nothing. An
independent traversal needs a fresh open, not a duplicate.

## Example

Capture on the submitting thread, where a failure is still the caller's to see,
then use the result on a worker that saw none of the inputs:

```rust
use std::fs;
use std::os::windows::io::AsHandle;
use std::thread;

use windows_namespace_request_sys::{CapturedHandle, prepare};
use wtf_string::Wtf16String;

let path = std::env::temp_dir().join(format!("wnrs-readme-{}.tmp", std::process::id()));
fs::write(&path, b"example")?;

// Resolved here rather than on the worker: the process current directory is
// shared mutable state that any thread can change in between.
let text = path.to_str().expect("a temporary path is valid UTF-8");
let prepared = prepare(&Wtf16String::from(text))?;
assert_eq!(prepared.as_wtf16().to_string_lossy(), text);

// An owned duplicate, so the captured parameters cannot be left pointing at a
// handle the caller has since closed.
let file = fs::File::open(&path)?;
let captured = CapturedHandle::capture(file.as_handle())?;
drop(file);

let length = thread::spawn(move || {
    fs::File::from(captured.into_owned_handle()).metadata().map(|m| m.len())
})
.join()
.expect("the worker did not panic")?;

assert_eq!(length, b"example".len() as u64);
# fs::remove_file(&path)?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Round-one entries

One entry per Win32 call. The list is audited from three real consumers -- this
repository's file watcher and enumeration crates, and `MikeGrier/Globazog-rs` --
rather than chosen by taste:

`CreateFileW`, `OpenFileById`, `FindFirstChangeNotificationW`, `CloseHandle` and
variant close routines, `GetFileInformationByHandleEx`,
`GetFileInformationByHandle`, `GetFinalPathNameByHandleW`,
`GetVolumeInformationByHandleW`, `GetFullPathNameW`.

Deletion, rename, directory creation, attribute setting, link creation, and the
`FindFirstFileExW` family are **deliberately** out of round one: no audited
consumer calls them. [DESIGN-NOTES.md](DESIGN-NOTES.md) records the full list so
a considered omission is distinguishable from an unexamined one.

## Status

Early. The boundary decisions are recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md);
implementation is queued as milestones M24 to M26 of
[CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md). Not yet ready
for a crates.io release.
