# Design rationale: captured impersonation and asynchronous file enumeration (Tier 2)

This file records why the cross-component decisions in
[DESIGN-NOTES.md](DESIGN-NOTES.md) were reached. The raw discussion is in
[design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md](design-sessions/DESIGN-SESSION-2026-08-27-async-file-enumeration.md).
Tier 1 remains authoritative.

## Probe release automation

For [DESIGN-NOTES.md](DESIGN-NOTES.md#d-probe-releases) -> `D-PB`.

The placement workflow already built and attested release executables, but
release-please did not manage its package. Platform probes had neither a managed
release route nor binary packaging. The earlier "never distributed" decision
would require every outside measurement to begin with a source checkout.

The engineer chose calendar versions for both probes. Replacing placement's
version with semver would change a deliberate build-identity convention. A
pinned release-please wrapper provides that version policy while keeping the
existing Rust release implementation. The Cargo workspace plugin has its own
dependency-only bump path, so extending only the direct version strategy would
leave probe versions subject to ordinary patch bumps on that path.

A metadata-derived archive avoids a second list of executable names. Checking
both the target list and emitted artifact list detects omitted builds as well
as extra binaries; checking PE headers also observes the architecture of the
bytes that will be attached. Packaging does not execute the platform experiments:
some mutate process-wide state or deliberately wait indefinitely.

The placement workflow's existing publication guards apply to the new route
too. GitHub attestations authenticate the artifact bytes; environment-derived
build labels do not. Local tests exercise real Rust release updates and ZIP
round trips; hosted token permissions and attestation issuance are exercised
only by the eventual release workflow.

## Why captured impersonation is its own crate

The enumeration open must occur asynchronously, but directory access is determined
by the submitter's effective security context. A thread-pool worker cannot recover
that context later: modern `SubmitThreadpoolWork` dispatch does not flow the
submitter's thread token.

Windows demonstrates that the capability is legitimate rather than application
policy. Legacy `QueueUserWorkItem` has `WT_TRANSFER_IMPERSONATION`, which causes the
callback to use the submitting thread's current process or impersonation token. The
modern object-based thread-pool API has no corresponding callback-environment flag,
so users of the modern API must build the transfer themselves.

Microsoft WIL supplies useful C++ precedent. Its
`open_current_access_token_nothrow` opens the thread token or falls back to the
process token, and `impersonate_token_nothrow` saves the current thread token,
temporarily applies another, and restores the exact saved state. Its restoration
path fails fast. The Microsoft Rust crates expose the Win32 calls and types but do
not compose them into a transportable safe abstraction.

The third-party `windows-token` crate was considered and rejected. At the time of
this design it was new and pre-alpha in its own documentation, did not implement
`OpenThreadToken`, restored with `RevertToSelf` rather than restoring a prior nested
impersonation token, and swallowed restoration failure in release builds. Those are
the exact guarantees this workspace needs.

Putting the helper directly in `windows-file-enumeration-sys` would make traversal
either depend on an enumeration implementation for an orthogonal security
primitive or reproduce the same sensitive code. Putting it in
`windows-threadpool-sys` would add security-token dependencies and policy to the
level thread-pool layer even though token transport is useful across dispatch
mechanisms. A separate `windows-impersonation-token-sys` crate preserves
both boundaries.

The future traversal layer creates a second concrete need. It must capture once
when traversal is submitted and reuse that context when it schedules descendant
directory enumerations later. Capturing separately inside each enumeration helper
would capture arbitrary workers and reintroduce the Globazog bug.

## Why the public token type is crate-owned

The workspace normally reuses Microsoft native types, but a `HANDLE` is only a raw
identifier. It does not express ownership, token rights, mutability restrictions,
cross-thread transport, capture timing, scoped application, exact restoration, or
what happens when restoration fails. `ImpersonationToken` exists to own those
additional invariants; internally it remains an implementation over
`windows-sys`.

The use-site API should make the safe path easy. An enumeration SQ offers an
ordinary begin helper that captures the current context and publishes the
token-bearing begin message as one operation. It also offers an explicit-token form
for traversal and other orchestrators that need to reuse a previously captured
context.

## Why restoration failure panics

Failure to restore an arbitrary application thread would already be serious.
Failure on a shared Windows thread-pool worker is worse: later unrelated callbacks
could execute under the wrong identity. Returning an error to the enumeration
consumer does not repair that worker, and swallowing the failure turns a known
security breach into process-global nondeterminism. The guard therefore panics
from `Drop` when `SetThreadToken` fails. Restoration still runs during Rust unwind;
if restoration itself then panics, Rust's double-panic behavior aborts the process.

## Why enumeration uses a bounded SQ and CQ

The session needs asynchronous communication in both directions without calling
client code from the cadence path. The SQ gives every begin, cancellation, and
abandonment operation one ordered ingress path. The CQ gives entries and terminal
outcomes one ordered egress path. `ThreadpoolWork` acts only as a coalesced SQ
doorbell and drain authority; it is not an enumeration worker.

Bounded rings make resource use explicit, but cancellation cannot be allowed to
fail when ordinary traffic fills the SQ. Every accepted enumeration therefore owns
a reserved future cancellation slot, and the session owns one abandonment
reservation. The CQ similarly reserves each accepted enumeration's terminal slot,
while retaining at least one unreserved data slot so reservations cannot deadlock
all useful progress.

## Why `GetFileInformationByHandleEx` replaces find-first/find-next

Globazog already uses `GetFileInformationByHandleEx` with
`FileIdExtdDirectoryRestartInfo` and `FileIdExtdDirectoryInfo`. The caller-owned
buffer exposes the refill boundary, retains wider metadata than
`WIN32_FIND_DATAW`, and can be held across callbacks while a bounded CQ is full.
That directly solves the hidden-buffer problem in `FindNextFileW`.

The API is synchronous and has no documented overlapped, APC, event, completion
routine, or IOCP form. A potentially-long-running `ThreadpoolWork` callback is
therefore the documented Windows-native bridge. Limiting each callback to one
refill and finite parsing work prevents an enumeration from monopolizing workers
without reaching into unsupported `Nt*` APIs.

## Why the enumeration surface resolves paths and preserves native order

Resolving ordinary paths before SQ acceptance snapshots relative-path meaning
without opening under the submitter thread. Opening the resulting ordinary path
can depend on executable and system long-path policy, so the crate deliberately
caps ordinary forms at `MAX_PATH`; callers use a fully qualified, verbatim
`\\?\` path for long input. `\\.\` retains ordinary Win32 normalization and is
snapshotted like the other non-verbatim forms.

The native API supplies no stable ordering contract, and sorting would require a
whole-directory staging layer that defeats streaming bounded delivery. The crate
therefore preserves order within each native record stream but labels it
unspecified. A higher traversal or presentation layer owns any sorting policy.

## Why metadata selection is limited to volume identity

The extended record supplies attributes, reparse tag, logical and allocation
sizes, extended-attribute size, four timestamps, and a 128-bit file ID together.
Dropping any of those fields saves no native work and would narrow the level
platform. The values remain native; in particular, signed 100-nanosecond Windows
timestamps avoid lossy epoch conversion.

Only volume qualification needs another query. Omitted, best-effort, and required
modes expose that real cost and capability boundary without per-entry opens.

## Why predicates and failures are crate-owned data

A validated data-only predicate can cross the SQ and run in a callback without
calling user code. A flat query-by-example conjunction maps the predicate leaves
already needed by Globazog, including native-name patterns, attribute masks, and
six-way size/time comparisons. Windows ordinal comparison defines case behavior
without handing wildcard semantics to a filesystem.

The same reasoning applies inside a session, to which of its own components may
act. A worker that finished its own enumeration would drop that enumeration's
thread-pool work object from inside that object's callback, waiting for itself
and then freeing the closure still running; and abandonment that released those
objects on the servicer would stall the session's only drain authority behind an
unbounded directory query. Making the worker a reporter, and keeping thread-pool
objects out of the registry entirely, removes both structurally rather than by
rule. Full details are in the enumeration crate's
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

An accepted failure is embedded in the enumeration's one reserved terminal.
This retains exact native detail without requiring another CQ data slot. Extended
directory information that is unavailable fails explicitly instead of falling
back to a metadata-poorer API. Likewise, a record that cannot fit the caller's
fixed capacity produces a typed oversize failure; hidden growth would violate the
memory bound and retry would rely on undocumented cursor behavior. Normal
exhaustion includes `ERROR_NO_MORE_FILES` on any refill and
`ERROR_FILE_NOT_FOUND` only on the initial restart refill; phase specificity
keeps a failed open or late read from looking like success. Full details and
alternatives are in the enumeration crate's
[DESIGN-RATIONALE.md](crates/windows-file-enumeration-sys/DESIGN-RATIONALE.md).

## Why the language baseline is checked rather than centralised

Records how the remedy in [DESIGN-NOTES.md](DESIGN-NOTES.md#restatement-drift) was reached.
The prompting event was an automated review of
[PR #46](https://github.com/MikeGrier/windows-threadpool-sys/pull/46) that raised seven
findings claiming `size_of::<T>()` would not compile without `use std::mem::size_of`. All
seven were wrong -- the function has been in the prelude since Rust 1.80 and this workspace
pins well past that -- but the reviewer was not being careless, and treating it as noise
would have guaranteed a repeat.

Three things had to coincide, and two were ours. The baseline is structurally invisible in a
diff: the toolchain pin is rarely edited so it never appears, the root manifest's
`[workspace.package]` table sits outside the hunk a normal change produces, and the crate
manifests carry `edition.workspace = true`, which is a pointer to a table that is not in the
diff either. Meanwhile the workspace's own older call sites imported or qualified `size_of`
in the pre-1.80 style -- the same struct, field, and conversion appeared written both ways in
two crates -- so local pattern-matching found genuine in-repo evidence for the wrong reading.
Only the third factor, that nearly all existing Rust predates the change, was outside our
control.

Four remedies were considered.

**Delete the restatements and link to the manifests.** Rejected, and it is the one that looks
most correct at first glance. The defect being fixed is precisely that a reader confined to a
diff cannot follow a link; substituting a pointer for the value optimises the document for a
reader who already has the whole repository open, which is not the reader who got this wrong.

**State the values and rely on the blast-radius sweep convention.** Rejected as insufficient
on its own. The convention governs someone who *knows* they are changing a contract; baseline
drift happens to someone bumping an MSRV who has no reason to suspect a dozen other files
mention it. Nothing prompts the sweep.

**Generate the documents from the manifests at build time.** Rejected as disproportionate. It
would require a generator, a check that the generated output is committed, and a templating
layer over prose that is mostly explanation rather than data -- appreciable machinery to keep
three short values honest, and a new artifact that can itself go stale.

**Check the restatements against the manifests in CI.** Adopted. It keeps the values where a
reviewer will read them, needs no toolchain (it only reads files), and costs one small
script. Its weakness is honest and worth stating: a *new* restatement in a file nobody
registered is not covered, since the checker polices a declared list rather than the whole
tree. Two things narrow that. The token sweep covers all of a registered file rather than
only its labelled claims, so drift in ordinary prose is caught without anyone anticipating
it; and a missing claim fails loudly, so the failure mode is a false alarm demanding
attention rather than silence. The design note therefore states the baseline's shape and
deliberately never its values, so it does not become an unpoliced copy itself.

Sabotage testing was treated as part of the deliverable rather than as validation of it,
following the rule that a binding which cannot be shown to fail is cosmetic. Five
mutations -- three manifest values, a deleted claim, and a stale version planted in prose --
each produce a distinct, located failure.

## Why a measured figure is asked to have one home

[DESIGN-NOTES.md](DESIGN-NOTES.md#prose-volume-and-error-surface) records the rule; this is how it
was reached, and what was rejected on the way.

The evidence was a review history, not an argument. Across the rounds on PR #90, most findings were
not wrong measurements -- they were transcriptions that had drifted from the thing they restated: a
table disagreeing with its own copy, a control quoted for the wrong regime, a wrap horizon stated in
minutes that the crate's own published rate contradicts. The measurements were fine. The
copies were not.

Two weaker rules were considered and rejected. **"Keep the copies in sync"** is what had already
been happening, and the failure mode is that nothing enforces it; every drifted figure on that
branch was written by someone intending to keep it in sync. **"Never publish a figure"** fails the
other way: a caller choosing a layout needs a number, and hiding it behind a link that may not be
followed trades one failure for another. What survived is narrower -- the *figure* lives in a
committed artifact, and prose carries the *claim* plus a link to it.

A correction from review is recorded with the rule itself: dropping the digits does **not** make a
claim permanent. A qualitative sentence cannot suffer transcription drift, because it transcribes
nothing, but a retake can still falsify it and a reader cannot see that from the sentence. So the
citation obligation is unchanged by the wording; only the transcription failure is removed. An
earlier draft of the rule said the digit-free form "cannot drift", which overstated it.

The mechanism -- how a figure gets from an artifact into rendered prose -- is deliberately left
open; markdown has no include, and rustdoc's include is whole-file. That is stated in the decision as an
unsettled trade rather than resolved here, and no work is scheduled against it.

## <a id="why-restatement-count-is-what-is-watched"></a>Why restatement count is what is watched

[DESIGN-NOTES.md](DESIGN-NOTES.md#prose-volume-and-error-surface) records the decision. This is how
it was reached, what was rejected on the way, and the remedy that was costed but not adopted. It was
moved here from that file, where it had been written inline: Tier 1 is the current decision, and a
section carrying its own motivating question, census procedure and superseded drafts had made the
decision harder to find inside it.

[Restatement drift](#restatement-drift) explains the mechanism and gives the remedy. This note
records something that section does not: a measurement of **where** the drift actually lives, taken
after PR #90's eighteenth review round, and what follows from it about formal specification.

The question that prompted it was whether this repository simply says too much -- whether English,
which must be inexact to serve human readers, is being asked to carry a specification load it cannot
bear, and whether some formal specification plus substantially less prose would shrink the error
surface.

### The measurement, and why it is not written down here

Prose volume was looked at first and set aside: whatever the ratio of prose to code is here, it is
not what the findings track. That ratio is deliberately not quoted, because quoting a measurement
this section takes no position on would be an uncited figure inside the argument against uncited
figures.

The thing to watch is that in `windows-waitable-queues`, a handful of single facts -- `Perpetual`'s
reservation-count ceiling, `Balanced`'s recurrence horizon, `Perpetual`'s position span, `Balanced`'s
field ceiling -- are each restated many times across several files, by hand, with nothing checking
any of them. Which of them has the most copies was counted once, during the review rounds that
produced this section, and has not been counted since; no census is committed, so that ordering is
recorded here as a historical observation rather than a current fact.

**The exact counts are deliberately not recorded here.** An earlier version of this section carried
them as a table, and the table drifted within days: one row gained an occurrence when a qualifier was
added to a rustdoc elsewhere in this same branch, so the census of restatements became a restatement
that needed maintaining. That is the section's own subject, demonstrated on the section.

Anyone who wants current numbers can compute them, which is the point of the principle below -- the
counts are a finding, and a finding should be computed rather than quoted:

```powershell
# Occurrences of a figure across the crate, and how many files carry it.
$files = git ls-files 'crates/windows-waitable-queues/*' |
    Where-Object { $_ -match '\.(rs|md|toml)$' }
foreach ($pattern in '\b255\b', '37 seconds', '2\^56', '4,294,967,295', 'about 20 years') {
    $hits = 0; $carrying = 0
    foreach ($file in $files) {
        $n = ([regex]::Matches([System.IO.File]::ReadAllText($file), $pattern)).Count
        if ($n) { $hits += $n; $carrying++ }
    }
    "{0,-16} {1,3} occurrences across {2} files" -f $pattern, $hits, $carrying
}
```

**All of these are restated by hand with nothing checking them.** Three of those facts -- the
ceiling, the span and the field ceiling -- follow from `ClaimLayout`'s associated constants. The
time figures follow from a field width *and* an assumed sustained push rate, so a
constants-versus-table check would validate the constant-derived facts outright and the time figures
only once the
rate is pinned somewhere single. That distinction bounds what the cheapest remedy below can do -- an
earlier version of this paragraph said every one was derivable from the constants, which overstated
it, in a note about overstatement. The error surface is proportional to how often a fact is restated,
not to total prose volume: a uniform cut to the prose leaves every restatement in place, just in
fewer words.

### Which errors this predicts, and which it does not

Sorting PR #90's findings across all rounds by class:

- **Restated derivable facts** -- the `2^31`/`2^30` target-dependent capacity, `MAX_RESERVED`
  conflated with capacity, "`Wide` removes it" for a bound that is finite, stale recurrence tables,
  a test count that matched no crate, the same horizon left unqualified across seven sites, a
  withdrawn magnitude surviving in two public rustdocs.
- **Structural** -- an unmarked supersedence row in a decision index, an orphaned milestone
  reference. A linter's job, not a specification's.
- **Evidence overclaiming** -- a noise floor computed from two runs, a refusal-count argument that
  did not reproduce in direction or magnitude across three re-measurements. These were the most
  valuable findings of the whole PR, and *more* measurement is what fixes them, not less prose.
- **Policy** -- client prescriptions surviving [D-no-client-prescriptions](crates/windows-platform-probes/DESIGN-NOTES.md#d-no-client-prescriptions).
  Only a reviewer catches these.
- **Algorithm properties** -- **zero findings, in any round.**

That last line is the one to be careful with, because it has two readings and only the second is
honest. There are no findings in that class because **there is no instrument for it**, not because
the algorithms are known good. `SH-14.1` is a live, known defect in the claim protocol; it was found
by a person reasoning carefully, and nothing in the toolchain would have caught it. Absence of
findings where nothing looks is not evidence of correctness -- the same error this repository has
corrected in its own measurements more than once.

### What follows

Three conclusions, of which the middle one is the one that changes practice.

**Formal specification and prose reduction address different classes.** TLA+ and `loom`
([D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31)) target algorithm properties. Neither
has been run, and neither is scheduled: `D-31` records the `loom` verification as planned, and
several documents name `M31.6` as its owner, but no checklist contains that item --
`windows-waitable-queues` has an archive,
[COMPLETED-CHECKLIST.md](crates/windows-waitable-queues/COMPLETED-CHECKLIST.md), and no open
checklist at all. So what can be said about that class is
that it produced no findings in any review round of PR #90
while carrying one known unfound defect, which is a statement about the reviews rather than a
result from either instrument. Restatement targets documented facts,
which have produced most findings. Both are worth doing; conflating them would aim the expensive
instrument at the cheap problem.

**The cut must be to restated assertions, not to rationale.** No finding in any round of PR #90 was
against a passage explaining *why* a decision was made. The findings were against duplicated
*assertions* of fact, against overclaims from evidence, and against prescriptions. Rationale is what
makes a decision re-checkable years later and is the reason this file exists at all; cutting it
uniformly to hit a volume target would remove the only prose that has never been wrong, while
leaving the prose that keeps being wrong in proportion.

**A formal spec's most useful property here is not proof -- it is that prose can point at it instead
of paraphrasing it.** That is [restatement drift](#restatement-drift)'s first remedy applied one
level up: define the protocol once in a form that can be checked, and let every document cite it.
This is the real connection between the two ideas, and it is why they belong in the same
conversation despite fixing different things.

### Prose carries the claim; an artifact carries the number

The sharper question, asked after several rounds of the above: **why is measured data living in
prose at all?**

There is no principled reason. It is an accident of what is easy. Markdown has no include and
rustdoc has no data include, so the only way to put a figure in front of a reader is to paste it --
and a pasted figure is a copy somebody must keep true by hand, in every place they pasted it,
forever.

The cost is measurable in this PR's own review history. Almost none of its measurement-related
findings were *wrong measurements*. They were **transcription failures**: the same table in the
README and the crate rustdoc disagreeing because one was retaken; an attribution naming a capture
the figures no longer came from; one recurrence horizon left unqualified across seven sites in three
wordings; a withdrawn magnitude surviving in two public rustdocs. The most instructive was a
proportion that restated two counts **given four words earlier in the same sentence** and got one of
them wrong -- it said "in both cases roughly 60%" where one of the two cases was 57 of 61. The data
was adjacent and the summary of it was false, because prose is not checkable and nobody checks it.

**This repository already contains the better pattern, and this branch was the first to apply it
in the probe crate.**
[`mutation-sweeps/2026-09-02/`](mutation-sweeps/2026-09-02) is a dated, committed capture directory: data as an artifact, cited
rather than retyped. `windows-platform-probes` produces the most-cited numbers in the workspace and
committed no capture at all when this section was written -- every figure it had published reached
its document by hand. The re-measurement that `M4.3` forced is the first exception:
[`crates/windows-platform-probes/captures/2026-09-16-drained-handshake/`](crates/windows-platform-probes/captures/2026-09-16-drained-handshake/README.md)
commits the raw runs, the script that derives the summary, and its output. The seven-run sweep that
the variance argument rests on still has no committed capture, so the gap this section describes is
narrowed rather than closed.

So the principle, which holds regardless of which mechanism is eventually chosen:

- **A claim belongs in prose.** "`reserving_mpsc` measured faster than `slotwise_mpsc` under
  contention, over a spread that overlaps the same-code control at every producer count" is a
  claim. It transcribes no figure, so it cannot drift from the artifact the way a pasted number
  does -- but it is not thereby permanent: a retake can make it false, and a reader cannot tell from
  the sentence alone. That is why the claim cites the artifact. Dropping the digits removes the
  transcription failure and leaves the citation obligation exactly where it was.
- **A number belongs in an artifact.** A measured cost, a capture's commit, a count of occurrences:
  one copy, with its provenance travelling *with* it rather than in a hand-maintained attribution
  table beside it.
- **A proportion over data we hold is not a finding, it is a restatement of one.** Computed by hand,
  checked by nobody, and stale the moment any input moves. The counts are the finding. A reader who
  wants a ratio can take one, against a denominator they chose and at a moment they know.

If this were adopted, the "which restatements are mechanically checkable" question earlier in this
note **dissolves** rather than being answered: all of them, because none would be restated.

**The mechanism is undecided and no work is scheduled here.** The reader-experience trade is real --
a figure in the prose is read by whoever reads the sentence, and a figure behind a link is read by
whoever follows it, which is a different and unmeasured set -- and it has not been settled.
Recorded as a principle so the next person choosing where to paste a number has the argument in front
of them, not as a queued change. Per "design notes are not a work queue", the absence of a checklist
item is deliberate.

### The cheapest available move, recorded but not scheduled

[README.md](crates/windows-waitable-queues/README.md) is already a build input for
`windows-waitable-queues` (`#[doc = include_str!]` in
[lib.rs](crates/windows-waitable-queues/src/lib.rs)), so a test can parse the published layout
tables and assert every row against `ClaimLayout`'s constants -- turning the occurrences that sit in
table rows into checked derivations of one definition, with no generator and no new tooling. It
reaches only those; the occurrences in prose are untouched by it.

**Be precise about what that would and would not catch, because this paragraph has now overstated it
twice.** The layout table's columns are the layout name, the reservation-count field ceiling, the
pushes-to-recurrence count, and a time. A constants check covers the **ceiling and push-count
columns** outright. The time column additionally needs the assumed rate pinned somewhere single. And
the two errors this note originally named -- the `2^31`/`2^30` target-dependent capacity and the
`MAX_RESERVED`-as-capacity conflation -- it would **not** have caught at all: both are prose
assertions in the surrounding text, not cells in any table.

That bound is the useful part rather than a caveat on it. A constants-versus-table check reaches the
occurrences that sit in table rows and none of the ones in prose, and both populations are
substantial -- which is the shape of the result, and a reason to build the check rather than not to.
The prose occurrences need something that reads assertions rather than rows. A remedy that covers the
tabular ones is worth having; claiming it covers both is how a partial instrument comes to be trusted
as a complete one.

*(An earlier version of this paragraph put a proportion here. It is gone deliberately: a ratio over
the counts above is a restatement of them, computed by hand and checked by nobody, and it drifts the
moment any file is edited -- which is the defect this whole note is about. The counts are the
finding. Anyone who needs a proportion can take one, against a denominator they chose and at a moment
they know.)*

**No work is scheduled by this note.** It was written to inform a decision that has not been taken,
and the deliberate absence of a checklist item is per the "design notes are not a work queue" rule
rather than an oversight. If the table-versus-constants test or a
prose-reduction pass is adopted, each needs its own item at that time.

## References

- [`QueueUserWorkItem` and `WT_TRANSFER_IMPERSONATION`](https://learn.microsoft.com/windows/win32/api/threadpoollegacyapiset/nf-threadpoollegacyapiset-queueuserworkitem)
- [`SubmitThreadpoolWork`](https://learn.microsoft.com/windows/win32/api/threadpoolapiset/nf-threadpoolapiset-submitthreadpoolwork)
- [WIL token helpers](https://github.com/microsoft/wil/blob/master/include/wil/token_helpers.h)
- [`GetFileInformationByHandleEx`](https://learn.microsoft.com/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandleex)

## <a id="machine-checking-what-is-argued"></a>Why M30 exists, what it is not, and how the pilot was chosen

Rationale for [CHECKLIST.md](CHECKLIST.md) -> `M30`. It is here, in Tier 2, rather than in
[DESIGN-NOTES.md](DESIGN-NOTES.md), because **no decision has been taken**: `M30.5` is what produces
one, and "adopt nothing, and say why" remains a legitimate result. Tier 1 records current decisions,
and recording pre-decision context there would let a reader mistake it for an adopted contract.

### What checks this workspace's concurrency today

Reasoning recorded beside the code, an extensive unit suite, a sabotage suite that injects defects
and requires each to be caught, and a cargo-mutants sweep. That combination is not weak, and it has
found real bugs -- [D-15](crates/windows-waitable-queues/DESIGN-NOTES.md#d-15)'s lost wakeup among
them.

### The measured blind spot, which is the reason for the milestone

[crates/windows-waitable-queues/README.md](crates/windows-waitable-queues/README.md) records that
weakening a producer's `Acquire` load of the consumer's position to `Relaxed` left the entire suite
green, while every *logic* defect injected beside it was caught. That asymmetry is not a gap in the
suite's thoroughness; it is what a test is. A test observes what a run happened to do, and cannot
observe an ordering that a run happened not to need.

The general form is worth keeping in view when reading the survey's results: classes with an oracle
converge, classes without one do not. Memory ordering has no oracle here.

### The goal is to narrow where hand-inspection has to look, not to replace it

A method that proves a protocol correct for three producers and a capacity of two does not prove the
shipping code correct. What it does is move a class of question out of "argued carefully" and into
"checked", so that what remains uncheckable is a short, named list rather than the whole surface.
**That list is the deliverable**, which is why `M30.3` is the item the milestone exists for rather
than a tidying step after the pilot. Formal methods here are a scoping instrument.

### The tool classes, and what each can and cannot see

- **TLA+/PlusCal** -- protocol-level, exhaustive over a small configuration. It has **no built-in
  hardware memory model**, and its ordinary interleaved-action semantics is sequential consistency,
  so by default it cannot see a weakened ordering. That is a property of the default model rather
  than an absolute limit: a specification *can* model weak-memory reordering explicitly, with store
  buffers or a reordering relation written into the spec. What it still checks in that case is the
  protocol as written, not the orderings the Rust implementation actually emits -- so the
  model-to-code gap remains, and `M30.1` should record which of the two is meant rather than
  treating "no memory model" as settled.
- **loom** -- an instrumented code-level model. It runs the crate's own logic, but the
  synchronization primitives must be substituted for `loom`'s instrumented types, and loom then
  explores the executions the C11 model permits. That is much closer to the code than a protocol
  spec, and it is where the measured weakened-`Acquire` blind spot lives -- but it is still a model:
  what runs under loom is not the shipping binary, and the Windows calls are not executed as
  written. An earlier version of this line called it "actual Rust under the C11 memory model", which
  overstated the guarantee and blurred exactly the model-to-code gap `M30.2` and `M30.3` exist to
  record.
- **kani or similar bounded proof** -- Rust, memory-safety and assertion checking.
- **`const` assertions** -- arithmetic relationships between constants. Already used here, and the
  cheapest of the four, because they fail the build rather than a run somebody chose to make.

### Why `SH-14.1` is the pilot, and why the reason is not the count

Reaching the claim position's wrap takes 2^32 pushes -- about 37 seconds of sustained maximum-rate
pushing on the host the queue crate publishes. That is far outside a unit suite budgeted in
milliseconds, but it is not in itself beyond a long integration test, so "beyond any test" would
overstate it.

What is beyond any test is the rest of the condition. The crate's README records that reaching the
wrap is necessary but *not sufficient*: a producer must also be stalled inside a window a few
instructions wide, and no test can schedule that deliberately. A model whose position wraps at 8
makes the whole interleaving reachable in seconds and **exhaustive** rather than sampled, and yields
a counterexample trace rather than a suspicion. Parameter shrinking earns its place by making the
interleaving exhaustive, not by making a count small.

`capacity == 1` was offered as a second candidate in an early draft and is not one. It belongs to
`slotwise_mpsc`'s slot *sequence* protocol
([D-12](crates/windows-waitable-queues/DESIGN-NOTES.md#d-12)), not to `reserving_mpsc`'s claim
position, and is already resolved by that shape refusing a capacity below two.

### This must not pre-empt D-31

[D-31](crates/windows-waitable-queues/DESIGN-NOTES.md#d-31) decided on considered grounds that 0.1.0
ships without machine-checked orderings. Its reasoning is the starting point rather than something to
overturn, and its central objection survives any tool choice: **no candidate models the real
`SetEvent`/`ResetEvent` calls**, so stubbing them verifies a model of `SetEvent` rather than
`SetEvent` itself. That is the "measures the model, not the thing" trap this workspace has already
been caught by once, and the doorbell is precisely where its one real ordering bug lived.

An earlier version of this section said the objection was that "a model checker covers atomics and
cannot cover `SetEvent`" -- which is wrong for TLA+, whose ordinary interleaved-action semantics is
sequentially consistent and which has no *built-in* memory model to cover atomics with (a spec can
model reordering explicitly, but then it checks the modelled protocol rather than the emitted code).
The tool-independent part of D-31's objection is only the syscall boundary; how much of the atomics
a tool sees is exactly what `M30.1` is for.

### The same trap has a second costume

Shrinking a model's parameters -- a position that wraps at 8 rather than 2^32 -- is itself a claim:
that the protocol's correctness does not depend on the width of that field. If nobody states why the
shipping code refines the reduced model, a green run proves something about the model alone. A
reduced model that has not been tied to the code can pass and mean nothing. `M30.2` carries that as
an acceptance criterion.

### Two corrections the milestone's own drafting needed

Both were errors in the argument *for* the pilot rather than in the plan, so a reader taking them on
trust would have aimed the pilot wrongly.

**The untestability was mis-attributed to the count.** The first draft said `SH-14.1` "needs 2^32
pushes to manifest and is therefore beyond any test". Corrected above: the count is reachable, and
the stall window is what is not.

**The success criterion went wrong twice, and the second error was the instructive one.** The first
draft asked for a counterexample from a deliberately broken variant and explicitly *not* a green run
on the correct one -- an overcorrection against vacuous green runs, since a counterexample from a
broken variant can equally be produced by a malformed or over-permissive model that would find one
anywhere. The fix was to require both: property check and anti-vacuity check.

That fix was wrong for this particular pilot, and the reason is worth keeping. **`SH-14.1` is a live
defect in the shipping protocol**, documented in the crate and disclosed to adopters. So a faithful
model of the shipping claim protocol, at a position width small enough to wrap, *must* find it --
and "the unmodified model satisfies its invariant" could only be satisfied by a model that does not
reproduce a defect the crate ships. The criterion was inverted: it would have been failed by a
correct model and passed by a broken one.

The general form: **when the system being modelled has a known defect, a green run on the unmodified
model is a failure signal, not a success.** The two checks still exist, but they attach to different
configurations rather than to modified and unmodified code -- the model must reproduce the defect
where the wrap is reachable, and must come back green where it is not (total pushes bounded below
the wrap, or a single producer, which has no race to lose). `M30.2` carries that as its criteria 2
and 3.
