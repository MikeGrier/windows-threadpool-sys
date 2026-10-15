# Completed checklist: win-multi-file-tail-watcher

Append-only record of finished [CHECKLIST.md](CHECKLIST.md) items.

## Moved 2026-10-15 13:59:42 -04:00 -- TW-1.1: the crate

### <a id="tw-11"></a>TW-1.1 -- The crate exists: a Windows-only workspace member registered for release and publication, with its README as the crate documentation and an undocumented `unsafe` refused by the build. *(completed 2026-10-15 13:59:42 -04:00)*

Registered as `win-time-sys` was: a workspace member, in release-please's configuration and manifest
(at `0.0.0`, so the first release is `0.1.0`), and in the publish workflow's tag filter,
manual-dispatch choices and workspace-sibling list. `README.md` replaces `COMPONENT.md` and is the
crate documentation. Dependencies are the three the item named: [wtf-string](../wtf-string/),
[windows-overlapped-io-sys](../windows-overlapped-io-sys/) with `default-features = false` and `fs`
only (the feature that holds the observer's length query, so none of its other optional
dependencies come along), and `windows-sys` with no Win32 feature, since each arrives with the call
that needs it. Each path dependency carries its version, as the repository's dependency-version
check requires.

`lib.rs` denies `missing_docs`, `unsafe_op_in_unsafe_fn` and
`clippy::undocumented_unsafe_blocks`. Sabotage: a public function with no documentation, and an
`unsafe` block with no safety comment, each failed `cargo clippy`; the edit was reverted. The crate
has no code and so no unit tests. `tools/check-workflow-refs.ps1`,
`.github/release/check-dependency-versions.cjs` and `tools/check-publishable.ps1` all pass with it
registered; the last two need the release scripts' locked dependencies (`npm ci` in
`.github/release`).

## Moved 2026-10-15 14:18:13 -04:00 -- TW-1.2: opening a file

### <a id="tw-12"></a>TW-1.2 -- `HeldFile::open` opens a file by `Wtf16String` path as a read-only, overlapped endpoint, and hands out observers. *(completed 2026-10-15 14:18:13 -04:00)*

`HeldFile` keeps a windows-overlapped-io-sys `UnassociatedEndpoint` and gives out its observers
([TW-D-10](DESIGN-NOTES.md#tw-d-10)); the name is provisional, recorded as a working position
rather than a decision. `open` converts the path with `Wtf16Str::to_os_string`, which keeps unpaired
surrogates, and opens it through the endpoint's own `open` with read access only. That call always
sets `FILE_FLAG_OVERLAPPED` and leaves the sharing mode at read, write and delete, so neither is
re-stated here; the tests below show the sharing from outside. Nothing in this crate checks the
path first: a failed open is `std`'s `io::Error` carrying the Win32 code.

Measured, and each a test: a missing file is `ERROR_FILE_NOT_FOUND`; a missing directory and an empty
path are `ERROR_PATH_NOT_FOUND`; a directory is `ERROR_ACCESS_DENIED`; a file another handle holds with no
sharing is `ERROR_SHARING_VIOLATION`; a path containing a NUL is refused by `std` as `InvalidInput`
before any call, with no Win32 code. Another handle can still write to and delete a held file, and the
hold survives the delete. Read-only is shown from the other side: a new open that denies write sharing
succeeds beside the hold, which a hold with write access would make a sharing violation. Names with
spaces, accents, CJK, an astral character and an unpaired surrogate open, as does a path past
`MAX_PATH` in its verbatim form. Two holds of one path are independent, and every observer of one hold
shares its handle and keeps it open after the `HeldFile` drops.

That the handle is overlapped is not tested here, since the only handle to it is the unsafe borrow an
observer offers, whose contract forbids associating it. The sabotage manifest
([sabotage.json](sabotage.json)) is this crate's first: opening with write access, converting the
path lossily, and losing the Win32 code are each caught, and a differently spelled conversion
survives as the control.

## Moved 2026-10-15 14:24:34 -04:00 -- TW-1.3: reading the length

### <a id="tw-13"></a>TW-1.3 -- `HeldFile::len` reads the file's current length through an observer of the held handle. *(completed 2026-10-15 14:24:34 -04:00)*

`len` takes a new observer and asks it for `file_len`, which is `standard_info`'s end-of-file
([TW-D-2](DESIGN-NOTES.md#tw-d-2)). The path is not consulted. The method is named `len` and carries
no `is_empty`, since the query can fail.

Tested, each against a real file: lengths of 0, 1, 5, 4096, 4097 and 100000 bytes; a length that
tracks twelve writes by a writer that stays open; a shrink by another handle, repeated to the same
size and to zero; a file of 2^32 + 5 bytes; the length after the file is deleted while held; the
length after a rename with a different file recreated at the old path, which reads the held file
and not the new one; an observer that outlives the hold and sees a later append; two holds of one
path; fifty steady reads. The sabotage manifest catches reading the allocation size instead, a
32-bit cut and a cached first read, and a length read through `standard_info` survives as the control.

Not tested here: a read on the thread pool, and a file on a remote or non-NTFS volume. The
directory entry's lag behind the held handle is recorded under [TW-D-2](DESIGN-NOTES.md#tw-d-2) and is
not re-measured.

## Moved 2026-10-15 14:45:25 -04:00 -- TW-1.4: the starting offset

### <a id="tw-14"></a>TW-1.4 -- `HeldFile::open_at` validates a starting offset against the length read at open, and refuses one beyond it with its own error. *(completed 2026-10-15 14:45:25 -04:00)*

`open_at(path, offset)` opens as `open` does, reads the length through the new handle, and returns
`OpenError::OffsetBeyondLength { offset, len }` when `offset > len` ([TW-D-3](DESIGN-NOTES.md#tw-d-3)).
An offset equal to the length is accepted, so a parent that stopped at the end of a file can
restart there. `OpenError::Io` carries every other failure, with its Win32 code, including a
failed length read. The accepted offset is kept and read back with `start_offset()`; `open` is
`open_at` with offset 0 and returns `io::Result` as before. Whether the offset is the watcher's
first "last known length" is not decided here; it is the TW-2.x items' to settle.

Tested: offsets 0, 1, 5, 9 and 10 on a 10-byte file are accepted and 11, 12, 4096, 2^32,
2^32 + 3 and `u64::MAX` are refused with both numbers; an empty file takes only 0; the check is made
against the length at open, so a later shrink leaves an accepted offset in place and a fresh open
at it is refused; a file of 2^32 + 5 bytes accepts its length and one less and refuses one more; a
missing file is `Io` with `ERROR_FILE_NOT_FOUND` whatever the offset, and `Io` has a source where
the offset error has none.

The first sweep found a hole: with only small refused offsets, comparing as 32 bits survived,
because none had low 32 bits within the length. Offsets 2^32 and 2^32 + 3 close it. The manifest
now also catches `>=`, a dropped check and a forgotten offset, and a reversed comparison survives
as the control.
