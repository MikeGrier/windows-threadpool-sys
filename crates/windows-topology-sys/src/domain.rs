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
    /// way. `memory_bytes` is `None` when the size is not known: Windows's
    /// own enumeration (`GetLogicalProcessorInformationEx`) does not report a
    /// NUMA node's capacity at all, so a domain discovered by this crate
    /// always has `memory_bytes: None`; a hand-written or fed-in description
    /// may supply it.
    Memory {
        /// The domain's memory capacity, if known.
        memory_bytes: Option<u64>,
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
/// With the `serde` feature, serializes as `{"kind": <string>, "id": <u32>,
/// "processors": [...], ...fields specific to that kind}` -- an internally
/// tagged shape, implemented by hand rather than derived, because `kind` is
/// open (D-4) and an unrecognised one must still round-trip its attributes.
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
    /// An identifier for this domain, unique among domains of the same
    /// `kind`. Where Windows reports a natural number (a NUMA node number, a
    /// group number) that number is used; otherwise domains are numbered in
    /// the order they were discovered.
    pub id: u32,
    /// The logical processors this domain covers. Empty for a memory-only
    /// domain (D-5).
    pub processors: ProcessorSet,
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
            AttributeValue::Float(n)
                if n.fract() == 0.0 && (0.0..=u64::MAX as f64).contains(&n) =>
            {
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
            AttributeValue::Float(n)
                if n.fract() == 0.0 && (i64::MIN as f64..=i64::MAX as f64).contains(&n) =>
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
                DomainKind::Other { name, .. } => name.as_str(),
            };
            map.serialize_entry("kind", kind_name)?;
            map.serialize_entry("id", &self.id)?;
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
                    if let Some(bytes) = memory_bytes {
                        map.serialize_entry("memory_bytes", bytes)?;
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
            let id = as_u32(take::<D::Error>(&mut fields, "id")?)?;
            let processors = processors_from_value(take::<D::Error>(&mut fields, "processors")?)?;

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
                        None | Some(AttributeValue::Null) => None,
                        Some(value) => Some(as_u64(value)?),
                    },
                },
                other => DomainKind::Other {
                    name: other.to_string(),
                    attributes: fields,
                },
            };

            Ok(Domain {
                kind,
                id,
                processors,
            })
        }
    }
}

#[cfg(test)]
mod tests;
