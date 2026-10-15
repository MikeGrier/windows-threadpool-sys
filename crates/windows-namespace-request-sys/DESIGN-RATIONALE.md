# Design rationale: windows-namespace-request-sys

Tier 2. How the decisions in [DESIGN-NOTES.md](DESIGN-NOTES.md) were reached --
alternatives considered, drafts that were wrong, and the reasoning that got
discarded along the way. Consulted for "why", never for "what is true now": if
this file and Tier 1 disagree, **Tier 1 wins**.

Cross-referenced by decision ID.

## D-18: `GetFullPathNameW` is not lexical

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-18`. What follows is
the record of getting there, which is unusually worth keeping because the same
mistake recurred in nine successive wordings, enumerated below.

### The shape of the error, which never changed

Every wrong draft did the same thing: **named a mechanism the evidence did not
reach.** The subject moved -- what the call does, what a number measures, which
alternatives cost less -- but the failure did not. That is why the decision
records the constraint rather than only the conclusion: a reader who takes only
"it is not lexical" away from D-18 has the answer without the thing that keeps
producing wrong answers.

### The drafts

1. **"This call is lexical."** The original text, contradicted by its own next
   sentence, which said it resolves against the process current directory. A
   lexical canonicalizer is a pure function of its input; this reads mutable
   process state.

2. **"It resolves relative components and `.`/`..` against the process current
   directory."** The first correction, which overshot. Collapsing `.`/`..` has
   an output that is a function of the input alone -- `C:\a\..\b` becomes
   `C:\b` under any current directory, and whether or not `C:\a` exists.
   Measured under two different current directories.

   **This entry itself carried the error it describes, for twenty-three
   rounds.** It read "pure string work and reads no process state at all", and
   two different current directories giving the same answer shows the output
   does not depend on that state, not that nothing was read. Every correction in
   this file was aimed at the *rooting* half; the overreach in the **lexical**
   half was the sentence doing the correcting, which is why nobody looked at it.
   Whether rooting reads process state is a separate question, and it is
   answered observably: changing the current directory changes the result.

3. **"The probe measures roughly 212 ns per resolution."** It does not. The
   probe reports a construct-and-drop cycle whose total contains an allocation
   and a drop, and its own output says so.

4. **"The probe deliberately declines to decompose that total."** An
   over-correction of (3). It does decompose: the report states that recycling
   an already-resolved path pays ~42 ns against ~210 ns and attributes the
   difference to the resolution. What the probe withholds is the **mechanism** --
   whether any of it is a kernel transition -- not the division.

5. **"Roughly 168 ns is this call's measured share."** Taking (4)'s split at face
   value. The subtraction spans the whole preparation step, which makes two heap
   allocations of the crate's own against the clone's one, plus a builder chain.
   Timed directly with no allocation in the loop, the call is about 110 ns --
   the draft overstated it by roughly half.

6. **"Either alternative is cheaper."** Said of `PathCchCanonicalizeEx` and
   `PathAllocCanonicalize` from the first commit onwards, and never measured.
   Nothing in this repository benchmarks them, Microsoft documents behaviour
   rather than relative cost, and `PathAllocCanonicalize` allocates its own
   result. Removed rather than substantiated, because the decision rests on
   rooting semantics and never needed it.

7. **"Touches no filesystem."** The claim that survived longest, because it is
   nearly right. What Microsoft documents is that the function does not verify
   that the resulting path is valid or names an existing file -- a statement
   about *verification*, not about I/O.

8. **"Observation cannot close that gap."** The correction to (7), and wrong in
   a more interesting way: observation *did* close it, in the affirmative
   direction, the moment anyone looked at the right form. Resolving `X:foo` for
   a non-current drive distinguishes an existing directory from an existing
   *file* from a missing one, and rewrites the `=X:` entry when it is not a
   directory. That is a filesystem query and a write to the process environment,
   in a call four drafts had described as reading process memory.

   The reasoning behind those drafts was sound -- the current directory is in
   the PEB, the `=X:` variables are in the environment block, both are process
   memory -- and the conclusion was false. It is the cleanest example in this
   file of the standing failure: a mechanism argued from where the data lives
   rather than measured. Every earlier draft at least *knew* it was asserting a
   mechanism; this one thought it was declining to.

9. **"A filesystem query rather than a syntax test."** Written in the paragraph
   correcting (8), and wrong the same way within a single round. Having found
   that an entry naming a missing directory or an existing *file* is rejected,
   the draft concluded the gate was existence and not shape. Measured, shape
   gates it too and independently: `C:/Windows/System32`,
   `C:\Windows\System32\.`, `C:\Windows\System32\..\System32` and
   `\\?\C:\Windows\System32` are each rejected while naming the same existing
   directory that `C:\Windows\System32` is accepted for. Acceptance is also
   literal -- `C:\Windows\` yields `C:\Windows\\foo`, with no normalisation at
   the join.

   The tell was the word *rather*. Ruling an alternative out is a strictly
   stronger claim than establishing the one you measured, and needs its own
   evidence; three observations of the existence check said nothing about
   shape. Both halves are now pinned by
   `a_rejected_drive_entry_is_replaced_by_the_drive_root` so the next draft
   cannot restate this from memory.

### Two facts that were measured, then asserted too narrowly

Both were found by review after the correction had already shipped, and both
were *enumerations* -- which is the form this kind of error likes.

- **The rooting forms.** Stated first as one case (the current directory), then
  two, and finally three: a relative path takes the current directory, a
  root-relative path takes only that directory's *root* (which is
  `\\server\share\` under a UNC current directory, so "current drive" was
  wrong), and a drive-relative path naming a drive OTHER than the current one
  takes the entry recorded for that drive from `=C:`, while on the current drive
  that entry makes no difference and the process directory wins. The
  current-drive arm is a fourth correction to the same enumeration, found the
  same way as the first three and after them: the count "three forms" was itself
  one of the things stated more confidently than measured.

- **The device set.** The short-circuit was first described as "exact-match
  only", which `CON:` disproves; then enumerated as `CON`/`NUL`/`PRN`/`AUX`/
  `COM1`-`9`/`LPT1`-`9`/`CONIN$`/`CONOUT$`, which omits the superscript
  spellings `COM\u{00b9}`, `COM\u{00b2}`, `COM\u{00b3}` and their `LPT`
  equivalents. Those are exactly the members a hand-written denylist misses, and
  the documentation asserted a closed list without them until a review measured
  it. The tests in [tests.rs](src/full_path/tests.rs) now pin every documented
  spelling so the next omission fails CI instead of a review -- each positive
  spelling against the device path it produces, and each negative one against
  its full rooted result.

  **A third correction, and the one that shows why "the device set" was the
  wrong unit all along.** A review asked for the documented `\CON` case to be
  pinned, since the doc stated it and no test covered it. Writing that
  assertion as the general rule the request was phrased in -- "a root-relative
  device word roots at the current directory's root" -- would have pinned a
  false claim. Measured across all eight accepted names, `NUL` alone
  short-circuits as the final component of ANY path: `\NUL`, `.\NUL`, `a\NUL`
  and `C:\NUL` all reach `\\.\NUL`, while every `CON`, `PRN`, `AUX`, `CONIN$`,
  `CONOUT$`, `COM1` and `LPT1` spelling roots. So a fully qualified path can
  still name a device.

  Every earlier entry here is a claim generalised from too few observations.
  This one was generalised from one MEMBER to a set, which is the same error in
  a dimension nobody had checked -- the enumeration of the set was audited
  twice, and the assumption that its members behave alike never was.

  That distinction between kinds of pinning was itself a later correction, and
  it is the reason the sentence now says which KIND of pinning each case gets. The claim "pins every
  documented spelling" was written while one negative spelling, `CON:x`, was
  covered only by `!starts_with("\\.\\")` -- a predicate the unrooted literal
  satisfies. So the claim was true of the list and false of the strength, which
  is the same shape as the trimming generalisation two entries below.

### The measurement that was itself unmeasured

The sharpest instance in this whole sequence is not about `GetFullPathNameW` at
all. The test helper that reads a `=X:` entry folded "empty value" and "absent
name" into one answer, and said so in a comment that called the equivalence
*measured*: `SetEnvironmentVariableW(name, "")` was reported to succeed and then
read back exactly as a name that was never set.

It does not. `GetEnvironmentVariableW` returns `0` for both, and the last error
is the only thing that separates them -- so a measurement that never cleared the
last error first could read nothing but whatever an earlier call had left there.
Cleared and re-measured, the two are distinct, for an ordinary name and an `=X:`
name alike:

| state | return | last error |
|---|---|---|
| set to `""` | `0` | `ERROR_SUCCESS` |
| deleted | `0` | `ERROR_ENVVAR_NOT_FOUND` |

An earlier note also recorded that `ERROR_ENVVAR_NOT_FOUND` "never surfaced even
for genuinely absent variables", which has the same cause and one more: the
deletion under test had not happened, because the null that deletes an entry had
been marshalled as an empty string instead. Two layers of the harness agreeing
with each other is not a measurement.

The consequence was live rather than cosmetic. `BorrowedDriveEntry` restores a
borrowed entry on unwind precisely so a panicking test cannot leak process
state; with the two answers collapsed, restoring an inherited *empty* entry
**deleted** it. The guard destroyed the state it existed to preserve, in exactly
one case, and only that case.

Pinned by `an_empty_drive_entry_is_distinguished_from_an_absent_one` in
[drive_entry.rs](src/full_path/tests/drive_entry.rs), and verified by re-introducing the collapse,
which makes it fail.

The general form is worth keeping separately from the specific fact: **a
comment that says "measured" is a claim about a procedure, and the procedure can
be wrong in ways the result never reveals.** Every other entry in this file is an
assertion that outran its evidence. This one had evidence, and the evidence was
of something else.

### The sweep, and its arithmetic

The wrong word had spread well beyond where it was reported. The consuming
probe's checklist item named [full_path.rs](src/full_path.rs) only; that file
held **three of the nine** on its own -- the module doc and both doc examples --
with two more in [path.rs](src/path.rs), two in
[tests.rs](src/full_path/tests.rs), one in an acceptance comment and one in
[DESIGN-NOTES.md](DESIGN-NOTES.md). Nine across five files, from a report naming
one. The reported site was a sample, not the population.

### Why the alternatives were never seriously in contention

`PathCchCanonicalizeEx` and `PathAllocCanonicalize` canonicalize without
rooting, which sounds like a strictly smaller job and therefore an easy win. It
is the wrong trade for this crate: a path that is still relative has its meaning
settled on the worker thread at execution time, against a current directory any
thread may have changed in between. That is precisely the race preparation
exists to close, so the "smaller job" omits the part being bought.

Recorded here rather than only in Tier 1 so the next reader who notices the
cheaper-looking API does not have to re-derive why it was declined.

## D-19: grafting a relative path

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-19`. This is the record of how it was
reached, with the measurements behind it. They were taken on 2026-10-15 on Windows 11 with a probe
test that was not kept (it failed on purpose so libtest would replay its output), the process
current directory set to `C:\Windows\System32`, the long-path flags below not including a
long-path-aware host.

**How the direction arrived.** The engineer asked what `prepare` did that was not Win32's own, said
a path should reach Win32 untouched with no length limit and no checking of its own, and then, once
"resolve at build time" was read as "when the object is built" and not at compile time, held that the
need to capture the root at construction remained, with relative paths resolved textually against it
later. Windows provides a lexical join, `PathAllocCombine`, which [D-18](DESIGN-NOTES.md#d-18) had not
weighed (it weighed only the canonicalisers), so it was measured.

**`PathAllocCombine` against `GetFullPathNameW`** (base = the process current directory):

| Input | `PathAllocCombine`, flags 0 | `GetFullPathNameW` |
|---|---|---|
| `a\b`, `.\a`, `..\a`, `a\..\b`, `..\..\..\..\x` | the same | the same |
| `\a\b` | `C:\a\b` | `C:\a\b` |
| `a.`, `a ` | `...\a` | `...\a` |
| `a/b` | `...\a/b` (slash kept) | `...\a\b` |
| `C:a`, `C:a\b`, `Q:a` | `C:a`, `C:a\b`, `Q:a` (unresolved) | resolved |
| `C:` | `C:\` | `C:\Windows\System32` |
| `C:\x\..\y` | `C:\y` | `C:\y` |
| `C:/x/y` | `C:/x/y` | `C:\x\y` |
| `\\?\C:\x\.\y` | `C:\x\y` (prefix dropped, `.` collapsed) | `\\?\C:\x\y` |
| `\\.\PhysicalDrive0` | `\\PhysicalDrive0` | `\\.\PhysicalDrive0` |
| `\\.\C:\x\..\y` | `\\C:\y` | `\\.\C:\y` |
| `CON`, `NUL`, `COM1` | `...\System32\CON` and so on | `\\.\CON` and so on |
| empty | the base | error 87 |

With a UNC base `\\server\share\dir\sub`: `a\b` and `\a\b` join correctly, and `..\..\..\x` gives
`\\server\x`, past the share root. With a verbatim base `\\?\C:\work\proj` the result loses its
prefix. Long results (a base of about 350 units): flags 0 fail with `0x800700ce`;
`PATHCCH_ALLOW_LONG_PATHS` gives a `\\?\` path; adding
`PATHCCH_FORCE_ENABLE_LONG_NAME_PROCESS` gives a plain long path. `PATHCCH_DO_NOT_NORMALIZE_SEGMENTS`
keeps a trailing dot or space that the default strips.

**`CreateFileW` on a device name, spelled four ways** (open for query; the number is the Win32 error,
or the file type):

| Name | bare | joined onto the directory | `x\name`, `x` missing | `drivers\name`, `drivers` exists |
|---|---|---|---|---|
| `NUL` | char device | char device | 3 | char device |
| `CON`, `con` | 87 | 2 | 3 | 2 |
| `CONIN$`, `CONOUT$` | char device | 2 | 3 | 2 |
| `PRN`, `AUX`, `COM1`, `COM2`, `LPT1` | 2 | 2 | 3 | 2 |
| `NUL.txt` | 2 | 2 | 3 | 2 |

So `NUL` is a device in any position and a join is faithful to it, where `GetFullPathNameW`'s habit of
resolving `a\NUL` to `\\.\NUL` is not what `CreateFileW` does with `a\NUL` (it fails with error 3 when
`a` is missing). `CON`, `CONIN$` and `CONOUT$` change meaning under a join, which is rule 5 of the
decision. `PRN`, `AUX`, `COM1`, `COM2` and `LPT1` fail the same way in every spelling here, so nothing
is shown about them either way.

**What was weighed and set aside.** A combine call with the long-path flags, for the reasons in the
decision. A classification that grafts everything but the forms known to be wrong, which would make
the list of wrong forms the policy; the engineer's rule was that the graft is allowed "when we can
show that the child path is clearly safe", so the rule names what is safe and lets the rest through
untouched.

## D-20: one overlapped-safe `DeviceIoControl` helper

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-20`. This is how it was reached.

**The problem was visible before any code.** The crate's handle-taking entries (`QueryVolumeInformation`
and the rest) call functions that take no `OVERLAPPED`, so how a handle was opened never mattered.
`DeviceIoControl` is the first call that does take one, and the same crate that cannot see how a
handle was opened is the one that has to call it.

**What was weighed.**

- **Ask the handle how it was opened.** The only route to the overlapped flag is an `Nt*` query,
  which the repository's DOCUMENTED APIS ONLY rule forbids, and
  `GetFileInformationByHandleEx` does not report it.
- **Pass a null `OVERLAPPED`.** Documented to fail unpredictably on an overlapped handle, and
  unpredictable is not an outcome a catalogue entry can report.
- **Associate a port of the helper's own, or change the handle's completion-notification mode.**
  Either changes state on a handle the caller owns and shares; a request borrows a duplicate, not
  the right to configure it.
- **Give every call an event and wait for it.** What was chosen. It is the same call on both kinds of
  handle, and it costs an event creation per call, which is the price of not knowing.
- **The event's low bit.** Without it a handle on a completion port gets a packet when the call
  completes, which reaches a consumer's loop as a completion it did not ask for. A sabotage that
  removes the bit is caught by a test that associates the handle with a port and checks nothing
  is queued after the call.

**Why the tests use two controls.** `FSCTL_QUERY_VOLUME_NUMA_INFO` is the entry's own control and the
one whose buffer-size failure is documented, but a volume may have no answer, so tests on it stand
down when there is none. The first sabotage sweep showed what that costs: telling the helper the
buffer was empty turned every call into a failure, and every test that tolerated "no answer" passed.
`FSCTL_GET_COMPRESSION` is answered by every file on an NTFS or ReFS volume, so the helper's tests
assert success on it and do not stand down; that closed the hole.

**How the pending path came to be pinned.** A named-pipe listen (`FSCTL_PIPE_LISTEN`) was the obvious
control that pends until a client connects. It failed with `ERROR_INVALID_FUNCTION`, so it was removed
rather than worked around. The first sabotage entries for the pending path were then caught only
because the full suite happened to make a call pend, and one survived for the same reason; a throwaway
test that counted `ERROR_IO_PENDING` over 20000 calls with the pending branch disabled showed 0, so
those entries were removed as neither a control nor a declared blind spot. The engineer's question
("I guess add a sabotage for it?") then led to the seam: split the helper at the one call that makes it a
control, and drive the rest with a call that does pend on demand.

**What was weighed for that call.** A fake that sets `OVERLAPPED.Internal` and `InternalHigh` and
signals the event by hand would pend deterministically too, but it relies on fields the documentation
reserves for the system and on how `GetOverlappedResult` reads them, which is the incidental-behaviour
binding the platform rules forbid. An overlapped `ReadFile` on a pipe uses only documented behaviour.

**A defect the seam work found in the tests.** The helper's tests opened handles without taking the read
side of `handle_allocation`, so on roughly one run in thirty another test, which holds the write side
while it reasons about a closed handle's value, saw a handle reused and panicked, poisoning the lock for
every test that follows. Measured as 1 mass failure in 60 runs before the fix and none in 200 after.
Separately, two `watch` tests shared the fixture label `watch-defaults` and removed each other's
directory, failing 8 of 30 runs; each now has its own.

**What this did not settle at the time.** `DeviceIoControl`'s own behaviour when it pends was unchecked, for
want of a control that does so on demand. `FSCTL_REQUEST_OPLOCK` is one, found in `NR-2.2`
([D-21](#d-21)'s rationale below).

## D-21: the disk-extents entry

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-21`. This is how it was reached.

**A premise in the checklist was wrong.** `NR-2.2` was written as "first, give the helper an input buffer, since
this item and the next two need one". This control takes none; `NR-2.3` and `NR-2.4`
(`IOCTL_STORAGE_QUERY_PROPERTY`) do. The input buffer was done under `NR-2.2` anyway, because it had been
scheduled there and because the one control found that pends on demand, `FSCTL_REQUEST_OPLOCK`, takes input, so
the two were the same piece of work; but the entry itself did not need it.

**Finding the control that pends.** The candidate was `FSCTL_REQUEST_OPLOCK`. A throwaway test requested a read
oplock on an overlapped handle and had another thread open the file for write: the call pended and did not
return in 25 seconds, so it was killed by a timer outside the test (an in-test wait would have hung the run). The
same test with the other thread writing a byte completed in 304 ms against a 300 ms delay, with the oplock's
original level `1` and new level `0`. So a write-open does not break a read oplock and a write does, and the
control pends exactly as long as the test says. The real test uses that, and the same request with no input,
which the device refuses at once, is what shows the input is what made it work.

**Why a `Vec<DiskExtent>` of `u64`s and not the raw reply.** A consumer given the bytes would have to know the
layout, the `ERROR_MORE_DATA` convention and where the count is, which is what an entry exists to carry. `u64`
is what an offset and a length in bytes are; the structure's `LARGE_INTEGER` is signed, and a negative value is
not a length, so it is reported as malformed and not cast. `i64`s that a caller then casts would put the same
check in every caller. `AlignedBuf` was not used: the reply is read with `from_ne_bytes`, so no alignment is
needed, and a `Vec<u8>` allocated with `try_reserve_exact` fails as an error the caller receives.

**The fake device had the wrong code, and the real one showed it.** The documentation suggests
`ERROR_INSUFFICIENT_BUFFER` for a buffer smaller than the header, and the first fake and the first real-device
test both said so. The real-device test failed (87, not 122), a sweep of every size from 0 to 48 found the
boundary (below the count plus one extent: 87; from it up: the answer), and the fake was corrected to match. The
fake is the only way to test the multi-extent half on this machine, so its one measured behaviour is stated where
it is built, and the documented half (`ERROR_MORE_DATA` with the count) is stated as not observed.

**What was weighed and set aside.** Sharing `win-volume-sys`'s `extents.rs` instead of writing this: that crate
is a working, released one, and the rule for speculative duplicate work is to build beside it and decide when the
new one is proven, so the decision is `NR-2.6`. Capping the count the device reports, to refuse a huge
allocation: a cap is a number this crate would have to choose and defend, the count comes from the volume driver,
and the failure is already an error the caller receives; not decided against for ever, but nothing shows it is
needed.

## D-22: storage properties

The decision is in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> `D-22`. This is how it was reached.

**The `NR-2.3` "check first" was the first question, and it was cheap.** The checklist said to check that an
`IOCTL_STORAGE_*` code behaves through the helper before building on it, because `FSCTL_PIPE_LISTEN` had not.
A throwaway test opened the disk with no access and asked for the property with output buffers of 0, 4, 7, 8,
16, 20, 24, 64 and 4096 bytes and with inputs shorter than the structure. That one run gave the whole size
protocol (7: bad length; 8: the header alone; 9 up: more data with the header written; 4096: the reply) and
the reply's shape, and a second probe printing only numeric fields showed the descriptor's own size equals what
remains of the reply. Nothing in the reply was recorded beyond those facts: it holds the device's identifiers,
which identify the machine.

**What was weighed for the reply type.**

- *Parsed, with typed identifiers inside the descriptors.* Declined for now, which the engineer confirmed
  ("at this time, no. We can revisit later if necessary"), for the gradient reason: which identifiers a key
  uses is the locality helper's question, which has not been asked yet, and a typed surface here would be an
  answer to it.
- *Raw bytes of the whole reply only.* A consumer would redo the offset checks every time, and the checks are
  where a damaged reply becomes an out-of-bounds read; so the sections are delimited and bounds-checked here and
  handed back as slices.
- *One generic entry that takes a property id and returns bytes.* Raised with the engineer, who said yes; built
  as `QueryStorageProperty` (below). Nothing argued it had no value, and it lets a consumer reach a property
  this crate has no entry for.

**Why the zeroed-buffer and `u32` reader moved.** The second entry to size a buffer from a reply needed the
same two helpers `volume_extents` had written privately; the mono-repo rule is to fix the layer, so they are now
in the helper's module and `volume_extents` uses them. Its sabotage entry for the allocation moved with the
function.

**The sweep and the redundant check.** The first sweep caught all twenty-one entries, but only after a pass over
the survivors a mutant could hide behind: a length guard in `decode` was refused anyway by the first offset read
that fell outside the header, so mutating it changed no outcome, and it was deleted rather than defended; and the
tests for a call that claims more bytes than it was given room for would have passed with the claim check
removed, because the header the fake wrote was empty, so they now write a valid one, and the test for a reply
shorter than its header asserts the number of calls, which is what distinguishes refusing it from asking again.

**A name collision, and what it showed.** The oplock test in `device_control` had its own `u32_at` that panicked
on a short buffer; the shared one returns `None`. The test now uses the shared one, so the one reader every entry
relies on is the one the earliest tests exercise.

## D-22 addendum: the generic storage property query

**What a generic entry exposes that a typed one hid.** A throwaway probe asked the disk about the cases the
unique-id entry never reached. An exists query on a real property succeeds with nothing written; an unknown
property fails (`ERROR_INVALID_FUNCTION`) for exists and standard alike; a mask query and an undefined query
type fail with `ERROR_NOT_SUPPORTED`. Those shaped the entry: an exists query cannot go through the
learn-the-size loop, which would refuse an empty reply as shorter than its header, so it is its own single call.

**A wrong conclusion that a second probe corrected.** The first probe sent additional parameters of zero bytes
and got the same reply for every length, and the module documentation was written to say a property that takes
none accepts and ignores them (measured). The test written from it failed on a nonzero byte: the unique-id
property refuses 0xA5 with `ERROR_INVALID_PARAMETER`. A second probe over values and lengths showed the first
additional byte is read by that property (0 gives 112 bytes, 1 gives 140) and ignored by the device property.
The claim was withdrawn from both modules before anything was committed, the test now covers both properties,
and the byte's effect on the answer is what shows additional parameters arriving at a real device. The cause was
a probe whose input could not distinguish "ignored" from "zero"; it is recorded because the same shape of probe
would have been trusted again.

**A lead, not an interpretation.** The larger reply for a first byte of 1 may carry more identifiers than the
usual one. The typed entry sends no parameters, so it does not ask. The generic entry is how a consumer gets the
larger reply, and what the byte means is not recorded as anything but an observation. If the locality helper
needs those identifiers, that is the moment to ask what the byte selects and whether the typed entry should
offer it.

**Why `Arc<[u8]>` for the parameters.** A request's `try_clone` returns a handle-capture error and has no
allocation-failure case, and the crate's rule is that an allocation that fails is an error the caller receives.
Sharing the parameters makes the copy allocation-free, so the rule holds without a new error variant. The
input that is sent is built fallibly (`zeroed_output`), at `perform`, where an error can be returned.

**Newtypes over the raw values.** Both the property id and the query type are `i32` in Windows, and a call that
swaps them compiles and asks a different question. Newtypes make the swap a compile error, and `from_raw` keeps
every value Windows may define later reachable, so the types narrow nothing. Only the query type has named
constants, because the three documented values are all that enumeration has; the property ids are `windows-sys`'
and are not reproduced.
