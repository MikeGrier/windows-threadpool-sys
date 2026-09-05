// Copyright (c) 2026 Mike Grier
//! The topology description: an open-kinded domain over a set of processors.
//!
//! This is the "interpretation" layer built on top of `relation.rs`'s
//! faithful, Win32-shaped records (D-2 in `DESIGN-NOTES.md`). Everything here
//! is plain data with no Win32 dependency, so it can be built either by
//! discovering the running system or entirely by hand -- or fed in from
//! elsewhere, which is this crate's whole reason for separating discovery
//! from description.

use std::collections::BTreeMap;

use crate::CacheKind;
use crate::observation::{Observation, Source};
use crate::observed::Observed;
use crate::processor_set::ProcessorSet;

/// The identity of one logical processor: its group and its number within
/// that group.
///
/// Never flattened to a single index (D-7 in `DESIGN-NOTES.md`): a Windows
/// thread's affinity is a `GROUP_AFFINITY`, one group and a bitmask within
/// it, so the group is a hard boundary a flattened index would lose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ProcessorId {
    /// The processor group.
    pub group: u16,
    /// The processor's number within that group (0..`MAX_PROCESSORS_PER_GROUP`,
    /// i.e. 0..`usize::BITS`).
    pub number: u8,
}

/// One logical processor.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Processor {
    /// This processor's identity.
    pub id: ProcessorId,
    /// Whether the processor is currently active. A processor slot can exist
    /// -- count toward a group's maximum -- without being online, since
    /// Windows reserves group capacity for processors that may be added
    /// later.
    pub online: bool,
    /// A relative scheduling weight for this processor, higher meaning more
    /// capable, on no fixed scale. On Windows this is the owning core's raw
    /// `EfficiencyClass`; `0` for an offline processor or one with no known
    /// owning core.
    pub capacity: u32,
}

/// What a [`Domain`] represents.
///
/// Open-kinded rather than a fixed enumeration (D-4 in `DESIGN-NOTES.md`):
/// Linux alone models `die`, `cluster`, and (on `s390x`) `book` and `drawer`,
/// none of which Windows reports and none of which a cache domain reliably
/// approximates. Enumerating every level any architecture will ever have is a
/// losing game, so an unrecognised kind is carried in [`DomainKind::Other`]
/// rather than rejected.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DomainKind {
    /// A Windows processor group.
    Group,
    /// A physical processor package (socket).
    Package,
    /// A processor die, on systems that report the distinction from package.
    Die,
    /// A processor module, on systems that report it.
    Module,
    /// A physical core: one or more logical processors as SMT siblings.
    Core {
        /// Whether this core has more than one logical processor.
        simultaneous_multithreading: bool,
        /// The scheduler's raw Windows efficiency class for this core.
        efficiency_class: u8,
    },
    /// A cache level shared by a set of logical processors.
    Cache {
        /// Cache level (1, 2, 3, ...).
        level: u8,
        /// Associativity Windows reported, or `0xFF` for fully associative.
        associativity: u8,
        /// Cache line size in bytes.
        line_size: u16,
        /// Total cache size in bytes.
        size_bytes: u32,
        /// What the cache holds.
        cache_type: CacheKind,
    },
    /// A memory locality domain -- a NUMA node modelled as a memory domain
    /// that may contain no processors at all (D-5), because CXL expanders,
    /// persistent memory, HBM tiers, and coherent GPU memory all present that
    /// way.
    Memory {
        /// The domain's memory capacity.
        ///
        /// [`Observed::NotObserved`] from `discover`: Windows's own enumeration
        /// (`GetLogicalProcessorInformationEx`) does not report a NUMA node's
        /// capacity at all, so this crate never learns it. A hand-written or
        /// fed-in description may supply one.
        ///
        /// # Why this is an `Observed` and not an `Option`
        ///
        /// So that a description **omitting** the field and one writing
        /// `"memory_bytes": null` stop being the same value: the first is
        /// `NotObserved` (nobody addressed it) and the second is `Absent` (the
        /// writer addressed it and had no value). Both still mean "the capacity
        /// is unknown" to a planner, so the distinction is one of **provenance
        /// of the description**, not of planning -- recorded plainly because
        /// `M5+.2` framed it as more than that.
        ///
        /// The distinction that actually matters was already available and is
        /// preserved: `Known(0)` is a node with genuinely no memory -- the
        /// CXL-expander shape [D-5](../DESIGN-NOTES.md) exists to represent --
        /// and is not confusable with an unknown capacity. That is
        /// [D-11](../DESIGN-NOTES.md)'s point, and it is why `Some(0)` was
        /// rejected as a stand-in in the first place.
        memory_bytes: Observed<u64>,
    },
    /// A domain kind this crate does not have a name for, carrying its raw
    /// name and whatever attributes came with it, so a description this
    /// crate cannot fully interpret still round-trips losslessly.
    Other {
        /// The raw kind name.
        name: String,
        /// Whatever attributes accompanied this domain, beyond `id` and
        /// `processors`.
        attributes: BTreeMap<String, AttributeValue>,
    },
}

/// A plain value for an unrecognised [`DomainKind::Other`]'s attributes.
///
/// Deliberately not `serde_json::Value`: this crate depends only on `serde`'s
/// traits, optionally, following `windows-file-watcher`'s D-72 precedent --
/// never on a specific format crate. A consumer chooses their own serializer.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// A JSON `null`.
    Null,
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number that decoded as a non-negative integer, preserved
    /// exactly (PR #20 review response). Collapsing every number to `f64`
    /// loses precision above 2^53 (~9 PB), silently corrupting both the
    /// documented lossless round-trip for an unknown attribute and known
    /// numeric field decoding (e.g. `memory_bytes`).
    UnsignedInteger(u64),
    /// A JSON number that decoded as a negative integer, preserved exactly.
    SignedInteger(i64),
    /// A JSON number with a fractional part (or one too large for either
    /// integer variant), represented as `f64`.
    Float(f64),
    /// A JSON string.
    String(String),
    /// A JSON array.
    Array(Vec<AttributeValue>),
    /// A JSON object.
    Object(BTreeMap<String, AttributeValue>),
}

/// One domain: a named relationship over a set of processors.
///
/// Domains reference processors; they do not nest (D-6 in `DESIGN-NOTES.md`).
/// No hierarchy is imposed, so this type never asserts that a package
/// contains a node or that a node contains a cache -- chiplets and CXL
/// already violate assumptions like that, and Linux's own levels do not form
/// a strict hierarchy either.
///
/// With the `serde` feature, serializes as `{"kind": <string>, "processors":
/// [...], ...fields specific to that kind}` -- an internally tagged shape,
/// implemented by hand rather than derived, because `kind` is open (D-4) and
/// an unrecognised one must still round-trip its attributes.
///
/// An `"id"` is written when the relationship walk labelled this relation, and
/// is **optional on read and discarded**. It carries no model meaning: a label
/// belongs to the observation that issued it (D-15), and a file cannot
/// establish which source observed anything (D-12).
///
/// **This JSON shape is explicitly not covered by this crate's semver
/// contract (D-8 in `DESIGN-NOTES.md`).** The Rust API above (`Domain`,
/// `DomainKind`, and friends) is covered by semver as always; the wire shape
/// they produce and accept is allowed to change in a minor release. This is
/// what makes D-9's deferrals (an HMAT-style attributed-relation model,
/// devices as topology participants) safe to defer rather than merely
/// convenient: adding them later is a schema evolution, not a breaking
/// change to a promise this crate never made.
#[derive(Clone, Debug, PartialEq)]
pub struct Domain {
    /// What this domain represents.
    pub kind: DomainKind,

    /// The logical processors this domain covers. Empty for a memory-only
    /// domain (D-5).
    pub processors: ProcessorSet,
    /// Which sources reported this relation, and what each called it.
    ///
    /// Empty for a relation nobody reported -- one built by hand, which is
    /// honest rather than a gap: no platform API said anything about it. A
    /// relation both Windows APIs describe carries **two** observations, which
    /// is what makes agreement visible without either label being discarded
    /// (D-15, D-19).
    pub observations: Vec<Observation>,
}

impl Domain {
    /// What `source` called this relation, if `source` reported it.
    ///
    /// The replacement for the removed `id` field, and it takes a source
    /// because there is no single answer without one: the two Windows APIs
    /// agree on the core partition while labelling it `[0, 2, 4, ..., 14]` and
    /// `[0, 1, ..., 7]`, so "the id" was never well defined once both were
    /// consulted (D-15).
    ///
    /// A caller wanting a *stable* handle for grouping should use the
    /// relation's position in [`MachineMemoryTopology::domains`][domains]
    /// instead: an observation label is meaningful only to the source that
    /// issued it, and a relation may have none at all.
    ///
    /// [domains]: crate::MachineMemoryTopology::domains
    #[must_use]
    pub fn label_from(&self, source: Source) -> Option<u32> {
        self.observations
            .iter()
            .find(|observation| observation.source == source)
            .map(|observation| observation.label)
    }

    /// Whether `source` reported this relation.
    #[must_use]
    pub fn observed_by(&self, source: Source) -> bool {
        self.observations
            .iter()
            .any(|observation| observation.source == source)
    }
}

/// Manual `Serialize`/`Deserialize` for the open-kinded types.
///
/// `AttributeValue` and `Domain` cannot be `#[derive(Serialize, Deserialize)]`:
/// `AttributeValue` is a self-describing value (its own shape is not known
/// until read), and `Domain`'s wire shape is internally tagged on `kind` with
/// the kind-specific fields flattened alongside `id`/`processors` -- and an
/// unrecognised `kind` must still capture whatever fields came with it (D-4).
/// Both are read by buffering the whole object into a
/// `BTreeMap<String, AttributeValue>` first, since which fields are expected
/// is only known once `kind` itself has been read, and a hand-written or
/// fed-in description cannot be relied on to place `kind` first.
#[cfg(feature = "serde")]
mod serde_impl {
    use std::collections::BTreeMap;
    use std::fmt;

    use serde::de::{Error, MapAccess, SeqAccess, Visitor};
    use serde::ser::{Error as SerError, SerializeMap};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{AttributeValue, Domain, DomainKind};
    use crate::CacheKind;
    use crate::processor_set::MAX_PROCESSORS_PER_GROUP;
    use crate::processor_set::ProcessorSet;

    impl Serialize for AttributeValue {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            match self {
                AttributeValue::Null => serializer.serialize_unit(),
                AttributeValue::Bool(b) => serializer.serialize_bool(*b),
                AttributeValue::UnsignedInteger(n) => serializer.serialize_u64(*n),
                AttributeValue::SignedInteger(n) => serializer.serialize_i64(*n),
                AttributeValue::Float(n) => serializer.serialize_f64(*n),
                AttributeValue::String(s) => serializer.serialize_str(s),
                AttributeValue::Array(items) => items.serialize(serializer),
                AttributeValue::Object(map) => map.serialize(serializer),
            }
        }
    }

    impl<'de> Deserialize<'de> for AttributeValue {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct ValueVisitor;

            impl<'de> Visitor<'de> for ValueVisitor {
                type Value = AttributeValue;

                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    f.write_str("a JSON-like value")
                }
                fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
                    Ok(AttributeValue::Null)
                }
                fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
                    Ok(AttributeValue::Null)
                }
                fn visit_bool<E: Error>(self, v: bool) -> Result<Self::Value, E> {
                    Ok(AttributeValue::Bool(v))
                }
                fn visit_i64<E: Error>(self, v: i64) -> Result<Self::Value, E> {
                    Ok(AttributeValue::SignedInteger(v))
                }
                fn visit_u64<E: Error>(self, v: u64) -> Result<Self::Value, E> {
                    Ok(AttributeValue::UnsignedInteger(v))
                }
                fn visit_f64<E: Error>(self, v: f64) -> Result<Self::Value, E> {
                    Ok(AttributeValue::Float(v))
                }
                fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
                    Ok(AttributeValue::String(v.to_string()))
                }
                fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
                    Ok(AttributeValue::String(v))
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                    let mut items = Vec::new();
                    while let Some(item) = seq.next_element()? {
                        items.push(item);
                    }
                    Ok(AttributeValue::Array(items))
                }
                fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                    let mut object = BTreeMap::new();
                    while let Some((k, v)) = map.next_entry()? {
                        object.insert(k, v);
                    }
                    Ok(AttributeValue::Object(object))
                }
            }

            deserializer.deserialize_any(ValueVisitor)
        }
    }

    fn as_bool<E: Error>(value: AttributeValue) -> Result<bool, E> {
        match value {
            AttributeValue::Bool(b) => Ok(b),
            _ => Err(E::custom("expected a boolean")),
        }
    }

    /// A number is preserved exactly when it decoded as an integer variant;
    /// only a [`AttributeValue::Float`] (a fractional source, or one too
    /// large for `i64`/`u64`) is converted, and even then only when it is a
    /// non-negative whole number representable in `f64` (PR #20 review
    /// response: the same ~9 PB ceiling D-11 already accepts for
    /// `memory_bytes`, not the silent precision loss the old
    /// `f64`-for-everything encoding had).
    fn as_u64<E: Error>(value: AttributeValue) -> Result<u64, E> {
        match value {
            AttributeValue::UnsignedInteger(n) => Ok(n),
            AttributeValue::SignedInteger(n) => {
                u64::try_from(n).map_err(|_| E::custom("expected a non-negative whole number"))
            }
            // **Exclusive at the top, and that is not a style choice.**
            // `u64::MAX as f64` rounds *up* to 2^64, which is one greater than
            // any `u64`. An inclusive bound therefore admitted exactly 2^64,
            // and `n as u64` saturates it to `u64::MAX` -- so a description
            // carrying 18446744073709551616 was silently read as a different
            // number. Excluding the bound rejects it instead, and costs
            // nothing: the largest `f64` below 2^64 is 2^64 - 2048, which is a
            // representable `u64` and still accepted. Raised in the PR #56
            // review.
            AttributeValue::Float(n) if n.fract() == 0.0 && (0.0..u64::MAX as f64).contains(&n) => {
                Ok(n as u64)
            }
            _ => Err(E::custom("expected a non-negative whole number")),
        }
    }

    fn as_u32<E: Error>(value: AttributeValue) -> Result<u32, E> {
        u32::try_from(as_u64(value)?).map_err(|_| E::custom("number is too large for this field"))
    }

    /// As [`as_u64`], but over the signed range: `CacheKind::Other` carries a
    /// raw `PROCESSOR_CACHE_TYPE`, a C enum backed by `i32`, which is not
    /// guaranteed non-negative. Preserved exactly for either integer variant;
    /// only a [`AttributeValue::Float`] is converted, subject to the same
    /// fractional/range check as before.
    fn as_i64<E: Error>(value: AttributeValue) -> Result<i64, E> {
        match value {
            AttributeValue::SignedInteger(n) => Ok(n),
            AttributeValue::UnsignedInteger(n) => {
                i64::try_from(n).map_err(|_| E::custom("expected a whole number"))
            }
            // Exclusive at the top for the reason `as_u64` gives, and found by
            // sweeping for the same shape rather than reported: `i64::MAX as
            // f64` rounds up to 2^63, which no `i64` can hold, and the cast
            // would saturate it to `i64::MAX`. The *lower* bound stays
            // inclusive because `i64::MIN as f64` is -2^63 exactly -- it is a
            // power of two and representable, so it converts back losslessly.
            AttributeValue::Float(n)
                if n.fract() == 0.0 && (i64::MIN as f64..i64::MAX as f64).contains(&n) =>
            {
                Ok(n as i64)
            }
            _ => Err(E::custom("expected a whole number")),
        }
    }

    fn as_i32<E: Error>(value: AttributeValue) -> Result<i32, E> {
        i32::try_from(as_i64(value)?).map_err(|_| E::custom("number is too large for this field"))
    }

    fn as_u16<E: Error>(value: AttributeValue) -> Result<u16, E> {
        u16::try_from(as_u64(value)?).map_err(|_| E::custom("number is too large for this field"))
    }

    fn as_u8<E: Error>(value: AttributeValue) -> Result<u8, E> {
        u8::try_from(as_u64(value)?).map_err(|_| E::custom("number is too large for this field"))
    }

    /// `CacheKind`'s own derived `Serialize` produces a bare string for a
    /// unit variant and `{"other": <number>}` for `Other`; decoded here by
    /// hand against the already-buffered value rather than by re-entering
    /// serde's enum machinery, which expects a real `Deserializer` rather
    /// than a value already read into an [`AttributeValue`].
    fn cache_kind_from_value<E: Error>(value: AttributeValue) -> Result<CacheKind, E> {
        match value {
            AttributeValue::String(s) => match s.as_str() {
                "unified" => Ok(CacheKind::Unified),
                "instruction" => Ok(CacheKind::Instruction),
                "data" => Ok(CacheKind::Data),
                "trace" => Ok(CacheKind::Trace),
                other => Err(E::custom(format!("unrecognised cache_type \"{other}\""))),
            },
            AttributeValue::Object(mut map) if map.len() == 1 => match map.remove("other") {
                Some(raw) => Ok(CacheKind::Other(as_i32(raw)?)),
                None => Err(E::custom("cache_type object must have an \"other\" key")),
            },
            _ => Err(E::custom(
                "cache_type must be a string or {\"other\": <number>}",
            )),
        }
    }

    /// Convert a buffered `"processors"` value (a JSON array of
    /// `{"group":_,"number":_}` objects) into a [`ProcessorSet`], without
    /// going through `ProcessorSet`'s own `Deserialize` impl: that impl
    /// expects a real `Deserializer`, and this value has already been
    /// buffered into an [`AttributeValue`].
    fn processors_from_value<E: Error>(value: AttributeValue) -> Result<ProcessorSet, E> {
        let AttributeValue::Array(items) = value else {
            return Err(E::custom("domain \"processors\" must be an array"));
        };
        let mut set = ProcessorSet::empty();
        for item in items {
            let AttributeValue::Object(mut object) = item else {
                return Err(E::custom(
                    "each processor must be an object with \"group\" and \"number\"",
                ));
            };
            let group = as_u16(take(&mut object, "group")?)?;
            let number = as_u8(take(&mut object, "number")?)?;
            if u32::from(number) >= MAX_PROCESSORS_PER_GROUP {
                // A real Windows GROUP_AFFINITY.Mask is one machine word, so
                // a group genuinely cannot hold this processor: reject rather
                // than silently truncate (D-10 in `DESIGN-NOTES.md`).
                return Err(E::custom(format!(
                    "processor number {number} is out of range: a processor group has at most \
                     {MAX_PROCESSORS_PER_GROUP} processors on this platform"
                )));
            }
            set.insert(group, number);
        }
        Ok(set)
    }

    fn take<E: Error>(
        fields: &mut BTreeMap<String, AttributeValue>,
        key: &str,
    ) -> Result<AttributeValue, E> {
        fields
            .remove(key)
            .ok_or_else(|| E::custom(format!("domain is missing required field \"{key}\"")))
    }

    /// Every kind name this crate decodes into a named [`DomainKind`].
    ///
    /// **Must list exactly the arms the deserializer matches before its
    /// `other =>` fallback**, and `every_well_known_name_decodes_to_a_named_kind`
    /// asserts that it does rather than leaving the two to drift.
    pub(super) const WELL_KNOWN_KIND_NAMES: &[&str] = &[
        "group", "package", "die", "module", "core", "cache", "memory",
    ];

    impl Serialize for Domain {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let mut map = serializer.serialize_map(None)?;
            let kind_name: &str = match &self.kind {
                DomainKind::Group => "group",
                DomainKind::Package => "package",
                DomainKind::Die => "die",
                DomainKind::Module => "module",
                DomainKind::Core { .. } => "core",
                DomainKind::Cache { .. } => "cache",
                DomainKind::Memory { .. } => "memory",
                DomainKind::Other { name, .. } => {
                    // The same rule as the attribute-name collision check
                    // below, for the same reason and by the same remedy.
                    //
                    // `Other` exists so a description this crate cannot fully
                    // interpret "still round-trips losslessly", and a name this
                    // crate *does* interpret breaks exactly that: the document
                    // would say `"kind": "group"`, and reading it back yields
                    // `DomainKind::Group`, not the `Other` that was written.
                    // `Group`, `Package`, `Die` and `Module` carry no fields,
                    // so that substitution succeeds silently and the attributes
                    // are dropped on the floor; `core`, `cache` and `memory`
                    // fail loudly on a missing field, or -- worse -- succeed as
                    // a different kind when the attributes happen to match.
                    //
                    // Refused rather than escaped or renamed, because both of
                    // those would change a name the caller chose. Raised in the
                    // PR #56 review.
                    if WELL_KNOWN_KIND_NAMES.contains(&name.as_str()) {
                        return Err(S::Error::custom(format!(
                            "domain kind name \"{name}\" collides with a kind this crate names \
                             itself, so the document would deserialize as that kind rather than \
                             as `Other`"
                        )));
                    }
                    name.as_str()
                }
            };
            map.serialize_entry("kind", kind_name)?;
            // The wire shape keeps an "id" because a description is written by
            // hand and a human-meaningful number is worth having. It is the
            // relationship walk's label where there is one -- not "the id",
            // which stopped being well defined once two sources labelled the
            // same relation differently (D-15).
            if let Some(label) = self.label_from(crate::observation::Source::RelationshipWalk) {
                map.serialize_entry("id", &label)?;
            }
            map.serialize_entry("processors", &self.processors)?;
            match &self.kind {
                DomainKind::Core {
                    simultaneous_multithreading,
                    efficiency_class,
                } => {
                    map.serialize_entry(
                        "simultaneous_multithreading",
                        simultaneous_multithreading,
                    )?;
                    map.serialize_entry("efficiency_class", efficiency_class)?;
                }
                DomainKind::Cache {
                    level,
                    associativity,
                    line_size,
                    size_bytes,
                    cache_type,
                } => {
                    map.serialize_entry("level", level)?;
                    map.serialize_entry("associativity", associativity)?;
                    map.serialize_entry("line_size", line_size)?;
                    map.serialize_entry("size_bytes", size_bytes)?;
                    map.serialize_entry("cache_type", cache_type)?;
                }
                DomainKind::Memory { memory_bytes } => {
                    // Three states, three wire shapes: a number, an explicit
                    // `null` for "addressed and unknown", and omission for
                    // "nobody said". Writing `null` for both would put the
                    // ambiguity back on the wire that the type just removed.
                    match memory_bytes {
                        crate::observed::Observed::Known(bytes) => {
                            map.serialize_entry("memory_bytes", bytes)?;
                        }
                        crate::observed::Observed::Absent => {
                            map.serialize_entry("memory_bytes", &Option::<u64>::None)?;
                        }
                        crate::observed::Observed::NotObserved => {}
                    }
                }
                DomainKind::Other { attributes, .. } => {
                    for (key, value) in attributes {
                        if matches!(key.as_str(), "kind" | "id" | "processors") {
                            // These three keys are always written above;
                            // silently letting an attribute reuse one would
                            // corrupt the wire shape rather than round-trip.
                            return Err(S::Error::custom(format!(
                                "domain attribute \"{key}\" collides with the reserved \
                                 \"kind\"/\"id\"/\"processors\" field names"
                            )));
                        }
                        map.serialize_entry(key, value)?;
                    }
                }
                DomainKind::Group | DomainKind::Package | DomainKind::Die | DomainKind::Module => {}
            }
            map.end()
        }
    }

    impl<'de> Deserialize<'de> for Domain {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let mut fields = BTreeMap::<String, AttributeValue>::deserialize(deserializer)?;

            let kind_name = match take::<D::Error>(&mut fields, "kind")? {
                AttributeValue::String(s) => s,
                _ => return Err(D::Error::custom("domain \"kind\" must be a string")),
            };
            // Read and discarded, and OPTIONAL. The wire "id" is one source's
            // label; a file cannot establish which source observed the relation
            // (D-12), so it never becomes an observation. Since it carries no
            // model meaning, requiring it would reject a description that is
            // otherwise complete -- including one this crate itself writes for a
            // relation no source labelled.
            let _ = fields.remove("id");
            let processors = processors_from_value(take::<D::Error>(&mut fields, "processors")?)?;
            // A described relation carries NO platform observation, and the
            // wire shape does not encode them.
            //
            // This is the same downgrade `Provenance::downgraded_to` performs
            // one level up (D-12): a file saying "the relationship walk
            // observed this" cannot establish that it did, so the claim is not
            // carried across the boundary. Serializing observations faithfully
            // would carry exactly the claim D-12 refuses.
            //
            // Nor is a `Source::Description` observation synthesized here. That
            // would restate what the object already says -- a deserialized
            // topology's `Provenance` is capped at `Restored` -- and D-22 has
            // just established these two are different questions that should
            // not duplicate each other.
            let observations = Vec::new();

            let kind = match kind_name.as_str() {
                "group" => DomainKind::Group,
                "package" => DomainKind::Package,
                "die" => DomainKind::Die,
                "module" => DomainKind::Module,
                "core" => DomainKind::Core {
                    simultaneous_multithreading: as_bool(take(
                        &mut fields,
                        "simultaneous_multithreading",
                    )?)?,
                    efficiency_class: as_u8(take(&mut fields, "efficiency_class")?)?,
                },
                "cache" => DomainKind::Cache {
                    level: as_u8(take(&mut fields, "level")?)?,
                    associativity: as_u8(take(&mut fields, "associativity")?)?,
                    line_size: as_u16(take(&mut fields, "line_size")?)?,
                    size_bytes: as_u32(take(&mut fields, "size_bytes")?)?,
                    cache_type: cache_kind_from_value(take(&mut fields, "cache_type")?)?,
                },
                "memory" => DomainKind::Memory {
                    memory_bytes: match fields.remove("memory_bytes") {
                        None => crate::observed::Observed::NotObserved,
                        Some(AttributeValue::Null) => crate::observed::Observed::Absent,
                        Some(value) => crate::observed::Observed::Known(as_u64(value)?),
                    },
                },
                // Every arm above must appear in `WELL_KNOWN_KIND_NAMES`, or
                // `Serialize` would let an `Other` claim that name and the
                // document would decode as the named kind instead. The test
                // named on that constant is what enforces it.
                other => DomainKind::Other {
                    name: other.to_string(),
                    attributes: fields,
                },
            };

            Ok(Domain {
                kind,
                processors,
                observations,
            })
        }
    }
}

/// Everything this crate knows about one processor, with each absence named.
///
/// Produced by [`MachineMemoryTopology::shard_set`](crate::MachineMemoryTopology::shard_set). Deliberately **not** a
/// second copy of [`Processor`]: that type is the platform's own record, while
/// this is the assembled answer to "may this processor host work, and where
/// does it allocate from" -- gathered from both Win32 sources plus the derived
/// relations.
///
/// # No sentinels, anywhere
///
/// Every optional field is an [`Observed`], so "the platform said zero" and
/// "nobody asked" are different values rather than the same one. The field
/// this exists to replace, [`Processor::capacity`], spells three facts as `0`
/// -- offline, in no core, and efficiency class zero -- and the third is every
/// processor on every non-hybrid machine.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessorFacts<'a> {
    /// The processor's identity, always `(group, number)` and never flattened
    /// (D-7): a Windows affinity is a `GROUP_AFFINITY`, so a bare index names a
    /// different processor in every group.
    pub id: ProcessorId,
    /// Whether the slot is active. An offline slot exists and counts toward its
    /// group's maximum, so planning work onto one is planning a thread that
    /// cannot run.
    pub online: bool,
    /// The core relation this processor belongs to, if any names it.
    ///
    /// `None` is a firmware gap the crate tolerates by design, not a
    /// contradiction -- see [`Self::efficiency_class`], which is
    /// [`Observed::NotObserved`] in exactly that case rather than `0`.
    pub core: Option<&'a Domain>,
    /// Whether the owning core has more than one logical processor.
    pub simultaneous_multithreading: Observed<bool>,
    /// The scheduler's efficiency class for the owning core.
    ///
    /// [`Observed::NotObserved`] when no core names this processor -- which is
    /// the distinction `Processor::capacity` cannot make. On a hybrid part
    /// Windows orders class `0` as the *least* performant, so an unknown
    /// processor reported as `0` is indistinguishable from an efficiency core:
    /// a policy excluding efficiency cores silently drops a possible
    /// performance core, and one tiering them mis-tiers it. Neither fails a
    /// functional test.
    pub efficiency_class: Observed<u8>,
    /// Whether the scheduler is currently avoiding this processor.
    ///
    /// [`Observed::NotObserved`] when the CPU-set enumeration was not
    /// consulted, which is any topology not produced by
    /// [`MachineMemoryTopology::discover`](crate::MachineMemoryTopology::discover). Parked is **not** offline: the
    /// processor is active and the scheduler is merely avoiding it.
    pub parked: Observed<bool>,
    /// Whether this processor is allocated to *this* process.
    ///
    /// A planner ignoring it places work on processors the process may not use,
    /// which is a wrong plan rather than a slow one.
    ///
    /// **Measured to carry no information on Windows 11 25H2** (D-23): the
    /// whole `AllFlags` byte reads `0x00` for every processor even after CPU
    /// sets are successfully allocated to this process, so `Known(false)` here
    /// reports a byte the kernel did not write.
    ///
    /// It also does not mean what its name suggests -- allocation is the
    /// explicit `SetProcessDefaultCpuSets` kind, not "may we run here". Do not
    /// branch on it; see [`CpuSet::allocated_to_target_process`](crate::CpuSet::allocated_to_target_process).
    pub allocated_to_this_process: Observed<bool>,
    /// The memory domain this processor allocates from, or
    /// [`Observed::NotObserved`] for the **unplaced** case, which has no honest
    /// fallback -- see [`MachineMemoryTopology::memory_domain_of`](crate::MachineMemoryTopology::memory_domain_of).
    pub memory_domain: Observed<&'a Domain>,
}

#[cfg(test)]
mod tests;
