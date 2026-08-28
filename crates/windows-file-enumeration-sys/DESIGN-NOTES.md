# Design notes: windows-file-enumeration-sys (Tier 1)

This file is authoritative for the crate. Cross-component decisions are also
recorded in the workspace [DESIGN-NOTES.md](../../DESIGN-NOTES.md), historical
reasoning is in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md), and implementation
is scheduled by M5 and M6 in the workspace [CHECKLIST.md](../../CHECKLIST.md).

## Intent

Provide a memory-safe, asynchronous, flat one-directory enumeration layer over
documented Windows APIs. Preserve native path/name and metadata fidelity while
making submission, delivery, cancellation, and resource bounds explicit.

## Decision index

| ID | Decision |
|---|---|
| <a id="d-1"></a>D-1 | **The crate is an independently publishable flat-enumeration platform layer.** One request enumerates one directory; recursive traversal composes requests in a separate layer. |
| <a id="d-2"></a>D-2 | **The session uses a bounded multi-producer SQ and bounded single-receiver CQ.** Begin, cancellation, abandonment, and future controls enter through the SQ. Entries and exactly one terminal outcome leave through the CQ with an `EnumerationId`; reserved cancellation, abandonment, and terminal capacity make accepted control and terminal delivery lossless. |
| <a id="d-3"></a>D-3 | **The native engine uses `GetFileInformationByHandleEx` with caller-owned storage.** It uses `FileIdExtdDirectoryRestartInfo` followed by `FileIdExtdDirectoryInfo`, retains partially consumed buffers under CQ backpressure, and performs at most one synchronous refill per worker callback. Find-first/find-next, direct `Nt*` APIs, and IOCP integration are outside this design. |
| <a id="d-4"></a>D-4 | **Opening a directory uses the submitter's explicitly captured `ImpersonationToken`.** Ordinary begin captures before publishing its SQ message; an explicit-token form lets traversal reuse one captured context. Later refills use the already-open handle and do not impersonate. |
| <a id="d-5"></a>D-5 | **Native values remain native where they express the contract.** Paths and names use `wtf-string` for native-width WTF-16 storage, and Microsoft `windows-sys` value types remain public where no additional crate-owned invariant is required. |
| <a id="d-6"></a>D-6 | **Superseded by D-7 through D-15.** FE-2 closed the v1 public-contract questions that this scaffold deliberately left open. |
| <a id="d-7"></a>D-7 | **A request owns one NUL-free WTF-16 path snapshot with explicit long-path behavior.** Ordinary path forms are resolved when the request is built and must fit the ordinary Win32 limit; long paths must arrive as fully qualified `\\?\` inputs and remain verbatim. |
| <a id="d-8"></a>D-8 | **The CQ has only entry and terminal records, and preserves per-request native order without promising a stable sort.** A failed terminal owns its error; dot entries are never delivered. |
| <a id="d-9"></a>D-9 | **Every inline `FILE_ID_EXTD_DIR_INFO` field with defined consumer meaning is always returned in native units.** Undefined `FileIndex` is omitted; only the separately queried volume serial is selected. |
| <a id="d-10"></a>D-10 | **File identity distinguishes an always-present 128-bit file ID from optional volume qualification.** Requests select omitted, best-effort, or required volume qualification without per-entry opens. |
| <a id="d-11"></a>D-11 | **The v1 predicate is an extensible, data-only, flat conjunction of query-by-example clauses.** It owns native-name matching, six comparison operators, and non-vacuous attribute-mask semantics. |
| <a id="d-12"></a>D-12 | **Validation and admission failures are synchronous; accepted enumeration failures are ordered terminal outcomes.** Every native failure retains its raw Win32 code, and partial entries remain observable before a late failed terminal. |
| <a id="d-13"></a>D-13 | **Unsupported extended directory enumeration is a typed terminal failure, never a metadata-losing fallback.** The crate owns the classification while preserving the native code. |
| <a id="d-14"></a>D-14 | **The native buffer has one fallibly allocated, fixed effective capacity and an 8-byte-aligned base address.** It defaults to 64 KiB, clamps and aligns small requests, never grows, and reports a typed oversize-record terminal. |
| <a id="d-15"></a>D-15 | **Replacing Globazog's Windows one-directory backend is a release acceptance gate.** The adapter must retain all required metadata, native paths and names, predicates, errors, ordering, and no-per-entry-open performance. |
| <a id="d-16"></a>D-16 | **A worker reports; the submission-ring servicer is the sole registry authority.** A worker delivers entries and its own terminal to the completion ring, then reports retirement through the submission ring. It claims and releases its own enumeration but never removes a registry entry, and never releases a thread-pool object. |
| <a id="d-17"></a>D-17 | **One session-owned engine work object serves every enumeration, through a ready set.** No thread-pool object is stored per enumeration, so nothing the servicer or a worker drops can wait on a directory query. |
| <a id="d-18"></a>D-18 | **Each accepted enumeration reserves a retirement message as well as a cancellation.** Reporting oneself finished must be as infallible as being cancelled, which raises the minimum submission capacity to four. |
| <a id="d-19"></a>D-19 | **The native buffer is allocated at admission, fallibly, with an 8-byte-aligned base.** A request stays a cheap, clonable, comparable description; the buffer belongs to the enumeration it serves. |
| <a id="d-20"></a>D-20 | **A quantum's progress is bounded by both a record count and an elapsed-time budget; whichever is spent first ends it.** A quantum always examines at least one record regardless of either bound. A quantum parked for completion-ring room remembers that its retained record already needs delivery, so resuming re-checks room with one cheap call before reparsing it. |

## Control authority and worker lifetime

The submission-ring servicer is the only authority that mutates the registry.
A worker's outputs are completion records and one retirement report:

- accepted entries go to the completion ring, best-effort, under backpressure;
- the terminal outcome goes into the slot that enumeration reserved at
  admission, which the worker owns and which cannot fail; and
- retirement goes to the submission ring through a slot reserved at admission,
  after which the servicer removes the registry entry and releases the token,
  directory handle, native buffer, and remaining reservations.

A worker therefore never removes a registry entry, never releases a
thread-pool object, and never blocks another authority. This is not a stylistic
split. Letting a worker finish its own enumeration meant dropping that
enumeration's work object from inside that object's own callback, which
self-waits forever and then frees the closure that is still executing. Reporting
instead of acting removes the hazard rather than guarding against it.

For the same reason no thread-pool object lives in the registry. One
session-owned engine work object, created with a runs-long callback environment
and owned by the client-side handles, serves every enumeration; `schedule` adds
an enumeration to a ready set and submits that object, and each callback claims
one ready enumeration. A claim is single-flight: an enumeration already running
is never claimed twice, so one native buffer and record cursor are only ever
touched by one worker. Abandonment then releases registry entries that own no
thread-pool object at all, which is what lets receiver drop tear a session down
without waiting on a directory query.

## Quantum budgets and backpressure resumption

A quantum bounds its own progress two ways: a record count
(`MAX_RECORDS_PER_QUANTUM`) and an elapsed-time budget (`MAX_QUANTUM_DURATION`),
checked with a plain monotonic `Instant`. Either one being spent ends the
quantum with `Yielded`, resubmitting rather than running to the end of an
enormous batch or letting a predicate that rejects every record it sees
monopolise a worker. Every record a quantum looks at counts against the record
budget the same way -- a dropped `.` or `..`, one a predicate rejected, and one
delivered -- because the cost that matters here is examining the record at
all, not what became of it. Neither bound can stall an enumeration completely:
a quantum's first record is never gated by either one, so a budget this tight
still always makes some progress.

Completion-ring backpressure is a separate concern from either budget. A
quantum that cannot deliver an accepted entry parks with the cursor left at
that exact record, and remembers (`EngineState::awaiting_room`) that the
record waiting there is already known to need delivery. A quantum that resumes
while that flag is set asks the ring for room with one cheap call before
reparsing, rebuilding, and re-evaluating a predicate against a record whose
fate is already decided -- which is what keeps a sustained full ring from
paying that cost on every retry.

## Request path contract

`EnumerationRequest` owns its path as `Wtf16String`; the caller's source value
need not outlive construction or submission. Construction rejects an empty path
and any interior NUL synchronously because passing either to a NUL-terminated
Win32 API would name no directory or a different directory.

An input beginning with the Win32 `\\?\` prefix must be fully qualified, may use
the extended-length limit, and remains code-unit-for-code-unit verbatim. For
validation, `\\?\UNC\` must be followed by non-empty server and share
components. Every other `\\?\` form must contain a non-empty namespace-root
component followed by `\`; an ASCII drive designator must be exactly
`<letter>:` followed by `\`. These rules admit drive, volume-GUID, and other
absolute Win32 namespace roots while rejecting drive-relative `\\?\C:foo` and
rootless `\\?\name` forms. Later components remain verbatim: the crate does not
remove trailing separators or reinterpret `.` and `..`.

All other inputs, including `\\.\` device-namespace inputs, are resolved at
request construction with `GetFullPathNameW`, so later worker execution cannot
observe a changed process current directory. The returned fully qualified drive,
UNC, or `\\.\` device path is stored exactly as returned and retains ordinary
Win32 normalization semantics. To make behavior independent of the host
executable's `longPathAware` manifest, both the input and resolved form must fit
the ordinary `MAX_PATH` limit, including the trailing NUL. Longer paths are
rejected synchronously with guidance to provide a fully qualified `\\?\` path.
This also handles reserved DOS device names such as `NUL`, which
`GetFullPathNameW` resolves into `\\.\` form; opening or enumerating a
non-directory device then fails asynchronously rather than entering an
unspecified conversion branch.

Empty, interior-NUL, over-limit, non-fully-qualified `\\?\`, and resolution
failures are synchronous path errors. Existence, access, and directory-kind
failures occur asynchronously when the accepted request opens the stored path
under its captured token.

## Completion ordering and terminal outcomes

The public CQ record is exactly one of:

- `Entry { enumeration_id, entry }`; or
- `Terminal { enumeration_id, outcome }`.

`TerminalOutcome` is exactly `Completed`, `Cancelled`, or
`Failed(EnumerationError)`. There is no separate CQ error record, so the one
reserved terminal slot is sufficient even when the unreserved CQ data capacity
is full. Receiver abandonment intentionally emits no terminal.

`ERROR_NO_MORE_FILES` from either directory-information refill is a clean
end-of-directory signal and produces `Completed`. `ERROR_FILE_NOT_FOUND` from
the initial `FileIdExtdDirectoryRestartInfo` refill before any batch is likewise
the first-query-empty form and produces `Completed`. The same code from
`CreateFileW` remains a directory-open failure, and from a later refill remains a
query failure. Neither clean-exhaustion case becomes an `EnumerationError`.

The first-query-empty form is rarer than "an empty directory" suggests, which is
why the rule is stated in terms of the query rather than in terms of emptiness:
an empty *subdirectory* still contains `.` and `..`, so it returns a batch and
exhausts on its second query. Only a directory with no records at all -- an empty
volume root, for instance -- has nothing for its first query to return.

Directory-ness is established at the open, not inferred from a refill.
`FILE_LIST_DIRECTORY` is the same bit as `FILE_READ_DATA`, so opening an ordinary
file with it succeeds; the crate therefore checks `FILE_ATTRIBUTE_DIRECTORY` on
the opened handle and reports `DirectoryOpen(ERROR_DIRECTORY)`. Leaving it to the
first refill would surface "you named a file" through error codes that cannot be
distinguished from "this filesystem does not support extended directory
information", turning a caller's mistake into a reported capability failure.

For one `EnumerationId`, accepted entries retain the order in which the native
record chain supplies them and the terminal follows every queued entry. `.` and
`..` records are examined for work budgeting but are removed before predicate
evaluation and delivery. The native order is explicitly unspecified and may
change across filesystems, directory mutations, calls, or Windows versions; v1
does not sort. Records from different enumeration IDs may interleave according
to CQ enqueue order.

## Entry metadata and native units

Every delivered entry contains the native WTF-16 leaf name, file-versus-
directory type, raw `FILE_ATTRIBUTE_*` bits, optional reparse tag, logical size,
allocation size, extended-attribute size, creation time, last-access time,
last-write time, change time, and file identity. Reparse status is represented
by whether the tag is present and is derived from
`FILE_ATTRIBUTE_REPARSE_POINT`; the raw attribute bits remain available.
`FileIndex` is not exposed because Windows documents it as undefined for
filesystems including NTFS.

The four times use a crate-owned `WindowsFileTimestamp(i64)` newtype containing
the signed count of 100-nanosecond intervals since 1601-01-01 UTC exactly as the
directory record reports it. The crate performs no Unix-epoch conversion,
saturation, timezone conversion, or replacement of zero. Zero and negative
sentinel values therefore participate in predicate comparisons as their raw
signed values. Logical and allocation sizes retain their native byte unit but
are exposed as `u64`; a negative native size is a malformed-record failure. All
defined inline fields are always present because selecting them would not avoid
a native call or per-entry work.

`FileIdentity` contains the record's exact 16 identifier bytes plus
`Option<u64>` volume serial. The byte array is not converted to a numeric
endianness. A file ID is only volume-qualified when the serial is `Some`; callers
must not treat an unqualified ID as globally unique.

The only v1 metadata selection is `FileIdentityMode`:

- `Omit` performs no `FileIdInfo` query and returns unqualified IDs;
- `BestEffort` performs one directory-handle query and returns unqualified IDs
  if that query fails; and
- `Required` performs the same one query but fails the enumeration before its
  first entry if the serial cannot be obtained.

The default is `Omit`. Globazog selects `BestEffort` when its result plan needs
file identity, matching its existing unknown-identity behavior. No mode opens an
entry.

## Query-by-example predicate

`EntryPredicate` is a non-exhaustive enum whose v1 variant contains an owned
`QueryByExample`. A query is a sequence of `PredicateClause` values evaluated as
a flat conjunction with short-circuiting; an empty query matches every non-dot
entry. Multiple clauses for one field express ranges without a separate range
type. Contradictory clauses simply match nothing. The non-exhaustive outer enum
admits future predicate families without replacing the request API.

The v1 clauses cover:

- a native leaf-name pattern or membership in a set of patterns, each with
  explicit sensitive or insensitive ordinal comparison and optional negation;
- file or directory type, with optional negation;
- reparse-point status and reparse-tag equality, each independently testable;
- non-zero attribute masks requiring all bits set or all bits clear;
- logical size and allocation size comparisons; and
- comparisons over any of the four native timestamps.

`ComparisonOperator` is exactly `Less`, `LessOrEqual`, `Equal`, `NotEqual`,
`GreaterOrEqual`, or `Greater`; it compares the entry value on the left with the
query value on the right. Size values are bytes and timestamp values are
`WindowsFileTimestamp`. A zero all-set or all-clear mask is rejected when the
query is built because either condition would be vacuous.

Name patterns are compiled single-segment data, not strings interpreted by a
filesystem wildcard engine. Their token vocabulary is literal WTF-16,
zero-or-more code points, exactly one code point, and n-ary alternation of token
sequences. "One code point" means one Unicode scalar represented by a valid
surrogate pair or one preserved unpaired surrogate. Sensitive literal
comparison is exact; insensitive literal comparison uses Windows ordinal case
folding through `CompareStringOrdinal`. Wildcards never cross a path separator
because an enumerated leaf name has no path segments. An empty membership set is
rejected when the query is built because its positive form matches nothing and
its negated form is a vacuous match-all. This vocabulary translates Globazog's
existing single-segment predicate representation without lossy string
conversion.

## Error taxonomy and capability failures

Request construction reports typed synchronous errors for empty, interior-NUL,
over-limit, or non-fully-qualified extended paths; path-resolution failure;
invalid predicate data; and unrepresentable buffer capacity. Begin admission
reports capture failure, ordinary SQ saturation, cancellation-reservation
exhaustion, retirement-reservation exhaustion, terminal-reservation exhaustion,
and native-buffer allocation failure synchronously, as well as refusing a
session whose receiver has abandoned it. A rejected begin preserves the request
and any explicit token for retry. No rejected operation receives an
`EnumerationId` or terminal.

After acceptance, `EnumerationError` distinguishes impersonation application,
directory open, required volume identity, unsupported extended directory
information, other directory query, oversize record, and malformed native record
failures. Native variants retain the original Win32 code; impersonation retains
the sibling crate's typed source. Malformed-record detail distinguishes invalid
alignment, truncated fixed fields, invalid next offset, odd name byte length,
out-of-bounds name, and negative size. A late query or parse failure does not
retract entries already queued: those entries are followed by exactly one
`Failed` terminal.

On an extended-directory query made with a live handle this crate opened *and
verified to be a directory*, a valid information class, a non-null
8-byte-aligned buffer base, and an
effective capacity of at least 1 KiB that is both an 8-byte multiple and
representable as `u32`, `ERROR_INVALID_FUNCTION`, `ERROR_NOT_SUPPORTED`, and
`ERROR_INVALID_PARAMETER` are classified as
`UnsupportedExtendedDirectoryInfo`. The classification is crate-owned and the
raw code remains available. The crate does not silently switch to find-first/
find-next because that would remove change time, allocation size, extended-
attribute size, and the 128-bit file ID from the promised level platform.

## Fixed native buffer and oversize records

The requested buffer capacity defaults to 64 KiB. Values below 1 KiB are clamped
to 1 KiB, then the effective capacity is rounded up to an 8-byte multiple. A
value whose aligned result cannot be passed as a Win32 `u32` is rejected when the
request is built.

The buffer itself is allocated at admission, one per accepted enumeration, and
completes before the begin is accepted. It does not belong to the request: a
request is a cheap, clonable, comparable description that may be submitted more
than once and is handed back intact when a begin is refused, none of which
survives owning a 64 KiB scratch buffer. The allocation must be fallible rather
than aborting, and must produce a base address aligned to at least 8 bytes so
the record's `i64` fields are never read misaligned -- neither of which the
ordinary growable-vector allocation path provides on its own.

The request uses that one effective capacity for its lifetime. It neither grows
nor retries with a different capacity. If a directory refill reports
`ERROR_MORE_DATA`, `ERROR_INSUFFICIENT_BUFFER`, or `ERROR_BAD_LENGTH` before
returning one complete record, the enumeration terminates with
`RecordTooLarge`, carrying the effective capacity and raw Win32 code. Other
errors remain ordinary query failures. No bytes from a failed refill are parsed.
This deterministic contract avoids depending on undocumented cursor advancement
after a failed refill and keeps the configured memory bound real.

## Globazog replacement gate

Before publication, an adapter must demonstrate that Globazog can obtain native
names, file/directory type, reparse status and tag, raw attributes, logical size,
all four times, and volume-qualified 128-bit identity when requested, without
per-entry opens. Its name, type, reparse, attribute, size, and timestamp
predicate leaves must translate without loss. Native order remains native and
unspecified, dot entries remain absent, and a failed terminal retains enough
detail for the adapter to reproduce Globazog's error-plus-failed-terminal
surface. The asynchronous replacement may improve bounded backpressure and
impersonation correctness, but it may not narrow metadata or predicate
capability.

## Publication boundary

The crate is Windows-only, independently versioned, and published to crates.io.
It depends on `windows-impersonation-token-sys`, `windows-threadpool-sys`, and
`wtf-string` by both path and version. Its direct `windows-sys` surface enables
Foundation and Storage FileSystem for enumeration, Globalization for
`CompareStringOrdinal`, and System Threading for the completion-ring event
doorbell. Release automation is registered at the workspace level.
