// Copyright (c) Mike Grier.

//! The vocabulary of what a cross-check found, as values rather than sentences.
//!
//! # Why these are types and not `String`s
//!
//! Each of [`CrossCheck`](super::CrossCheck)'s three lists used to hold the
//! human sentence and nothing else, and the NDJSON row published each list's
//! **length**. So the fact a mining pass most needs -- *which* condition
//! occurred -- existed only in the prose: a survey reading
//! `"parse_incomplete":1` could not tell *the probe detected a bug in itself*
//! from *a core record contradicted itself* from *this topology was not
//! measured from a running machine*.
//!
//! That is the gap recorded in
//! [DESIGN-NOTES.md](../../DESIGN-NOTES.md#d-encoded-row-is-the-contract): the
//! row was impoverished relative to the prose, which is backwards given that the
//! row is the artifact a fleet survey mines and the designs rest on.
//!
//! A variant carries its own data, renders its own sentence through
//! [`fmt::Display`], and names itself through `code`. One definition, so the
//! sentence and the discriminant cannot drift apart -- where a parallel
//! `(code, String)` pair could be updated on one side only.
//!
//! # The code is the contract; the sentence is not
//!
//! `code` is a **stable machine discriminant** and changing one is a breaking
//! change to the row, exactly as renaming a field would be. The `Display` text
//! is free to be reworded at any time: it reaches only the prose, where a reader
//! is the consumer and rewording is harmless.
//!
//! This is what lets the prose stay written for a human. Before, a test or a
//! survey wanting to know which condition fired had to match on English, so
//! improving a sentence risked breaking a consumer -- which is a reason not to
//! improve it.

use std::fmt;

use windows_topology_sys::{AnomalyKind, EnumerationAnomaly, Source};

use crate::row::Value;

/// A diagnostic as the row publishes it: its code, and the values it carries.
///
/// **The data, not only the code.** M3.1 published the code alone, so a survey
/// learned `contradictory_cores` without learning that three cores contradicted
/// themselves. The variants already carry those values, for `Display`; what
/// stopped M3.1 publishing them is that the row was a positional template where
/// a nested object had to be hand-assembled.
///
/// `code` is always first, so a reader scanning accumulated output sees the
/// identity before the detail.
fn entry(code: &'static str, fields: Vec<(&'static str, Value)>) -> Value {
    let mut members = vec![("code", Value::Text(code.to_owned()))];
    members.extend(fields);
    Value::Object(members)
}

/// A counter comparison that was made and did not match.
///
/// Each is a finding about the shipping crate's parse, and is the only list
/// whose entries produce [`Verdict::Disagree`](super::Verdict::Disagree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disagreement {
    /// The parsed processor count and `GetActiveProcessorCount` differ.
    OnlineProcessors {
        /// What `windows-topology-sys` parsed.
        parsed: usize,
        /// What the Win32 counter reported.
        counter: u32,
    },
    /// The parsed group count and `GetActiveProcessorGroupCount` differ.
    ProcessorGroups {
        /// What `windows-topology-sys` parsed.
        parsed: usize,
        /// What the Win32 counter reported.
        counter: u16,
    },
    /// The parsed highest NUMA node and `GetNumaHighestNodeNumber` differ.
    ///
    /// Highest against highest, never a count against `highest + 1`: Windows
    /// does not promise the largest node *number* equals the node count, and
    /// nodes 0 and 2 are a valid sparse topology.
    HighestNumaNode {
        /// The largest node number the parse carries, if it carries any.
        parsed: Option<u32>,
        /// What the Win32 counter reported.
        counter: u32,
    },
}

impl Disagreement {
    /// The stable discriminant a survey groups by.
    ///
    /// Changing one of these is a breaking change to the NDJSON row.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::OnlineProcessors { .. } => "online_processors",
            Self::ProcessorGroups { .. } => "processor_groups",
            Self::HighestNumaNode { .. } => "highest_numa_node",
        }
    }
}

impl Disagreement {
    /// This disagreement as the row publishes it.
    #[must_use]
    pub fn published(&self) -> Value {
        let fields = match self {
            Self::OnlineProcessors { parsed, counter } => vec![
                ("parsed", Value::Number(*parsed)),
                ("counter", Value::Number(*counter as usize)),
            ],
            Self::ProcessorGroups { parsed, counter } => vec![
                ("parsed", Value::Number(*parsed)),
                ("counter", Value::Number(*counter as usize)),
            ],
            Self::HighestNumaNode { parsed, counter } => vec![
                (
                    "parsed",
                    parsed.map_or(Value::Null, |node| Value::Number(node as usize)),
                ),
                ("counter", Value::Number(*counter as usize)),
            ],
        };

        entry(self.code(), fields)
    }
}

impl fmt::Display for Disagreement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OnlineProcessors { parsed, counter } => write!(
                f,
                "online processors: topology crate says {parsed}, GetActiveProcessorCount says \
                 {counter}"
            ),
            Self::ProcessorGroups { parsed, counter } => write!(
                f,
                "groups: topology crate says {parsed}, GetActiveProcessorGroupCount says {counter}"
            ),
            Self::HighestNumaNode { parsed, counter } => write!(
                f,
                "NUMA nodes: topology crate's highest node is {}, GetNumaHighestNodeNumber says \
                 {counter}",
                parsed.map_or_else(|| "none".to_string(), |n| n.to_string()),
            ),
        }
    }
}

/// A comparison this run could not make, or could not trust.
///
/// Each is a gap in this measurement, not a finding about the parse -- which is
/// why [`parse_in_doubt`](super::CrossCheck::parse_in_doubt) excludes this list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotCompared {
    /// The counters moved across the parse, so the two readings describe
    /// different instants.
    MachineChanged,
    /// A counter failed one of its two reads, so nothing established that the
    /// machine held still.
    BracketNotEstablished,
    /// The counts include relations no platform API reported, so a mismatch
    /// could not be attributed to the parse.
    CountsIncludeUnparsedRelations,
    /// `GetActiveProcessorCount` reported failure.
    ActiveProcessorCountFailed,
    /// `GetActiveProcessorGroupCount` reported failure.
    ActiveProcessorGroupCountFailed,
    /// `GetNumaHighestNodeNumber` reported failure.
    HighestNumaNodeFailed,
}

impl NotCompared {
    /// The stable discriminant a survey groups by.
    ///
    /// Changing one of these is a breaking change to the NDJSON row.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::MachineChanged => "machine_changed",
            Self::BracketNotEstablished => "bracket_not_established",
            Self::CountsIncludeUnparsedRelations => "counts_include_unparsed_relations",
            Self::ActiveProcessorCountFailed => "active_processor_count_failed",
            Self::ActiveProcessorGroupCountFailed => "active_processor_group_count_failed",
            Self::HighestNumaNodeFailed => "highest_numa_node_failed",
        }
    }
}

impl NotCompared {
    /// This entry as the row publishes it.
    ///
    /// Every variant is a bare condition with no values of its own, so each
    /// publishes its code and nothing else -- which is the honest rendering
    /// rather than an object padded to look like the others.
    #[must_use]
    pub fn published(&self) -> Value {
        entry(self.code(), Vec::new())
    }
}

impl fmt::Display for NotCompared {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::MachineChanged => {
                "the machine changed while this ran -- the counters moved across the parse, so \
                 the two readings describe different instants"
            }
            Self::BracketNotEstablished => {
                "the bracket around the parse was not closed -- a counter failed one of its two \
                 reads, so nothing established that the machine held still"
            }
            Self::CountsIncludeUnparsedRelations => {
                "these counts include relations no platform API reported, so a counter mismatch \
                 could not be attributed to the parse"
            }
            Self::ActiveProcessorCountFailed => {
                "GetActiveProcessorCount returned 0, which is its failure report"
            }
            Self::ActiveProcessorGroupCountFailed => {
                "GetActiveProcessorGroupCount returned 0, which is its failure report"
            }
            Self::HighestNumaNodeFailed => {
                "GetNumaHighestNodeNumber failed, so no NUMA comparison was made"
            }
        };

        f.write_str(text)
    }
}

/// A way the parse is short, or its claims mutually inconsistent, such that
/// agreeing counters cannot certify it.
///
/// Established from the PARSE rather than from any counter, which is why no
/// counter agreeing can retire an entry here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseIncomplete {
    /// `windows-topology-sys` recorded records it could not decode.
    ///
    /// States the CONDITION, not a direction. "Records were dropped and the
    /// counts are short" was true of the two anomaly kinds that decode to
    /// nothing and false of `TruncatedArray`, which keeps the record with the
    /// entries that fit -- and a cache record kept with a partial affinity mask
    /// can INFLATE a partition count rather than shorten it, so the old message
    /// pointed a reader the wrong way.
    EnumerationAnomalies {
        /// How many anomalies were recorded.
        count: usize,
    },
    /// NUMA domains only CPU Sets reported, so the two sources group nodes
    /// differently.
    NumaDomainsOnlyInCpuSets {
        /// How many such domains.
        count: usize,
    },
    /// The survey reported no cache relationships whatsoever.
    NoCacheLevels,
    /// Levels the survey carries that decoded to no partitions at all.
    CacheLevelsWithoutPartitions {
        /// Which levels.
        levels: Vec<u8>,
    },
    /// A measured topology reported none of a count a running machine must have.
    MeasuredButCountsAbsent {
        /// Which counts were zero, in the order the report names them.
        absent: Vec<&'static str>,
    },
    /// No packages were reported, though the machine has one.
    NoPackages,
    /// No cores were reported, though the machine has one.
    NoCores,
    /// Cores whose SMT flag disagrees with the processors recorded beside it.
    ContradictoryCores {
        /// How many such cores.
        count: usize,
    },
    /// Cache levels numbered 0, which is not a level Windows reports.
    UnnumberedCacheLevels {
        /// How many such levels.
        count: usize,
    },
    /// A level was named as the outermost partitioning cache with no summary
    /// for it.
    ///
    /// This is the probe detecting a bug in itself, and is the condition the
    /// renderer prints as `BUG IN THIS PROBE`.
    PartitioningSummaryMissing {
        /// The level that was named.
        level: u8,
    },
    /// The topology was not measured from a running machine.
    NotMeasured,
    /// Cores and packages that cover no processors.
    RelationsWithoutProcessors {
        /// How many cores.
        cores: usize,
        /// How many packages.
        packages: usize,
    },
    /// Relations carrying no observation from any source.
    UnreportedRelations {
        /// How many relations.
        count: usize,
    },
    /// Relations a caller described rather than any platform API reporting them.
    DescribedRelations {
        /// How many relations.
        count: usize,
    },
    /// Cores only CPU Sets reported, so the two sources group processors into
    /// cores differently.
    CoresOnlyInCpuSets {
        /// How many such cores.
        count: usize,
    },
    /// Walk relations sharing a processor with another of the same kind.
    OverlappingWalkRelations {
        /// How many relations.
        count: usize,
    },
    /// Per-processor attributes carrying more than one distinct value.
    ProcessorAttributeConflicts {
        /// How many attributes.
        count: usize,
    },
    /// NUMA domains carrying more than one distinct node number.
    NumaDomainsWithConflictingLabels {
        /// How many domains.
        count: usize,
    },
    /// NUMA domains carrying no observation from either source.
    NumaDomainsUnreported {
        /// How many domains.
        count: usize,
    },
    /// The crate's two enumerations never agreed.
    EnumerationsDisagreed {
        /// How many attempts were made.
        ///
        /// `u32` because that is what `Coherence::Disagreed` carries; taking the
        /// upstream type rather than casting keeps this a copy of the value and
        /// not a conversion of it.
        attempts: u32,
        /// Processors seen only by the relationship walk.
        walk_only: usize,
        /// Processors seen only by CPU Sets.
        cpu_sets_only: usize,
    },
    /// The crate reports its coherence was never collected.
    CoherenceNotCollected,
}

impl ParseIncomplete {
    /// The stable discriminant a survey groups by.
    ///
    /// Changing one of these is a breaking change to the NDJSON row.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::EnumerationAnomalies { .. } => "enumeration_anomalies",
            Self::NumaDomainsOnlyInCpuSets { .. } => "numa_domains_only_in_cpu_sets",
            Self::NoCacheLevels => "no_cache_levels",
            Self::CacheLevelsWithoutPartitions { .. } => "cache_levels_without_partitions",
            Self::MeasuredButCountsAbsent { .. } => "measured_but_counts_absent",
            Self::NoPackages => "no_packages",
            Self::NoCores => "no_cores",
            Self::ContradictoryCores { .. } => "contradictory_cores",
            Self::UnnumberedCacheLevels { .. } => "unnumbered_cache_levels",
            Self::PartitioningSummaryMissing { .. } => "partitioning_summary_missing",
            Self::NotMeasured => "not_measured",
            Self::RelationsWithoutProcessors { .. } => "relations_without_processors",
            Self::UnreportedRelations { .. } => "unreported_relations",
            Self::DescribedRelations { .. } => "described_relations",
            Self::CoresOnlyInCpuSets { .. } => "cores_only_in_cpu_sets",
            Self::OverlappingWalkRelations { .. } => "overlapping_walk_relations",
            Self::ProcessorAttributeConflicts { .. } => "processor_attribute_conflicts",
            Self::NumaDomainsWithConflictingLabels { .. } => "numa_domains_with_conflicting_labels",
            Self::NumaDomainsUnreported { .. } => "numa_domains_unreported",
            Self::EnumerationsDisagreed { .. } => "enumerations_disagreed",
            Self::CoherenceNotCollected => "coherence_not_collected",
        }
    }
}

impl ParseIncomplete {
    /// This entry as the row publishes it, with the values its variant carries.
    #[must_use]
    pub fn published(&self) -> Value {
        let count = |value: &usize| vec![("count", Value::Number(*value))];
        let fields = match self {
            Self::EnumerationAnomalies { count: n }
            | Self::NumaDomainsOnlyInCpuSets { count: n }
            | Self::ContradictoryCores { count: n }
            | Self::UnnumberedCacheLevels { count: n }
            | Self::UnreportedRelations { count: n }
            | Self::DescribedRelations { count: n }
            | Self::CoresOnlyInCpuSets { count: n }
            | Self::OverlappingWalkRelations { count: n }
            | Self::ProcessorAttributeConflicts { count: n }
            | Self::NumaDomainsWithConflictingLabels { count: n }
            | Self::NumaDomainsUnreported { count: n } => count(n),
            Self::NoCacheLevels
            | Self::NoPackages
            | Self::NoCores
            | Self::NotMeasured
            | Self::CoherenceNotCollected => Vec::new(),
            Self::CacheLevelsWithoutPartitions { levels } => {
                vec![("levels", levels.iter().copied().collect())]
            }
            Self::MeasuredButCountsAbsent { absent } => {
                vec![("absent", absent.iter().copied().collect())]
            }
            Self::PartitioningSummaryMissing { level } => {
                vec![("level", Value::Number(*level as usize))]
            }
            Self::RelationsWithoutProcessors { cores, packages } => vec![
                ("cores", Value::Number(*cores)),
                ("packages", Value::Number(*packages)),
            ],
            Self::EnumerationsDisagreed {
                attempts,
                walk_only,
                cpu_sets_only,
            } => vec![
                ("attempts", Value::Number(*attempts as usize)),
                ("walk_only", Value::Number(*walk_only)),
                ("cpu_sets_only", Value::Number(*cpu_sets_only)),
            ],
        };

        entry(self.code(), fields)
    }
}

impl fmt::Display for ParseIncomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EnumerationAnomalies { count } => write!(
                f,
                "windows-topology-sys recorded {count} enumeration anomal{}, so what Windows \
                 returned was not fully decoded and the counts above may be short, or overstated \
                 where a record was kept with an incomplete processor set",
                if *count == 1 { "y" } else { "ies" },
            ),
            Self::NumaDomainsOnlyInCpuSets { count } => write!(
                f,
                "{count} NUMA domain(s) were reported only by CPU Sets and never by the \
                 relationship walk, so the two sources group nodes differently -- which no \
                 counter and no coherence check reaches, since coherence compares processor sets"
            ),
            Self::NoCacheLevels => f.write_str(
                "no cache levels were reported at all, so what divides this machine by cache was \
                 not established in either direction",
            ),
            Self::CacheLevelsWithoutPartitions { levels } => write!(
                f,
                "cache level(s) {levels:?} decoded to no partitions at all, so what divides this \
                 machine at those levels was not established"
            ),
            Self::MeasuredButCountsAbsent { absent } => write!(
                f,
                "this topology was measured from a running machine, which cannot have none, but \
                 it reported no {}",
                absent.join(" and "),
            ),
            Self::NoPackages => {
                f.write_str("no packages were reported at all, though the machine has one")
            }
            Self::NoCores => {
                f.write_str("no cores were reported at all, though the machine has one")
            }
            Self::ContradictoryCores { count } => write!(
                f,
                "{count} core(s) report an SMT flag that disagrees with the number of processors \
                 recorded beside it, so the record contradicts itself"
            ),
            Self::UnnumberedCacheLevels { count } => write!(
                f,
                "{count} cache level(s) are numbered 0, which is not a level Windows reports, so \
                 what they describe was not established"
            ),
            Self::PartitioningSummaryMissing { level } => write!(
                f,
                "L{level} was named as the outermost partitioning cache and this survey carries \
                 no summary for it, so what it divides was not established"
            ),
            Self::NotMeasured => f.write_str(
                "this topology was not measured from a running machine, so nothing here describes \
                 the host it is reported on",
            ),
            Self::RelationsWithoutProcessors { cores, packages } => write!(
                f,
                "{cores} core(s) and {packages} package(s) cover no processors, so they raise \
                 those counts and the policies derived from them without describing any part of \
                 the machine"
            ),
            Self::UnreportedRelations { count } => write!(
                f,
                "{count} relation(s) carry no observation from any source, so they are counted \
                 here without any platform API having described them"
            ),
            Self::DescribedRelations { count } => write!(
                f,
                "{count} relation(s) were described by a caller rather than reported by any \
                 platform API, so the counts above are not all of them measured"
            ),
            Self::CoresOnlyInCpuSets { count } => write!(
                f,
                "{count} core(s) were reported only by CPU Sets and never by the relationship \
                 walk, so the two group processors into cores differently and the core count \
                 above holds both groupings"
            ),
            Self::OverlappingWalkRelations { count } => write!(
                f,
                "{count} relation(s) reported by the relationship walk share a processor with \
                 another of the same kind, so one processor is claimed by two packages, two cores \
                 or two NUMA nodes and the counts above hold both"
            ),
            Self::ProcessorAttributeConflicts { count } => write!(
                f,
                "{count} per-processor attribute(s) carry more than one distinct value, so the \
                 efficiency classes above are one claim rather than an agreed one"
            ),
            Self::NumaDomainsWithConflictingLabels { count } => write!(
                f,
                "{count} NUMA domain(s) carry more than one distinct node number, so what node \
                 they are was not established and the highest below takes the larger"
            ),
            Self::NumaDomainsUnreported { count } => write!(
                f,
                "{count} NUMA domain(s) carry no observation from either source, so they raise \
                 the domain count while contributing no node number to compare"
            ),
            Self::EnumerationsDisagreed {
                attempts,
                walk_only,
                cpu_sets_only,
            } => write!(
                f,
                "windows-topology-sys reports its two enumerations never agreed within \
                 {attempts} attempt(s): {walk_only} processor(s) seen only by the relationship \
                 walk, {cpu_sets_only} seen only by CPU Sets and so absent from the parsed list \
                 entirely"
            ),
            Self::CoherenceNotCollected => f.write_str(
                "windows-topology-sys reports its coherence was never collected, so nothing \
                 established that its two enumerations describe the same machine",
            ),
        }
    }
}

/// The stable discriminant for an anomaly the enumeration recorded.
///
/// **`AnomalyKind` is `#[non_exhaustive]`, so this match needs a catch-all and
/// a variant added upstream lands in it.** That is stated rather than hidden:
/// `unclassified` is a real answer meaning "this probe's vocabulary is older
/// than the crate's", which is more useful to a survey than a code invented
/// here that pretends to name the new kind. The row publishes it, so a sweep
/// over accumulated output finds the day the vocabularies parted rather than
/// silently mislabelling the anomaly.
///
/// It is deliberately NOT a compile error. Owning the row's vocabulary means
/// this crate decides when a new upstream kind earns a code, and a build that
/// breaks on a dependency bump would force that decision at the worst moment.
#[must_use]
pub fn anomaly_code(anomaly: &EnumerationAnomaly) -> &'static str {
    match anomaly.kind {
        AnomalyKind::Undersized { .. } => "undersized",
        AnomalyKind::OverrunsBuffer { .. } => "overruns_buffer",
        AnomalyKind::TrailingBytes { .. } => "trailing_bytes",
        AnomalyKind::TruncatedArray { .. } => "truncated_array",
        _ => "unclassified",
    }
}

/// An anomaly as the row publishes it.
///
/// Carries WHERE as well as what: the enumeration it was reading and the byte
/// offset it stopped at. A survey grouping by kind across a fleet wants both --
/// the same kind at the same offset on many hosts is a different finding from
/// the same kind scattered, and neither is visible from a count.
///
/// `AnomalyKind` is `#[non_exhaustive]`, so a kind added upstream lands in
/// `unclassified` and publishes no fields rather than a guess at which ones it
/// has. That is the honest rendering: the code says the vocabulary is older than
/// the crate, and inventing fields for it would say more than is known.
#[must_use]
pub fn published_anomaly(anomaly: &EnumerationAnomaly) -> Value {
    let source = match anomaly.source {
        Source::RelationshipWalk => "relationship_walk",
        Source::CpuSets => "cpu_sets",
        _ => "unclassified",
    };

    let mut fields = vec![
        ("source", Value::Text(source.to_owned())),
        ("offset", Value::Number(anomaly.offset)),
    ];

    fields.extend(match anomaly.kind {
        AnomalyKind::Undersized { declared, minimum } => vec![
            ("declared", Value::Number(declared)),
            ("minimum", Value::Number(minimum)),
        ],
        AnomalyKind::OverrunsBuffer {
            declared,
            remaining,
        } => vec![
            ("declared", Value::Number(declared)),
            ("remaining", Value::Number(remaining)),
        ],
        AnomalyKind::TrailingBytes { remaining } => {
            vec![("remaining", Value::Number(remaining))]
        }
        AnomalyKind::TruncatedArray { declared, decoded } => vec![
            ("declared", Value::Number(declared)),
            ("decoded", Value::Number(decoded)),
        ],
        _ => Vec::new(),
    });

    entry(anomaly_code(anomaly), fields)
}
