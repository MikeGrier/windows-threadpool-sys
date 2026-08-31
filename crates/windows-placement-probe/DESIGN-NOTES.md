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
