# Completed checklist: win-pipes-sys

## Moved 2026-10-18 17:21:37 -04:00 -- The first pipe function

### <a id="wp-11"></a>WP-1.1 -- `create_named_pipe`, `CreateNamedPipeW` with typed open and pipe modes. *(completed 2026-10-18 17:21:37 -04:00)*

The engineer, 2026-10-18, on which crate `CreateNamedPipeW` should be exposed from. The placement, the arguments the
crate passes as given and the one it refuses, and what was observed are [WP-D-1](DESIGN-NOTES.md#wp-d-1).
Eight sabotages, one a control that survives by design, and one caught by the suite hanging.

### <a id="wp-12"></a>WP-1.2 -- The workspace's other sites that call `CreateNamedPipeW` with an `unsafe` block of their own make their pipes with this crate. *(completed 2026-10-18 18:02:44 -04:00)*

Seven files in five crates, found with a search for the call on 2026-10-18, each crate its own commit and none moved
as a side effect of this one ([the rule](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under)):
`windows-ioring-sys`'s `tests/bounded_pop.rs`; `windows-threadpool-sys`'s `tests/tp_io_operation_identity.rs` and
`tests/tp_io_behavioral_matrix.rs`; `windows-overlapped-io-sys`'s `tests/overlapped_cancellation.rs` (three pipes) and
`tests/operation_identity.rs`; `win-volume-sys`'s `src/device_control/tests.rs`; and `windows-platform-probes`'s
`src/ioring.rs`, which is probe code and not a test, with a normal path-only dependency as every workspace
dependency of that crate is. `windows-namespace-request-sys`'s evidence test had been the first. A search for the
call now finds only this crate's own.

Each site kept its test and its behaviour: the constants that named the modes are gone, and so are the `unsafe`
blocks and the null-and-invalid checks; the Win32_System_Pipes test feature went with the site that alone used it,
and stays in `windows-overlapped-io-sys` for the raw `ConnectNamedPipe` that remains ([WP-1.3](CHECKLIST.md)).
Each crate's tests passed unchanged, and the `win-volume-sys` sabotages that depend on its pipe are still caught.

**A convention this item surfaced and corrected.** A dev-dependency on a workspace crate is path-only, with no
`version`: Cargo checks a pinned version against the path at every build, so a release of the dependency breaks the
depending crate, and a versionless dev-dependency is left out of the published manifest
(`windows-ioring-sys`'s manifest records the reason). The dev-dependencies added in the previous two commits of this
work carried a version and were corrected first, in `chore` commits of their own.

### <a id="wp-13"></a>WP-1.3 -- `ConnectNamedPipe` in its two forms and `DisconnectNamedPipe`, with `ERROR_PIPE_CONNECTED` and `ERROR_IO_PENDING` named as results. *(completed 2026-10-18 18:11:42 -04:00)*

`connect_named_pipe`, `connect_named_pipe_overlapped` (`unsafe`), `disconnect_named_pipe`, `ConnectOutcome` and
`ConnectStart`. Why there are two connect functions and not one, why the synchronous one cannot check the handle, and
what was observed are [WP-D-2](DESIGN-NOTES.md#wp-d-2). Twelve tests and seven sabotages, one a control that survives
and one caught by the suite hanging. The first consumer, `windows-overlapped-io-sys`'s raw calls, is
[WP-1.3.1](CHECKLIST.md).

### <a id="wp-131"></a>WP-1.3.1 -- `windows-overlapped-io-sys`'s `tests/overlapped_cancellation.rs` issues its three `ConnectNamedPipe` calls through `connect_named_pipe_overlapped`. *(completed 2026-10-18 18:18:56 -04:00)*

The three raw calls became one `unsafe` helper in that file, which maps `Pending` and a finished `Succeeded` to the
port's `Issued::Pending` and an already connected client to `ERROR_PIPE_CONNECTED`, as the raw closures did. The
`Win32_System_Pipes` dev feature, which only that file used, is removed from the crate's manifest.

### <a id="wp-14"></a>WP-1.4 -- `CreatePipe`, the anonymous pipe, returning the two ends as owned handles. *(completed 2026-10-18 18:34:46 -04:00)*

`create_pipe` and `AnonymousPipe`, with `read` and `write` fields. Why a struct, why one attributes parameter serves
both ends, and the error edge no test reaches are [WP-D-3](DESIGN-NOTES.md#wp-d-3). Four sabotages, two of them
declared survivors.

### <a id="wp-15"></a>WP-1.5 -- `GetNamedPipeInfo`, `GetNamedPipeHandleStateW` and `SetNamedPipeHandleState`, taking and returning the typed modes. *(completed 2026-10-18 18:45:44 -04:00)*

`get_named_pipe_info` with `PipeInfo` and `PipeInfoFlags`; `get_named_pipe_handle_state` with `PipeStateQuery` and
`PipeHandleState`; `set_named_pipe_handle_state` with `PipeStateUpdate`. The three raw reads in this crate's own
`create_named_pipe` tests now go through them. Why the state read takes a request, the user name's capacity, and what
was observed are [WP-D-4](DESIGN-NOTES.md#wp-d-4). Eleven sabotages, two of them declared survivors.
