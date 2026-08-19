# Plans: wtf-string

Active planned work for the crate. When the whole checklist completes its entry moves to a
COMPLETED-PLANS.md tracker, and completed milestones archive to a COMPLETED-CHECKLIST.md, both created in
this directory at that time.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | `OsString`-shaped strings with native `u16` (WTF-16), conversion-free storage for Windows FFI: an encoding-generic core (`WtfString<E>` / `WtfStr<E>`) shipping the `Wtf16` arm, always-terminated storage, portable `str`/`String` conversions, and Windows-only lossless `OsStr` interop. Six milestones (M1 scaffold+design → M6 docs). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
