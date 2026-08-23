# Checklist: windows-topology-sys

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md); the session that produced them is
[DESIGN-SESSION-2026-08-22-topology-schema.md](design-sessions/DESIGN-SESSION-2026-08-22-topology-schema.md).

## M1 -- Safe enumeration

- [x] **M1.1** -- Crate skeleton: `Cargo.toml`, `src/lib.rs`, README, workspace member, release-please
  registration. `windows-sys` features are `Win32_Foundation` and `Win32_System_SystemInformation`.
  Everything behind `cfg(windows)`.

- [x] **M1.2** -- `ProcessorSet`: a `(group, mask)` set that spans groups, with iteration, membership, and
  set operations (D-3). This is the crate's one abstraction above faithful records, and it is what prevents
  the recurring group-related bugs, so it lands before anything consumes it.

- [x] **M1.3** -- The safe `GetLogicalProcessorInformationEx` walk (D-1): two-call sizing against
  `ERROR_INSUFFICIENT_BUFFER`, advance by each record's own `Size` rather than by indexing, discriminate
  the union on `Relationship`, and read each trailing `GroupMask` array to its true `GroupCount` length
  rather than to the `[GROUP_AFFINITY; 1]` the type claims. Every one of those is a way to get this wrong
  silently; the walk is the crate's reason to exist.

- [x] **M1.4** -- Owned, faithful records for each relation: processor core (with SMT flag and efficiency
  class), package, cache (level, size, type), NUMA node, and group. Faithful means the record says what
  Win32 said, with a `ProcessorSet` in place of a raw mask and no interpretation layered on (D-2).

- [x] **M1.5** -- Tests: the walk survives a buffer whose records are of differing sizes; a
  multi-group `ProcessorSet` round-trips; enumeration on the host is self-consistent (every processor
  named by a domain exists in some group, and group processor counts agree with the group relation).

## M2 -- The description

- [ ] **M2.1** -- `Processor { id: (group, number), online, capacity }` and the open-kinded
  `Domain { kind, id, processors, ..attributes }` (D-4, D-6, D-7). Well-known kinds are `group`,
  `package`, `core`, `cache`, and `memory`; unknown kinds parse and round-trip rather than failing, which
  is the whole point of leaving the set open.

- [ ] **M2.2** -- A memory domain carries `memory_bytes` and may contain **no** processors (D-5). Assert
  that shape in a test with a hand-written CXL-style description, because the case is unreachable on
  most hardware and would otherwise go unexercised.

- [ ] **M2.3** -- Optional scalar `distances`, absent on Windows and populatable by a fed-in description.
  Document that this is deliberately not the HMAT attributed-relation model, with a pointer to D-9 so a
  reader finds the reasoning rather than assuming an oversight.

- [ ] **M2.4** -- Assemble a `Topology` from the M1 records, plus typed accessors over the open
  representation (`caches_at_level(3)`, `memory_domains()`) so the ergonomic cost of open kinds stays
  confined to the JSON (D-4).

## M3 -- Serialization

- [ ] **M3.1** -- `serde` behind a default-off optional feature, following `windows-file-watcher`'s D-72
  precedent, so an ordinary consumer never links it.

- [ ] **M3.2** -- Record in DESIGN-NOTES that the schema is not semver-covered (D-8), and state it in the
  rustdoc where a consumer will actually see it -- it is load-bearing for D-9's deferrals, not a footnote.

- [ ] **M3.3** -- Round-trip tests: discovered topology serializes and deserializes unchanged; a
  hand-written synthetic description parses; a description carrying an unknown domain kind survives a
  round trip; a Linux-shaped description (one group, more than 64 processors, populated distances,
  memory-only nodes) parses without loss (D-10).

## M4 -- Documentation

- [ ] **M4.1** -- Crate docs leading with what this is and is not: enumeration, not a renderer; no policy;
  no devices. Point at D-9 for what was excluded and why, so a consumer evaluating the crate can tell
  quickly whether it answers their question.

- [ ] **M4.2** -- A worked example printing the host's topology as JSON -- the thing a consumer will want
  first, and the thing that produces a description to hand-edit into a synthetic one.

> **-> CROSS-COMPONENT HANDOFF:** with M4 complete this crate can serve
> [windows-ioring-sys](../windows-ioring-sys/CHECKLIST.md) -> `M7` (`ring-copy`, the topology-aligned
> sample). That milestone is blocked on this one and carries the reciprocal prerequisite callout.
