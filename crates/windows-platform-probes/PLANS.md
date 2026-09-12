# Plans: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md).

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | M1: stream a probe's report as it is measured. The report sink buffers each report into a `String`, so a termination that does not unwind -- Ctrl-C, or an abort during unwinding -- discards it, where the line-by-line printing it replaced kept it. Costs most on `probe-cancel-io`, which runs about twenty seconds precisely when the wedge it hunts for occurs. M2: check correspondence between a report's parts rather than each part alone, after a pull-request review found the renderer calling a state a bug while the verdict certified the same run as `agree`. | [DESIGN-NOTES.md](DESIGN-NOTES.md#d-buffered-report), [#d-correspondence-failures](DESIGN-NOTES.md#d-correspondence-failures) |
| [../../CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) | in progress | M27: create the crate, migrate this session's probes into it under the three-tier scheme, and queue migration of the nine earlier measurements that still live only in git-ignored scratch. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
