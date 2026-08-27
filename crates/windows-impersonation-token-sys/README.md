# windows-impersonation-token-sys

Memory-safe capture, transport, and scoped application of Windows impersonation
tokens.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Status

The publishable crate skeleton and owned `ImpersonationToken` capture type are
complete. Scoped application, deterministic tests, and final publication
documentation are tracked as IT-3 through IT-5 in the workspace
[CHECKLIST.md](../../CHECKLIST.md).

## Scope

This crate will own the narrow impersonation-token lifecycle needed by
cross-thread Windows work:

- capture the calling thread's effective impersonation state;
- transport owned state to another thread;
- apply it for a bounded operation; and
- restore the exact prior thread-token state.

It is not a general Windows security or access-token utility collection. The
canonical contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md), with historical
reasoning in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

## License

MIT. Copyright (c) Mike Grier.
