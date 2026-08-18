# windows-overlapped-io-sys

Owned overlapped I/O endpoints and pinned operations for Windows.

**Windows only.** Every item is behind `cfg(windows)`; the crate builds to an
empty shell on other platforms.

This crate provides the ownership, association, completion, cancellation, and
rundown model for overlapped I/O on top of `windows-sys`. It is the reusable
foundation beneath `windows-threadpool-sys`: raw I/O completion ports and the
object-based thread pool share endpoint and operation storage while remaining
distinct completion backends.

## Operation-family adapters

Endpoints are opened safely with `UnassociatedEndpoint::open`, and each operation
family has an adapter behind an opt-in Cargo feature. The `fs` and `socket`
adapters are fully safe; the `device` adapter owns its buffers but its `ioctl` is
`unsafe`, because an arbitrary control code may embed pointers it cannot own:

| Feature | Adapter | Safe? |
|---|---|---|
| `fs` | file read/write and scatter/gather, on the blocking and IOCP backends | yes |
| `socket` | socket send/receive, on the blocking and IOCP backends | yes |
| `device` | `DeviceIoControl`, on the blocking and IOCP backends | no — buffer-owning `unsafe` raw-code seam |

The default feature set is empty, keeping the core completion machinery (raw IOCP
and blocking backends, owned endpoints, pinned operations) minimal. A narrow
unsafe submission seam remains available for families without an adapter, and the
optional `operation-backtrace` feature captures a submit-site backtrace for the
drop-time outstanding-operation diagnostic.

Fully generic, fully safe overlapped submission remains intentionally unsolved;
the per-family adapters are the sanctioned safe path.

## Operation identity

Submitting returns an `OperationId` that names *that* operation for the life of
the process, not merely while its storage address stays put. This matters
because cancellation is a safe operation racing a completion it cannot observe:

- An operation's storage address is reused once the operation is reclaimed, so
  an address alone would let an identity retained a moment too long refer to a
  different, live operation.
- Each identity therefore carries a process-wide generation taken at submission,
  and each backend keeps a registry of live identities.
- `cancel` validates the identity first and rejects a stale one with
  `ErrorKind::NotFound` **without** calling `CancelIoEx`, so a recycled address
  is never handed to the kernel on the caller's behalf.

The result is that holding an identity too long is harmless, and a late cancel
fails rather than silently cancelling an unrelated operation.
