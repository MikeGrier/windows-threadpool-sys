# Checklist: topology provenance

**Problem.** [crates/windows-topology-sys/src/topology.rs](crates/windows-topology-sys/src/topology.rs)
documents that a `Topology` is "built either by `Topology::discover` from the running system, by hand,
or (with the `serde` feature) by deserializing a fed-in description" -- and **nothing distinguishes the
three once built**. `Topology` derives `Default`, has public fields, and derives `Deserialize`. There is
a passing test that parses a *Linux-shaped* description, complete with an ACPI SLIT-style distance
matrix, on a Windows-only crate. A consumer handed that value treats another machine's topology, or a
fabricated one, as this machine's truth.

This is not hypothetical for the work in flight. `probe-core-affinity` needs synthetic multi-node
topologies precisely because no NUMA machine is available, and the whole point of a probe is that its
output is believed.

**Decision.** Topology content carries its own provenance, defaulting to the *untrusted* value so that
forgetting is safe and claiming is deliberate. Persisted forms carry it visibly, and loading can only
ever downgrade -- a file cannot assert that it is this machine.

Related: [CHECKLIST-io-domains.md](CHECKLIST-io-domains.md) M-inf.4, which is what surfaced this.

## M1: the marker, and its invariants

- [x] **TP-1.1** -- Add `Provenance` to `windows-topology-sys` with three states ordered by trust:
  `Measured` (read from the running system), `Restored` (deserialized from a description of some
  machine), `Synthetic` (constructed by hand). **`Synthetic` is `Default`.** That is the load-bearing
  choice: `Topology::default()`, `..Default::default()`, and any construction that omits the field all
  come out tainted, so a caller must do work to claim data is real rather than work to admit it is not.
  Document that the threat model is *accident*, not forgery -- a caller who writes
  `provenance: Measured` over fabricated data has lied deliberately, and no type prevents that.

- [x] **TP-1.2** -- Add the field to `Topology` and set `Measured` in `discover()`. This is a **breaking
  change** for struct-literal construction, and deliberately so: every existing site is forced to state
  which kind of data it holds. Update the crate's own tests and every dependent that constructs a
  `Topology` by hand.

- [x] **TP-1.3** -- Serde: serialize the marker so it is *visible* in the persisted form, and
  **downgrade on load** -- `Measured` becomes `Restored`, everything else is unchanged. The rule is
  **never upgrade**, so a hand-edited `"provenance": "measured"` is ignored rather than honoured. A
  description absent the field loads as `Synthetic`. Test each of the four load cases, including that a
  round trip of a measured topology does not come back measured.

## M2: making it loud where it is read

- [x] **TP-2.1** -- `Fingerprint` in [crates/windows-platform-probes/src/fingerprint.rs](crates/windows-platform-probes/src/fingerprint.rs)
  carries the provenance and renders it **first and unmissably** when it is not `Measured`. The
  fingerprint string is documented as canonical, so string equality is a usable comparison -- which
  means the marker must be *inside* the string, or a synthetic host could compare equal to a real one.
  That is the specific bug this prevents, not merely a display nicety.

- [x] **TP-2.2** -- Every probe banner and every persisted probe line inherits it, since
  `print_banner` and `Slice` are what end up pasted into checklists and design notes. A number quoted
  from a synthetic run must arrive already labelled, because the label is what a reader will not think
  to ask for.
  **Done, and the banner inherits it by construction** -- it embeds the fingerprint's own `Display`
  rather than re-rendering, so the two cannot drift. `print_banner` was split so the line is available
  as a string (`banner_line`) and the marker's arrival is asserted rather than confirmed by reading a
  format string.
  **`Slice` deliberately carries no marker of its own, and the reason is structural rather than an
  oversight.** A `Slice` records which processors a measurement was pinned to, and one can only exist
  from a real `measure()` run: `measure` takes no injected topology (and
  [crates/windows-platform-probes/src/core_affinity.rs](crates/windows-platform-probes/src/core_affinity.rs)
  now documents why it must not), and pinning to a processor that does not exist panics. A slice is
  therefore always real, and it is always printed beneath the banner that carries the host's
  provenance. **If `measure` ever does gain such a seam, this reasoning collapses and `Slice` needs its
  own marker** -- which is a second, independent reason not to add one.

## M3: closing the loop with the probes

- [ ] **TP-3.1** -- Reconsider whether `probe-core-affinity`'s synthetic hosts should be expressed as
  `Topology` values rather than as `Vec<ProcessorPlace>`. Going through `Topology` would exercise the
  provenance path end to end and let a synthetic *NUMA* host drive selection through the real
  `discover_places` conversion; staying at `ProcessorPlace` keeps the tests pure and fast. **Decide on
  the evidence, and record the decision either way** -- this item is not "do it", it is "choose".
  Note the constraint from
  [crates/windows-platform-probes/src/core_affinity.rs](crates/windows-platform-probes/src/core_affinity.rs):
  `measure()` must still not gain a topology-injection seam, whatever is decided here.
