# windows-file-enumeration-sys

Memory-safe asynchronous enumeration of one Windows directory with bounded
submission and completion rings.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Status

The publishable crate boundary, release automation, and v1 public API design are
complete. Implementation is tracked by FE-3 through FE-11 in the workspace
[CHECKLIST.md](../../CHECKLIST.md).

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

The canonical contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md), with historical
reasoning in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md). The originating
discussion is in the workspace
[design session](../../design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).

## License

MIT. Copyright (c) Mike Grier.
