# Spike reproducers

Standalone programs that established behaviour this crate's design notes now assert. They are
**not** built by the workspace: each is a single file written directly against `windows-sys` with no
dependency on this crate, so that what it measures is the operating system's behaviour and not ours.

To run one, drop it into a scratch binary crate with a single dependency:

```toml
[dependencies]
windows-sys = { version = "0.61.2", default-features = false, features = [
    "Win32_Foundation", "Win32_Security", "Win32_Storage_FileSystem",
    "Win32_System_IO", "Win32_System_Threading",
] }
```

| File | Establishes |
|---|---|
| [completion-event-spike.rs](completion-event-spike.rs) | [D-19](../../DESIGN-NOTES.md#d-19) -- the completion event is edge-triggered on the completion queue going empty to non-empty; also what `SetIoRingCompletionEvent` permits (call at any time, replace, clear with `NULL`, duplicate survives closing the original) |
| [drain-ordering-spike.rs](drain-ordering-spike.rs) | [D-23](../../DESIGN-NOTES.md#d-23) -- an unflagged flush does not cover preceding writes; [D-24](../../DESIGN-NOTES.md#d-24) -- `DRAIN_PRECEDING_OPS` is a full, ring-wide barrier spanning submissions |

## Why the drain spike looks over-built

It carries a concurrency check and a control case because the first two versions of it **could not
discriminate** and would have produced confidently wrong answers:

1. Buffered writes complete in submission order, so a barrier and no barrier looked identical.
2. `NO_BUFFERING` but *extending* writes -- still identical, because the filesystem serializes
   extending writes and writes past the valid-data length.
3. `NO_BUFFERING` over a **pre-written extent** -- 28 of 32 small writes overtook large ones with no
   flags at all, which is the baseline that makes the barrier results mean anything.

The concurrency check is retained precisely so that a future run on different hardware reports
"results below are VACUOUS" rather than a false pass. A control that matches the treatment means the
harness is measuring nothing.

## Re-running these is worthwhile

Every behaviour here is **undocumented**, measured on a single machine (`IoRing` version 400, real
kernel ring, `UM_EMULATION` absent), against one device. None of it is a contract Microsoft has
published, so it could differ on another Windows build, on Server, or under the user-mode emulation
path -- and could in principle change under servicing. If you have access to different hardware or a
different OS build, running these and recording the result is a cheap contribution.
