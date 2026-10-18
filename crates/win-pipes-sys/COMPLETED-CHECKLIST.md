# Completed checklist: win-pipes-sys

## Moved 2026-10-18 17:21:37 -04:00 -- The first pipe function

### <a id="wp-11"></a>WP-1.1 -- `create_named_pipe`, `CreateNamedPipeW` with typed open and pipe modes. *(completed 2026-10-18 17:21:37 -04:00)*

The engineer, 2026-10-18, on which crate `CreateNamedPipeW` should be exposed from. The placement, the arguments the
crate passes as given and the one it refuses, and what was observed are [WP-D-1](DESIGN-NOTES.md#wp-d-1).
Eight sabotages, one a control that survives by design, and one caught by the suite hanging.
