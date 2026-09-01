# Design notes: windows-placement-probe

Decisions about the tool that measures what thread placement costs and produces
a record someone can paste into a discussion thread.

## crates.io is a second path, and it must not become the first one

**Decided: publish to crates.io, but not yet.** The reasoning is about which
path a runner meets first, not about whether the reach is worth having.

The product of this tool is not a binary, it is **data whose provenance can be
checked**. A binary attached to a GitHub release is traceable to the commit that
built it, because CI stamps that commit in and the download itself is the
evidence. A crates.io tarball carries no repository, so a `cargo install`ed
build finds no git metadata and reports its commit as unknown -- by
construction, not by oversight. Records from that path are strictly weaker, and
the tool marks them `!!UNOFFICIAL!!` accordingly.

That is the whole tension: `cargo install <name>` is the more discoverable
route, needs no release page, and is what a Rust developer reaches for first.
Publishing early would make the weakest submission path also the easiest one,
and submissions would drift to it before anyone noticed.

**Timing resolves it.** Publishing *after* the download path exists, is
documented in the README, and has been walked end to end by someone without this
repository means the strong path is the one people meet first, and crates.io
becomes the fallback it should be rather than the default.

**A hard prerequisite, not merely a preference.** Publishing this crate requires
its dependencies to be on crates.io: it depends on `windows-topology-sys` and
`windows-waitable-queues` by path, and a published crate cannot. As of this
writing `windows-topology-sys` 0.1.0 is published and `windows-waitable-queues`
is not, so the earliest possible publication is after that release. The
dependency pins also need to name whatever version is actually published --
during local development the `path` entry is used and the `version` entry is
never exercised, so a stale pin is invisible until the moment it matters.

**Rejected: baking the commit into the packaged source so a crates.io build can
name it.** It is achievable -- generate a file at package time and read it when
git is absent -- and it would answer a different question than the one the
marking exists to ask. "This source came from commit X" is not "this binary was
built by CI from commit X"; only the second makes the artifact independently
checkable, because only the second was produced by something other than the
person submitting the record. Blurring the two would leave the record's trust
section saying something it cannot support. The unknown commit is honest, and
honest is the point.

**What publication will oblige**, recorded so the cost is not rediscovered
later: the record schema becomes a semver surface the moment anyone stores one
(see the package metadata), and the README must say plainly that a crates.io
build produces records marked unofficial, so nobody chooses that path without
knowing what it costs the data.

## The schema freezes at the first release, not before

**Decided: regenerate `schema/v1.txt` in place while the tool is unreleased, and
apply the append-only rule from the first release onward.**

The archived golden exists so that a shape change cannot happen silently, and the
append-only rule on top of it exists for a narrower reason: **a record already
held by someone else cannot be regenerated**, so once a record in the wild claims
schema N, N's meaning is fixed.

That rationale is about other people's data. Until the tool is released nobody
has any, so bumping the version records a shape that never reached a single
reader -- and the first public release then ships already carrying dead numbers,
each with an archived file describing a record that never existed.

This was learned by doing it. During development the schema was raised twice in
one evening, to v3 and then v4, each with a derived golden, on the reasoning that
the tool exists to emit records people paste and some had been produced locally.
Following that consistently would have released a crate whose first version
carried four schema files and three unreachable versions. It was collapsed back
to v1 in one change.

**The latitude ends at the release, and the boundary is deliberately sharp**
rather than a judgement call repeated per change:

- *Before the first release* -- regenerate `v1.txt` from the record. The golden
  still guards every change, because the test compares the file against a freshly
  serialized record either way; what is given up is only the archive of shapes
  nobody received.
- *After the first release* -- never edit a published golden. The next shape
  change raises `SCHEMA_VERSION` and adds the next file beside it.

**Rejected: keeping v1 through v4.** It is the safer-looking option and it is
strictly worse for a reader, who would find four archived shapes and no way to
tell that three of them were never emitted by any build anyone could obtain.

**Rejected: dropping the golden until release.** The guard is what makes an
accidental shape change visible, and that is as valuable during development as
after it -- more so, since that is when the shape actually moves.
