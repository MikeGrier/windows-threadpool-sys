# win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns, more
efficiently than one timer per file.

Each registered file is opened once and the handle is kept. On every poll the crate reads each
file's current length through that handle, never by path and never from directory change
notifications, and reports the files whose length changed. The crate attaches no meaning to a
change and is never told what the caller consumed. It builds only on Windows.

What was decided, and why: [DESIGN-NOTES.md](DESIGN-NOTES.md). The work, in order:
[CHECKLIST.md](CHECKLIST.md).
