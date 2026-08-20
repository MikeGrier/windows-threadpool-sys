# Plans: wtf-string

Active planned work for the crate. When the whole checklist completes, its entry and its completed
milestones are archived to sibling completed-plans and completed-checklist trackers created in this
directory at that time.

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | `OsString`-shaped strings with native `u16` (WTF-16), conversion-free storage for Windows FFI: an encoding-generic core (`WtfString<E>` / `WtfStr<E>`) shipping the `Wtf16` arm, always-terminated storage, portable `str`/`String` conversions, Windows-only lossless `OsStr` interop, plus a `Wtf8` arm, a safe `OsString`-parity mutation surface, `Param<PCWSTR>` interop, and `no_std` support. Ten milestones (M1 scaffold+design -> M10 docs/publication). | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
