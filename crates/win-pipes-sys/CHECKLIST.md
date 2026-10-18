# Checklist: win-pipes-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by the engineer's ruling of
2026-10-18, in answer to which crate `CreateNamedPipeW` should be exposed from. The area is the *Pipe
Functions* Microsoft Learn lists under *System Services > Interprocess Communications > Pipes*
([WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt)); one function is built, and the rest are queued.

- [x] **WP-1.1** -- `create_named_pipe`, `CreateNamedPipeW` with typed open and pipe modes. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-11)

- [x] **WP-1.2** -- The workspace's other sites that call `CreateNamedPipeW` with an `unsafe` block of their own make their pipes with this crate. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-12)

- [x] **WP-1.3** -- `ConnectNamedPipe` in its two forms and `DisconnectNamedPipe`, with `ERROR_PIPE_CONNECTED` and `ERROR_IO_PENDING` named as results. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-13)

- [x] **WP-1.3.1** -- `windows-overlapped-io-sys`'s `tests/overlapped_cancellation.rs` issues its three `ConnectNamedPipe` calls through `connect_named_pipe_overlapped`. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-131)

- [x] **WP-1.4** -- `CreatePipe`, the anonymous pipe, returning the two ends as owned handles. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-14)

- [x] **WP-1.5** -- `GetNamedPipeInfo`, `GetNamedPipeHandleStateW` and `SetNamedPipeHandleState`, taking and returning the typed modes. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-15)

- [x] **WP-1.6** -- `PeekNamedPipe`, `TransactNamedPipe`, `CallNamedPipeW` and `WaitNamedPipeW`, the operations on a connected pipe and on a name. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-16)

- [ ] **WP-1.7** -- `GetNamedPipeClientComputerName`, `GetNamedPipeClientProcessId`,
  `GetNamedPipeClientSessionId`, `GetNamedPipeServerProcessId` and `GetNamedPipeServerSessionId`, the queries of
  who is at the other end.
