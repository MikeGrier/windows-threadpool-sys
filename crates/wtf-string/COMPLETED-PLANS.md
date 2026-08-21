# Completed plans: wtf-string

Append-only archive of completed checklists, moved out of [PLANS.md](PLANS.md).

| Path to CHECKLIST.md | Completion Date | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | 2026-08-21 | `OsString`-shaped strings with native `u16` (WTF-16), conversion-free storage for Windows FFI. All ten planned milestones landed (M1 scaffold+design -> M10 docs/publication): an encoding-generic core (`WtfString<E>` / `WtfStr<E>`) with always-terminated storage, both the `Wtf16` and `Wtf8` arms, portable `str`/`String` conversions, the Win32 FFI pointer surface (counted + terminated input, buffer-fill and callee-allocated output), Windows-only lossless `OsStr` interop, a safe `OsString`-parity mutation surface, optional `windows`-crate `Param<PCWSTR>` interop, and a `no_std`/`alloc`-only baseline proven against a bare-metal target. Decisions D-1...D-18 are recorded. The checklist file remains for its parked `M-inf` horizon bucket, which holds no pending work. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
