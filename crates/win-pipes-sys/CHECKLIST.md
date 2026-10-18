# Checklist: win-pipes-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). The crate was made by the engineer's ruling of
2026-10-18, in answer to which crate `CreateNamedPipeW` should be exposed from. The area is the *Pipe
Functions* Microsoft Learn lists under *System Services > Interprocess Communications > Pipes*
([WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt)); one function is built, and the rest are queued.

- [x] **WP-1.1** -- `create_named_pipe`, `CreateNamedPipeW` with typed open and pipe modes. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-11)

- [x] **WP-1.2** -- The workspace's other sites that call `CreateNamedPipeW` with an `unsafe` block of their own make their pipes with this crate. -> [completed 2026-10-18](COMPLETED-CHECKLIST.md#wp-12)

- [ ] **WP-1.3** -- `ConnectNamedPipe` and `DisconnectNamedPipe`, the server end's connection, including its
  overlapped form and `ERROR_PIPE_CONNECTED` as the call reports it. `windows-overlapped-io-sys`'s
  `tests/overlapped_cancellation.rs` issues `ConnectNamedPipe` raw, three times, and is the first consumer.

- [ ] **WP-1.4** -- `CreatePipe`, the anonymous pipe, returning the two ends as owned handles.

- [ ] **WP-1.5** -- `GetNamedPipeInfo`, `GetNamedPipeHandleState` and `SetNamedPipeHandleState`, taking and
  returning the typed modes, so the tests of this crate that read them through raw calls can stop.

- [ ] **WP-1.6** -- `PeekNamedPipe`, `TransactNamedPipe`, `CallNamedPipe` and `WaitNamedPipe`, the operations on a
  connected pipe and on a name.

- [ ] **WP-1.7** -- `GetNamedPipeClientComputerName`, `GetNamedPipeClientProcessId`,
  `GetNamedPipeClientSessionId`, `GetNamedPipeServerProcessId` and `GetNamedPipeServerSessionId`, the queries of
  who is at the other end.
