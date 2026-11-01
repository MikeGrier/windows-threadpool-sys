// Copyright (c) 2026 Mike Grier
// Split from dioring.rs at f13cd814.

use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::vec::Vec;

use fixed_hash_table::{Full, HeapMap};

use crate::alloc_tags::{tag, tagged};
use crate::clock::Instant;
use crate::contract::EpochId;
use crate::domain_lists::DomainLists;
use crate::durability::{Reach, Routing, State};
use crate::ids::{DioringIds, Lineage};
use crate::types::{EpochState, FlushDomain, ImportScope, UnknownLineage};

/// A flush domain interned by this instance: a word-sized stand-in for its bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct DomainId(pub(crate) u32);

/// Who answers for a file's writes (DI-D-27): the built-in default for a file with no declared
/// domain or one the provider does not serve, and the provider for the domains it serves.
pub(crate) fn routing(file: &[DomainId], served: &[DomainId]) -> Routing {
    if file.is_empty() {
        return Routing {
            default: true,
            provider: false,
        };
    }
    Routing {
        default: file.iter().any(|domain| !served.contains(domain)),
        provider: file.iter().any(|domain| served.contains(domain)),
    }
}

/// What an import reaches: the files -- `None` for the domains an [`ImportScope::Domains`] names,
/// whose list [`import_list`] makes once the import is admitted -- and the one lineage it is
/// confined to if any.
pub(crate) type ImportReach = (Option<Reach>, Option<u64>);

/// The targets an imported failure reaches (DI-D-21), and the one lineage it is confined to, if
/// the scope names one. A domain no target was declared with reaches only the targets declared with
/// none. `live` says whether a lineage is the instance's own and live.
pub(crate) fn import_reach<E: EpochId + 'static, I: Instant>(
    scope: &ImportScope<DioringIds<E, I>>,
    live: impl Fn(Lineage) -> bool,
) -> Result<ImportReach, UnknownLineage<DioringIds<E, I>>> {
    Ok(match scope {
        ImportScope::All => (Some(Reach::All), None),
        ImportScope::Lineage(lineage) if live(*lineage) => (Some(Reach::All), Some(lineage.seq)),
        ImportScope::Lineage(lineage) => return Err(UnknownLineage(*lineage)),
        ImportScope::Domains(_) => (None, None),
    })
}

/// Give `visit` each of `names` the instance has never been given -- by a target or its provider
/// -- in the order named (DI-D-46): a misspelling, or a target not given yet.
pub(crate) fn unmatched(
    names: &[FlushDomain],
    domains: &HeapMap<FlushDomain, DomainId>,
    mut visit: impl FnMut(&FlushDomain),
) {
    for name in names.iter().filter(|name| !domains.contains_key(*name)) {
        visit(name);
    }
}

/// The reach of an import naming `names`: the domains of them the instance was given, in a list of
/// the failure's own (DI-D-108), let go when it is resolved.
///
/// # Errors
///
/// What is inconsistent: no list is free, which the imports admitted rule out.
pub(crate) fn import_list(
    names: &[FlushDomain],
    domains: &HeapMap<FlushDomain, DomainId>,
    lists: &mut DomainLists,
) -> Result<Reach, &'static str> {
    lists
        .claim(names.iter().filter_map(|name| domains.get(name).copied()))
        .map(Reach::Named)
}

/// The contract's spelling of a lineage's epoch state.
pub(crate) fn epoch_state<E: EpochId + 'static, I: Instant>(
    state: State,
) -> EpochState<DioringIds<E, I>> {
    match state {
        State::Open => EpochState::Open,
        State::Pending => EpochState::Pending,
        State::Blocked(by) => EpochState::Blocked(by),
        State::Durable => EpochState::Durable,
        State::Abandoned => EpochState::Abandoned,
    }
}

/// The first value `values` yields twice. Ordered rather than hashed, a hasher being `std`'s
/// (DI-D-97).
pub fn first_repeat<T: Ord + Clone>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut seen = BTreeSet::new();
    for value in values {
        let copy = tagged(tag::DOMAINS_FIRST_REPEAT_COPY, || value.clone());
        if !tagged(tag::DOMAINS_FIRST_REPEAT_SEEN, || seen.insert(copy)) {
            return Some(value);
        }
    }
    None
}

/// Intern `domain`, if the table has room for it or holds it already.
pub(crate) fn intern(
    table: &mut HeapMap<FlushDomain, DomainId>,
    domain: &FlushDomain,
) -> Option<DomainId> {
    let next = DomainId(u32::try_from(table.len()).ok()?);
    table.entry(domain.clone()).or_insert(next).ok().copied()
}

/// Intern `domains`, returning their ids sorted and without repeats: the provider's, when the
/// instance is built.
///
/// # Errors
///
/// [`Full`], handing back the domain, when the table has no room for one `domains` names that it
/// does not hold: those it interned before it are kept. The core checks that `domains` fit before
/// it interns any.
pub(crate) fn intern_all(
    table: &mut HeapMap<FlushDomain, DomainId>,
    domains: impl IntoIterator<Item = FlushDomain>,
) -> Result<Arc<[DomainId]>, Full<FlushDomain, DomainId>> {
    let mut ids: Vec<DomainId> = Vec::new();
    for domain in domains {
        let next =
            DomainId(u32::try_from(table.len()).expect("fewer than 2^32 distinct flush domains"));
        let id = *table.entry(domain).or_insert(next)?;
        tagged(tag::DOMAINS_INTERN_ALL_ID, || ids.push(id));
    }
    ids.sort_unstable();
    ids.dedup();
    Ok(tagged(tag::DOMAINS_INTERN_ALL_SHARED, || ids.into()))
}
