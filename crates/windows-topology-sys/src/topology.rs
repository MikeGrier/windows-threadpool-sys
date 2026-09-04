// Copyright (c) 2026 Mike Grier
//! Assembling a [`MachineMemoryTopology`] from discovered relations.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use crate::EnumerationAnomaly;
use crate::cpu_set::CpuSet;
use crate::domain::{Domain, DomainKind, Processor, ProcessorFacts, ProcessorId};
use crate::observation::{AttributeObservation, Observation, ProcessorAttribute, Source};
use crate::observed::Observed;
use crate::processor_set::ProcessorSet;
use crate::provenance::Provenance;
use crate::relation::{self, Relations};
/// How many passes [`MachineMemoryTopology::discover`] makes before concluding
/// that a disagreement is genuine.
///
/// Three, because the bound's job is to separate "the machine changed while we
/// were looking" from "these two APIs disagree", and one repeat already does
/// that: a hot-add is not repeated microseconds later. The third pass is
/// slack for two changes landing in quick succession, and the cost of being
/// wrong in this direction is one more cheap enumeration, where the cost of
/// being wrong in the other is reporting a transient as genuine.
const COHERENCE_ATTEMPTS: u32 = 3;

/// How the two Win32 sources agreed when a topology was collected.
///
/// [`MachineMemoryTopology::discover`] reads the relationship walk and the
/// CPU-set enumeration in two separate calls, and Windows offers no
/// transactional way to read them together -- so the pair may describe
/// different instants. [D-16](../DESIGN-NOTES.md#d-16) is the decision this
/// type implements: retry to remove the transient cases, then represent
/// whatever survives.
///
/// **What is compared is which processors exist**, because that is the
/// disagreement a second pass can actually settle. A processor hot-added or
/// removed between the two calls appears in one source and not the other, and
/// is not hot-added again a microsecond later. Disagreements about how
/// processors are *grouped* -- which core, which node -- are deliberately not
/// retried: [D-17](../DESIGN-NOTES.md#d-17) establishes those are expected in
/// the field and persistent, so re-reading cannot settle them and they are
/// carried as separate per-source observations instead.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Coherence {
    /// Not collected from a running system, so the question does not arise.
    ///
    /// A hand-built or deserialized topology takes this: nothing was read
    /// twice, so there is nothing to have agreed. The default for the same
    /// reason.
    #[default]
    NotCollected,
    /// Both sources described the same processors on the pass this topology
    /// was built from.
    Agreed,
    /// They never agreed within the retry bound, so the disagreement is
    /// genuine rather than transient, and is reported rather than hidden.
    ///
    /// Per [D-16](../DESIGN-NOTES.md#d-16), exhausting the bound is not a
    /// failure to collect -- it is the *conclusion* that the disagreement is
    /// real. The topology is returned, and this says what differed on the
    /// final pass so a caller can decide what that means for its own use.
    Disagreed {
        /// Processors the relationship walk reported and CPU Sets did not.
        walk_only: Vec<ProcessorId>,
        /// Processors CPU Sets reported and the relationship walk did not.
        ///
        /// **These are absent from [`MachineMemoryTopology::processors`]**,
        /// which carries the walk's view alone. This field is where they are
        /// named, and reading it is the only way to learn they exist -- see
        /// [D-26](../DESIGN-NOTES.md#d-26) for why they are reported here
        /// rather than synthesized into a list that has one source.
        cpu_sets_only: Vec<ProcessorId>,
        /// How many passes were made before giving up.
        attempts: u32,
    },
}

/// A processor, cache, and memory topology: a set of processors and the
/// domains that relate them.
///
/// Built either by [`MachineMemoryTopology::discover`] from the running system, by hand,
/// or (with the `serde` feature) by deserializing a fed-in description.
///
/// The JSON shape this produces and accepts is explicitly not covered by this
/// crate's semver contract -- see [`Domain`]'s documentation and D-8 in
/// `DESIGN-NOTES.md`.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MachineMemoryTopology {
    /// Every logical processor, including one for each inactive slot up to a
    /// group's maximum processor count.
    ///
    /// **As the relationship walk reported them.** A processor that only the
    /// CPU-set enumeration saw is *not* here, and that is a real omission
    /// rather than an oversight: this list has one source, and inventing an
    /// entry for a processor the walk never described would put a `Processor`
    /// here with no `core`, no class and no domain membership -- a fabricated
    /// record of exactly the kind [`Observed`] exists to prevent.
    ///
    /// The disagreement is not lost, it is [reported](Self::coherence):
    /// [`Coherence::Disagreed`] names such processors in its `cpu_sets_only`
    /// field. A caller that needs them can read them there, and one that
    /// ignores coherence sees the walk's view, which is what this field has
    /// always been.
    ///
    /// Reaching that state takes a disagreement surviving every pass of
    /// [`Self::discover`]'s retry, so it is not the ordinary case -- but the
    /// ordinary case is not what a topology crate is for. See
    /// [D-26](../DESIGN-NOTES.md#d-26).
    pub processors: Vec<Processor>,
    /// Every domain.
    pub domains: Vec<Domain>,
    /// What `GetSystemCpuSetInformation` reported, as **its own observation**.    ///
    /// Windows describes processors through two APIs, and this is the second
    /// one. It is not a more convenient spelling of [`Self::domains`]: it
    /// carries facts the relationship walk has no equivalent for -- whether a
    /// processor is parked, whether it is allocated to *this* process, the
    /// scheduler's own last-level-cache grouping, a scheduling class and an
    /// allocation tag.
    ///
    /// It also **duplicates** some facts, deliberately and without
    /// reconciliation. `CoreIndex`, `NumaNodeIndex` and `EfficiencyClass` also
    /// appear, derived differently, in `domains` and [`Self::processors`]. The
    /// two paths can disagree -- under a hypervisor, or where one is stale --
    /// and merging them here would silently pick a winner and destroy the
    /// disagreement, which is the only thing a second observer is *for*. Which
    /// of them a consumer should believe, and what to do when they differ, is a
    /// decision that has not been taken.
    ///
    /// `None` means **not observed**, which is not the same as observed-and-
    /// empty: a hand-built or deserialized topology has not asked the running
    /// system, and a consumer must be able to tell that from a machine that
    /// genuinely reported nothing.
    #[cfg_attr(feature = "serde", serde(default))]
    pub cpu_sets: Option<Vec<CpuSet>>,
    /// What each source said about each processor's attributes.
    ///
    /// [D-18](../DESIGN-NOTES.md)'s second subject kind, and the one relation
    /// unification cannot reach: two sources describing the same core agree on
    /// `(kind, membership)` even when they disagree about a number hanging off
    /// it, so that disagreement has nowhere to live among the relation's own
    /// observations.
    ///
    /// Never reduced, for the same reason relations keep both observations:
    /// collapsing them would destroy the disagreement, which is the only thing
    /// a second observer is for. Use
    /// [`Self::attribute_conflicts`] to find the subjects the sources did not
    /// agree on.
    ///
    /// Empty for a hand-built or deserialized topology -- nobody asked, which
    /// is [`Observed::NotObserved`](crate::Observed::NotObserved)'s meaning
    /// carried up to a collection.
    #[cfg_attr(feature = "serde", serde(default, skip))]
    pub processor_attributes: Vec<AttributeObservation>,
    /// Where this content came from.
    ///
    /// **Defaults to [`Provenance::Synthetic`]**, so a topology built by hand
    /// or by [`Default`] is tainted unless its author says otherwise. Only
    /// [`Self::discover`] produces [`Provenance::Measured`], and
    /// deserialization can never produce it -- see
    /// [`Provenance`] for why the default points this way.
    #[cfg_attr(
        feature = "serde",
        serde(
            default,
            deserialize_with = "crate::provenance::deserialize_downgraded"
        )
    )]
    pub provenance: Provenance,
    /// Records the enumeration could not decode, if any.
    ///
    /// **Empty on every healthy machine**, and empty for a hand-built or
    /// deserialized topology, which asked nothing. A non-empty list means a
    /// buffer Windows returned did not describe what it claimed to -- a
    /// defective hypervisor, a driver corrupting memory -- and it carries the
    /// byte offset and the reason so that is diagnosable from the data rather
    /// than from a debugger.
    ///
    /// Per [D-24](../DESIGN-NOTES.md#d-24) this is *recorded* rather than
    /// thrown: a malformed record is not evidence that this crate reached an
    /// inconsistent state, so it must not panic, and dropping it silently would
    /// leave a consumer unable to tell a truncated enumeration from a small
    /// machine. Whatever decoded before the anomaly is still present in the
    /// fields above and is still correct.
    #[cfg_attr(feature = "serde", serde(default))]
    pub enumeration_anomalies: Vec<EnumerationAnomaly>,
    /// Whether the two Win32 sources described the same machine when this was
    /// collected.
    ///
    /// Stated rather than implied, per [D-16](../DESIGN-NOTES.md#d-16): a
    /// reader cannot infer from the data's shape how far its parts may be
    /// *correlated* with each other, and that is a different question from
    /// whether any single part is accurate.
    ///
    /// **Written out, never read back in.** A description is welcome to carry
    /// this so a human reading a dump can see how the run that produced it
    /// went, but a file cannot *establish* that two enumerations agreed --
    /// exactly as it cannot establish that the relationship walk observed
    /// anything (D-12). So deserialization always yields
    /// [`Coherence::NotCollected`], which is the true answer for a topology
    /// nobody collected, and a description claiming `Agreed` gains nothing by
    /// saying so.
    #[cfg_attr(feature = "serde", serde(skip_deserializing))]
    pub coherence: Coherence,
}

impl MachineMemoryTopology {
    /// Discover the running system's topology.
    ///
    /// # Errors
    ///
    /// Returns any error from the underlying `GetLogicalProcessorInformationEx`
    /// or `GetSystemCpuSetInformation` calls.
    pub fn discover() -> io::Result<Self> {
        // D-16's retry. Both calls are whole-machine enumerations and cheap, so
        // a repeat costs almost nothing, and it is not plausible that several
        // passes in a row fail to find a coherent set.
        //
        // Bounded, and the bound is the point: exhausting it is not a failure
        // to collect, it is the conclusion that the disagreement is genuine.
        // The topology is returned either way, with `coherence` saying which
        // happened.
        let mut last = None;
        for attempt in 1..=COHERENCE_ATTEMPTS {
            let (topology, cpu_sets) = Self::collect_once()?;
            let (walk_only, cpu_sets_only) = topology.processor_divergence(&cpu_sets);
            if walk_only.is_empty() && cpu_sets_only.is_empty() {
                return Ok(topology.finish(cpu_sets, Coherence::Agreed));
            }
            last = Some((topology, cpu_sets, walk_only, cpu_sets_only, attempt));
        }

        // Every pass disagreed. What survives a retry is not transience, so it
        // is represented rather than retried further or refused over.
        let (topology, cpu_sets, walk_only, cpu_sets_only, attempts) =
            last.expect("COHERENCE_ATTEMPTS is a non-zero constant, so the loop ran at least once");
        Ok(topology.finish(
            cpu_sets,
            Coherence::Disagreed {
                walk_only,
                cpu_sets_only,
                attempts,
            },
        ))
    }

    /// One pass over both sources, in the order the fold needs them.
    fn collect_once() -> io::Result<(Self, Vec<CpuSet>)> {
        let relations = relation::discover()?;
        let mut topology = Self::from_relations(relations);
        // Both are cheap reads of the running system, so both belong to
        // discovery -- neither is a measurement in the sense that would make it
        // expensive or optional.
        // The walk's per-processor claims, recorded before the fold so both
        // sources' claims about one processor sit side by side (D-18).
        topology.record_walk_attributes();
        let (cpu_sets, cpu_set_anomaly) = crate::cpu_set::enumerate()?;
        topology.enumeration_anomalies.extend(cpu_set_anomaly);
        // Returned unfolded, because the coherence question is asked of the two
        // sources *before* they are combined -- once folded, which source said
        // what about a processor's existence is no longer a question the shape
        // can answer.
        Ok((topology, cpu_sets))
    }

    /// Combine the pass's two sources and state how they agreed.
    fn finish(mut self, cpu_sets: Vec<CpuSet>, coherence: Coherence) -> Self {
        // Folded into the relation set, and *also* kept verbatim. Not a
        // contradiction: D-19's unified view is presented **in addition to**
        // the individual per-source ones, so a caller wanting what CPU Sets
        // said, in its own shape, still has it.
        self.fold_in_cpu_sets(&cpu_sets);
        self.cpu_sets = Some(cpu_sets);
        self.coherence = coherence;
        // The one place in the crate that may claim this is the machine you are
        // on, because it is the one place that asked the operating system.
        self.provenance = Provenance::Measured;
        self
    }

    /// Which processors one source reported and the other did not.
    ///
    /// Existence only. See [`Coherence`] for why grouping disagreements are
    /// deliberately outside this comparison.
    fn processor_divergence(&self, cpu_sets: &[CpuSet]) -> (Vec<ProcessorId>, Vec<ProcessorId>) {
        // Only processors the walk calls active are comparable: it reports a
        // slot for every position up to a group's maximum, including ones no
        // processor occupies, and CPU Sets reports only real processors. A raw
        // set comparison would therefore diverge on every machine with an
        // inactive slot, which is a difference in what the two APIs enumerate
        // rather than a difference about the machine.
        let walk: BTreeSet<ProcessorId> = self
            .processors
            .iter()
            .filter(|processor| processor.online)
            .map(|processor| processor.id)
            .collect();
        let sets: BTreeSet<ProcessorId> = cpu_sets
            .iter()
            .map(|set| ProcessorId {
                group: set.group,
                number: set.logical_processor_index,
            })
            .collect();
        (
            walk.difference(&sets).copied().collect(),
            sets.difference(&walk).copied().collect(),
        )
    }

    /// Record what the CPU-set enumeration says about relations, unifying with
    /// what the relationship walk already reported.    ///
    /// # What is folded, and what deliberately is not
    ///
    /// Only **core** and **NUMA node** membership. Those are the two facts both
    /// Windows APIs describe, so they are the two where "two sources, one
    /// relation" can arise at all.
    ///
    /// `LastLevelCacheIndex` is **not** folded. Per D-14 it answers a different
    /// question from the derived cache partitioning -- measured on the
    /// development host it reports one LLC group where the derivation finds
    /// eight L2 partitions, and neither is wrong -- so under D-15 it is a
    /// *different relation*, not a second observation of the same one. Folding
    /// it into `Cache` would assert an agreement that was never claimed.
    ///
    /// `EfficiencyClass` is not folded as a *relation* either, because it is a
    /// **per-processor attribute** rather than a membership -- D-18's other
    /// subject kind, tracked as `M3+.1.4`. It is read only when CPU Sets
    /// reports a core the walk did not, where it supplies that new relation's
    /// attribute rather than a fabricated one.
    ///
    /// # How a relation is matched
    ///
    /// By `(kind, membership)` per D-15 -- but *kind* here means which kind, not
    /// its attributes. Two sources reporting the same core over the same
    /// processors have observed one relation even if they disagree about its
    /// efficiency class; that disagreement is an attribute conflict, which is
    /// the subject `M3+.1.4` covers rather than a reason to treat them as two
    /// relations.
    fn fold_in_cpu_sets(&mut self, cpu_sets: &[CpuSet]) {
        // `CoreIndex` is documented as relative to the record's `Group`, so it
        // is not unique across a multi-group machine: group 0 core 0 and group 1
        // core 0 are different cores that share an index. Keying on the index
        // alone folded them into one cross-group `Core` domain, corrupting both
        // core membership and the efficiency class derived from it. Raised in
        // PR #56 review. `NumaNodeIndex` is *not* group-relative -- node numbers
        // are machine-wide -- so it keys on itself.
        let cores = Self::grouped_by(cpu_sets, |set| {
            (u32::from(set.group) << 8) | u32::from(set.core_index)
        });
        let nodes = Self::grouped_by(cpu_sets, |set| u32::from(set.numa_node_index));

        self.fold_memberships(
            &cores,
            |kind| matches!(kind, DomainKind::Core { .. }),
            |members| DomainKind::Core {
                // Derived from the membership rather than reported: CPU Sets
                // has no SMT field, and a core with more than one logical
                // processor is what the flag means.
                simultaneous_multithreading: members.len() > 1,
                // Taken from what CPU Sets actually reported for these
                // processors, never fabricated. Defaulting this to `0` would
                // reinvent the `Processor::capacity` sentinel the reshape
                // exists to remove -- `0` is a legitimate class, so a stand-in
                // would be indistinguishable from a real value.
                efficiency_class: members
                    .iter()
                    .map(|set| set.efficiency_class)
                    .max()
                    .unwrap_or_default(),
            },
            |members| members.first().map_or(0, |set| u32::from(set.core_index)),
        );
        self.fold_memberships(
            &nodes,
            |kind| matches!(kind, DomainKind::Memory { .. }),
            |_| DomainKind::Memory {
                memory_bytes: Observed::NotObserved,
            },
            |members| {
                members
                    .first()
                    .map_or(0, |set| u32::from(set.numa_node_index))
            },
        );
        // The attribute subject, which relation unification cannot reach: both
        // sources report an efficiency class for the same processor, and they
        // agree about the core while possibly disagreeing about this (D-18).
        for set in cpu_sets {
            self.processor_attributes.push(AttributeObservation::new(
                ProcessorId {
                    group: set.group,
                    number: set.logical_processor_index,
                },
                ProcessorAttribute::EfficiencyClass,
                u32::from(set.efficiency_class),
                Source::CpuSets,
            ));
        }
    }

    /// Record the relationship walk's per-processor attribute claims.
    ///
    /// The walk attaches an efficiency class to a *core*, so its claim about a
    /// processor is its core's value -- fanned out here rather than left for a
    /// consumer to re-derive, which is the reconstruction this model exists to
    /// stop.
    fn record_walk_attributes(&mut self) {
        let claims: Vec<AttributeObservation> = self
            .domains
            .iter()
            .filter(|domain| domain.observed_by(Source::RelationshipWalk))
            .filter_map(|domain| match domain.kind {
                DomainKind::Core {
                    efficiency_class, ..
                } => Some((efficiency_class, &domain.processors)),
                _ => None,
            })
            .flat_map(|(efficiency_class, processors)| {
                processors.iter().map(move |(group, number)| {
                    AttributeObservation::new(
                        ProcessorId { group, number },
                        ProcessorAttribute::EfficiencyClass,
                        u32::from(efficiency_class),
                        Source::RelationshipWalk,
                    )
                })
            })
            .collect();
        self.processor_attributes.extend(claims);
    }

    /// The `(processor, attribute)` subjects the sources did not agree on.
    ///
    /// Empty is the ordinary answer. A non-empty one is the **attribute
    /// conflict** [D-17](../DESIGN-NOTES.md) says to expect in the field and to
    /// record rather than refuse over -- firmware tables are populated
    /// incrementally, and the places to meet this are hardware nobody here has.
    ///
    /// Reported rather than resolved: picking a winner would destroy the
    /// disagreement, and on a hybrid part the choice decides whether a
    /// processor is treated as a performance or an efficiency core.
    #[must_use]
    pub fn attribute_conflicts(&self) -> Vec<(ProcessorId, ProcessorAttribute)> {
        let mut claims: BTreeMap<(ProcessorId, ProcessorAttribute), Vec<u32>> = BTreeMap::new();
        for observation in &self.processor_attributes {
            claims
                .entry(observation.subject())
                .or_default()
                .push(observation.value);
        }
        claims
            .into_iter()
            .filter(|(_, values)| {
                let mut distinct = values.clone();
                distinct.sort_unstable();
                distinct.dedup();
                distinct.len() > 1
            })
            .map(|(subject, _)| subject)
            .collect()
    }

    /// Group the CPU-set records by whatever `key` names, keeping the records
    /// themselves so a new relation's attributes come from what was reported.
    fn grouped_by(
        cpu_sets: &[CpuSet],
        key: impl Fn(&CpuSet) -> u32,
    ) -> BTreeMap<u32, Vec<&CpuSet>> {
        let mut grouped: BTreeMap<u32, Vec<&CpuSet>> = BTreeMap::new();
        for set in cpu_sets {
            grouped.entry(key(set)).or_default().push(set);
        }
        grouped
    }

    /// Attach a CPU-sets observation to each matching relation, adding one
    /// where no relation of that kind covers the same processors.
    fn fold_memberships(
        &mut self,
        grouped: &BTreeMap<u32, Vec<&CpuSet>>,
        is_kind: impl Fn(&DomainKind) -> bool,
        make_kind: impl Fn(&[&CpuSet]) -> DomainKind,
        label_of: impl Fn(&[&CpuSet]) -> u32,
    ) {
        for members in grouped.values() {
            let mut processors = ProcessorSet::empty();
            for set in members {
                processors.insert(set.group, set.logical_processor_index);
            }

            // The label is the source's own, taken from the records rather than
            // from the grouping key: those differ wherever the key has to carry
            // more than the label to be unique, which is the group-relative
            // `CoreIndex` case below.
            let observation = Observation::new(Source::CpuSets, label_of(members));
            match self
                .domains
                .iter_mut()
                .find(|domain| is_kind(&domain.kind) && domain.processors == processors)
            {
                // One relation, observed twice. The labels differ and both are
                // kept, which is the whole of D-15.
                Some(domain) => domain.observations.push(observation),
                // A relation only CPU Sets reported. Recorded rather than
                // dropped: the walk not describing it is a fact about the walk,
                // not evidence the relation is not there.
                None => self.domains.push(Domain {
                    kind: make_kind(members),
                    processors,
                    observations: vec![observation],
                }),
            }
        }
    }

    fn from_relations(relations: Relations) -> Self {
        let mut domains = Vec::new();

        for group in &relations.groups {
            domains.push(Domain {
                kind: DomainKind::Group,
                processors: group.active_processors.clone(),
                observations: vec![Observation::new(
                    Source::RelationshipWalk,
                    u32::from(group.group),
                )],
            });
        }
        for (index, package) in relations.packages.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Package,
                processors: package.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, die) in relations.dies.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Die,
                processors: die.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, module) in relations.modules.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Module,
                processors: module.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, core) in relations.cores.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Core {
                    simultaneous_multithreading: core.simultaneous_multithreading,
                    efficiency_class: core.efficiency_class,
                },
                processors: core.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for (index, cache) in relations.caches.iter().enumerate() {
            domains.push(Domain {
                kind: DomainKind::Cache {
                    level: cache.level,
                    associativity: cache.associativity,
                    line_size: cache.line_size,
                    size_bytes: cache.cache_size,
                    cache_type: cache.cache_type,
                },
                processors: cache.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, index as u32)],
            });
        }
        for node in &relations.numa_nodes {
            domains.push(Domain {
                kind: DomainKind::Memory {
                    memory_bytes: Observed::NotObserved,
                },
                processors: node.processors.clone(),
                observations: vec![Observation::new(Source::RelationshipWalk, node.node_number)],
            });
        }

        let processors = Self::processors_from(&relations, &domains);
        Self {
            processors,
            domains,
            cpu_sets: None,
            processor_attributes: Vec::new(),
            // Synthetic, not measured: this is a pure transform of whatever
            // relations it was handed, and cannot know where they came from.
            // `discover` stamps the claim because `discover` is what read the
            // machine -- so if this ever gains a second caller, that caller
            // does not silently inherit an assertion it has not earned.
            provenance: Provenance::Synthetic,
            // For the same reason: one set of relations is one source, and
            // coherence is a statement about two agreeing. `discover` sets this
            // once it has both.
            coherence: Coherence::NotCollected,
            // Carried, not re-derived: whatever the walk could not decode is a
            // fact about the enumeration that produced these relations, and a
            // pure transform is the wrong place to lose it.
            enumeration_anomalies: relations.anomalies,
        }
    }

    /// One `Processor` entry per slot up to each group's maximum processor
    /// count, online or not -- Windows only reports core/cache/node relations
    /// for active processors, so an inactive slot's capacity falls back to
    /// `0` rather than being invented.
    fn processors_from(relations: &Relations, domains: &[Domain]) -> Vec<Processor> {
        let mut result = Vec::new();
        for group_info in &relations.groups {
            for number in 0..group_info.maximum_processor_count {
                let id = ProcessorId {
                    group: group_info.group,
                    number,
                };
                let online = group_info
                    .active_processors
                    .contains(group_info.group, number);
                let capacity = online
                    .then(|| {
                        domains.iter().find_map(|d| match d.kind {
                            DomainKind::Core {
                                efficiency_class, ..
                            } if d.processors.contains(group_info.group, number) => {
                                Some(u32::from(efficiency_class))
                            }
                            _ => None,
                        })
                    })
                    .flatten()
                    .unwrap_or(0);
                result.push(Processor {
                    id,
                    online,
                    capacity,
                });
            }
        }
        result
    }

    /// The processor named by `id`, if this topology has one.
    #[must_use]
    pub fn processor(&self, id: ProcessorId) -> Option<&Processor> {
        self.processors.iter().find(|p| p.id == id)
    }

    /// Every domain of kind [`DomainKind::Group`].
    pub fn groups(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Group))
    }

    /// Every domain of kind [`DomainKind::Package`].
    pub fn packages(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Package))
    }

    /// Every domain of kind [`DomainKind::Core`].
    pub fn cores(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Core { .. }))
    }

    /// Every cache domain, at any level.
    pub fn caches(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Cache { .. }))
    }

    /// Every cache domain at exactly `level`.
    pub fn caches_at_level(&self, level: u8) -> impl Iterator<Item = &Domain> {
        self.domains.iter().filter(
            move |d| matches!(&d.kind, DomainKind::Cache { level: found, .. } if *found == level),
        )
    }

    /// Every cache level this machine reports, ascending, without repeats.
    ///
    /// Derived from what the topology actually contains rather than from a
    /// fixed ceiling. [`DomainKind::Cache`]'s `level` is a `u8`, so a caller
    /// that sweeps a hard-coded `1..=4` silently reports a partitioning L5 as
    /// absent -- a wrong answer that looks like a confident one.
    pub fn cache_levels(&self) -> Vec<u8> {
        let mut levels: Vec<u8> = self
            .caches()
            .filter_map(|d| match &d.kind {
                DomainKind::Cache { level, .. } => Some(*level),
                _ => None,
            })
            .collect();
        levels.sort_unstable();
        levels.dedup();
        levels
    }

    /// The distinct processor partitions the caches at `level` form.
    ///
    /// # Why this is not [`Self::caches_at_level`]
    ///
    /// Windows reports one relationship per *cache*, not per partition, and a
    /// level is routinely reported more than once over the very same
    /// processors. Measured on the eight-core development host rather than
    /// reasoned about: L1 arrives as eight `data` domains **plus** eight
    /// `instruction` domains covering exactly the same eight processor pairs.
    ///
    /// Counting relationships therefore claims sixteen L1 partitions where the
    /// machine has eight. Two consequences, both silent: a partition count that
    /// is a whole multiple too large, and -- on a machine whose L1i and L1d are
    /// its only two cache domains -- a level reported as *partitioning* when it
    /// divides nothing at all.
    ///
    /// Deduplication is by processor set, which is the thing a caller
    /// partitioning work actually cares about; the first domain covering each
    /// distinct set is kept, so the returned ids are stable for a topology.
    ///
    /// **Distinct is not disjoint.** Two sets that overlap without being equal
    /// both survive this, so the result is a set of domains rather than a
    /// proven partition. [`Self::outermost_partitioning_cache`] is where that
    /// stronger property is required and checked.
    ///
    /// **A domain covering no processors is dropped**, because it partitions
    /// nothing and every consumer of this list wants pieces of the machine. It
    /// would otherwise survive both filters here and in
    /// [`Self::outermost_partitioning_cache`]: it is not *equal* to any
    /// non-empty set, so deduplication keeps it, and it is disjoint from
    /// everything vacuously, so the pairwise check passes it. A level with one
    /// real cache plus one empty domain would then count two partitions and be
    /// reported as dividing a machine it does not divide. Contrast
    /// [`Self::memory_domains`], which deliberately keeps a processor-less
    /// domain because a memory domain with no CPUs is real hardware (D-5); a
    /// *cache* over no processors is not.
    pub fn cache_partitions_at_level(&self, level: u8) -> Vec<&Domain> {
        let mut partitions: Vec<&Domain> = Vec::new();
        for domain in self
            .caches_at_level(level)
            .filter(|domain| !domain.processors.is_empty())
        {
            if !partitions
                .iter()
                .any(|kept| kept.processors == domain.processors)
            {
                partitions.push(domain);
            }
        }
        partitions
    }

    /// The outermost cache level that actually divides this machine, together
    /// with the distinct partitions it forms.
    ///
    /// A level whose caches all cover the same processors partitions nothing --
    /// a fully shared L3 is one domain spanning everything -- so it is never a
    /// candidate however far out it sits. `None` means no reported level
    /// divides the machine, which is a real answer and not a failure: a
    /// single-core host is one partition by every measure.
    ///
    /// This is deliberately not "level 3". A shipping ARM64 laptop measured
    /// during the 2026-08-30 session reports **no L3 at all**, with two L2
    /// domains of six processors forming the real cluster boundary.
    ///
    /// # A level must be a partition, not merely a set of domains
    ///
    /// [`Self::cache_partitions_at_level`] deduplicates by equal processor set,
    /// which is what the measured case needs (L1i and L1d cover identical
    /// sets). That alone does **not** make the result a partition: two distinct
    /// sets can still overlap. Real hardware does not do this, but a
    /// `MachineMemoryTopology` is deliberately constructible by hand and by deserialization
    /// (see [`Provenance`](crate::Provenance)), so this method cannot assume
    /// hardware produced it -- and a caller splitting work across overlapping
    /// "partitions" double-counts the processors in the intersection and
    /// overwrites their domain assignment, silently.
    ///
    /// So a level qualifies only when its distinct sets are **pairwise
    /// disjoint**. Full coverage of the online processors is deliberately *not*
    /// required: a processor with no cache reported at a level is a gap in what
    /// the firmware said, not evidence that the domains which *were* reported
    /// overlap, and rejecting the level would discard a true boundary over it.
    ///
    /// # "Outermost" is decided by inclusion, not by the level number
    ///
    /// Level groups the relations into candidate partitions -- that is reading
    /// what the source said. It does **not** order them, because "a higher
    /// number is coarser" is asserted structure of the kind `M2+.2` forbids,
    /// and a machine that reports L2 over six processors and no L3 at all is
    /// the counterexample this crate has actually measured.
    ///
    /// A candidate is coarser than another when **every block of the other is
    /// contained in one of its blocks**, which is checkable against the very
    /// memberships Windows reported. On ordinary hardware the two agree; where
    /// they disagree, the firmware numbering is the one that is wrong.
    ///
    /// Defined here, in the crate that owns the topology, so that every
    /// consumer asks the same question rather than restating the rule and
    /// drifting from it.
    pub fn outermost_partitioning_cache(&self) -> Option<(u8, Vec<&Domain>)> {
        let candidates: Vec<(u8, Vec<&Domain>)> = self
            .cache_levels()
            .into_iter()
            .filter_map(|level| {
                let blocks = self.cache_partitions_at_level(level);
                (blocks.len() > 1 && Self::are_pairwise_disjoint(&blocks))
                    .then_some((level, blocks))
            })
            .collect();

        // Coarsest by inclusion: the candidate no other candidate is coarser
        // than. Where two candidates describe the *same* partition -- identical
        // blocks under different levels, which is the ordinary case for an L1
        // and L2 that split a machine the same way -- neither refines the other,
        // so the tie is broken by taking the **higher level**. Where they
        // describe *different* partitions neither of which refines the other,
        // there is no outermost one and the answer is `None`; see below.
        //
        // That tie-break reads the source's own labelling of one boundary and is
        // not the level ordering `M2+.2` forbids: distinct partitions are still
        // ordered by inclusion, and level decides only which of two names for
        // the identical partition is the outer one.
        // Two candidates can be *incomparable* rather than identical: disjoint
        // partitions `{0,1}/{2,3}` and `{0,2}/{1,3}` each survive the filter,
        // neither refines the other, and they are not the same partition. There
        // is then no outermost partitioning cache, and answering with the
        // higher-numbered level would invent one -- reintroducing exactly the
        // level ordering `M2+.2` forbids, in the one case where it changes the
        // answer. `None` is the honest result, and per D-13 the caller already
        // has to handle it.
        let maximal: Vec<_> = candidates
            .iter()
            .filter(|candidate| {
                !candidates
                    .iter()
                    .any(|other| Self::refines(&candidate.1, &other.1))
            })
            .collect();
        let first = maximal.first()?;
        if !maximal
            .iter()
            .all(|other| Self::same_partition(&first.1, &other.1))
        {
            return None;
        }
        maximal
            .iter()
            .max_by_key(|(level, _)| *level)
            .map(|(level, blocks)| (*level, blocks.clone()))
    }

    /// Whether two candidate partitions have exactly the same blocks.
    ///
    /// The identical case `refines` deliberately excludes: an L1 and an L2 that
    /// split the machine the same way are one boundary under two names, which is
    /// the ordinary case on real hardware and the only case the level tie-break
    /// is allowed to decide.
    fn same_partition(left: &[&Domain], right: &[&Domain]) -> bool {
        left.len() == right.len()
            && left
                .iter()
                .all(|block| right.iter().any(|o| o.processors == block.processors))
    }

    /// Whether every block of `finer` sits inside some block of `coarser`, and
    /// the two are not the same partition.
    ///
    /// The refinement order over candidate partitions, derived from the same
    /// membership inclusion the granularity order uses.
    fn refines(finer: &[&Domain], coarser: &[&Domain]) -> bool {
        let all_contained = finer.iter().all(|block| {
            coarser
                .iter()
                .any(|outer| block.processors.is_subset(&outer.processors))
        });
        let same = finer.len() == coarser.len()
            && finer
                .iter()
                .all(|block| coarser.iter().any(|o| o.processors == block.processors));
        all_contained && !same
    }

    /// Whether no two of these domains claim the same processor.
    ///
    /// Quadratic on purpose: the input is one cache level's distinct domains,
    /// which is a handful even on a large machine, and pairwise intersection
    /// asks the question directly rather than through a set type this crate
    /// would otherwise not need.
    fn are_pairwise_disjoint(domains: &[&Domain]) -> bool {
        domains.iter().enumerate().all(|(i, left)| {
            domains[i + 1..]
                .iter()
                .all(|right| left.processors.is_disjoint(&right.processors))
        })
    }

    /// Everything this crate knows about one processor, without sentinels.
    ///
    /// The shard-set surface `M4+.2` calls for: what a caller needs to decide
    /// whether a processor may host work, gathered so the answer is asked for
    /// once rather than assembled differently by each consumer.
    ///
    /// Every field says which absence it means (D-13), and **none uses a
    /// sentinel**. That is the point rather than a nicety:
    /// [`Processor::capacity`] spells "offline", "in no core", and "efficiency
    /// class zero" as the same `0`, and the third is every processor on every
    /// non-hybrid machine -- so the stand-in collides with the overwhelmingly
    /// common real value and no careful caller can tell them apart.
    #[must_use]
    pub fn shard_set(&self) -> Vec<ProcessorFacts<'_>> {
        self.processors
            .iter()
            .map(|processor| {
                let core = self.cores().find(|d| {
                    d.processors
                        .contains(processor.id.group, processor.id.number)
                });
                let cpu_set = self.cpu_sets.as_ref().and_then(|sets| {
                    sets.iter().find(|s| {
                        s.group == processor.id.group
                            && s.logical_processor_index == processor.id.number
                    })
                });
                ProcessorFacts {
                    id: processor.id,
                    online: processor.online,
                    core,
                    simultaneous_multithreading: match core.map(|d| &d.kind) {
                        Some(DomainKind::Core {
                            simultaneous_multithreading,
                            ..
                        }) => Observed::Known(*simultaneous_multithreading),
                        _ => Observed::NotObserved,
                    },
                    efficiency_class: match core.map(|d| &d.kind) {
                        Some(DomainKind::Core {
                            efficiency_class, ..
                        }) => Observed::Known(*efficiency_class),
                        _ => Observed::NotObserved,
                    },
                    parked: cpu_set.map_or(Observed::NotObserved, |s| Observed::Known(s.parked)),
                    allocated_to_this_process: cpu_set.map_or(Observed::NotObserved, |s| {
                        Observed::Known(s.allocated_to_target_process)
                    }),
                    memory_domain: self.memory_domain_of(processor.id),
                }
            })
            .collect()
    }

    /// Which memory domain `processor` allocates from.
    ///
    /// [`Observed::Known`] with the domain, or [`Observed::NotObserved`] when
    /// no memory domain names it -- the **unplaced** case, which is deliberately
    /// not collapsed into "node 0".
    ///
    /// # Why the unplaced case gets its own answer
    ///
    /// An unknown *cache* domain costs an optimisation. An unknown *memory*
    /// domain has no honest fallback at all: the pool has to be allocated
    /// somewhere, and guessing means quietly allocating remote memory for the
    /// life of the process. `windows-placement-probe` already encodes this
    /// asymmetry -- it refuses a missing NUMA node while tolerating a missing
    /// cache domain -- and this method is where that distinction becomes the
    /// model's rather than each consumer's.
    ///
    /// [`Observed::Absent`] is never returned: a memory domain covering no
    /// processors is a real shape (D-5), but "this processor belongs to no
    /// node" is a gap in what the firmware said, not a positive statement that
    /// it has no memory.
    #[must_use]
    pub fn memory_domain_of(&self, processor: ProcessorId) -> Observed<&Domain> {
        self.memory_domains()
            .find(|domain| {
                domain
                    .processors
                    .contains(processor.group, processor.number)
            })
            .map_or(Observed::NotObserved, Observed::Known)
    }

    /// Every processor no memory domain names.
    ///
    /// The set a caller must decide about before allocating anything, and
    /// empty on a machine whose firmware covered every processor. Offered so
    /// the question is asked once rather than rediscovered per allocation site.
    #[must_use]
    pub fn unplaced_processors(&self) -> Vec<ProcessorId> {
        self.processors
            .iter()
            .filter(|processor| !self.memory_domain_of(processor.id).was_observed())
            .map(|processor| processor.id)
            .collect()
    }

    /// Every memory domain, including one with no processors (D-5).
    pub fn memory_domains(&self) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(|d| matches!(d.kind, DomainKind::Memory { .. }))
    }
}

#[cfg(test)]
mod tests;
