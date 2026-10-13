# Plans: io-buffers

| Path to CHECKLIST.md | Status | Brief description | Design Notes |
|---|---|---|---|
| [CHECKLIST.md](CHECKLIST.md) | in progress | Buffers for asynchronous I/O, portable: the `IoBuf` and `IoBufMut` contract moved out of `windows-ioring-sys` so `durable-ioring`'s portable core can name it, adopted by the ring crate and by `win-numa-sys`, and one owned aligned buffer. Also the merge point for the buffer traits `windows-ioring-sys` and `windows-overlapped-io-sys` each duplicate. | [DESIGN-NOTES.md](DESIGN-NOTES.md) |
