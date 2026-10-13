// Copyright (c) 2026 Mike Grier
// Split from dioring.rs at 60ebadb1.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::clock::Instant;
use crate::contract::EpochId;
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

/// What an import reaches: the files, the one lineage it is confined to if any, and the domains it
/// named that the instance has never been given.
pub(crate) type ImportReach = (Reach, Option<u64>, Vec<FlushDomain>);

/// The targets an imported failure reaches (DI-D-21), the one lineage it is confined to, if the
/// scope names one, and the domains it names that the instance has never been given, in the order
/// named (DI-D-46). A domain no target was declared with reaches only the targets declared with
/// none. `live` says whether a lineage is the instance's own and live; `domains` is every domain
/// the instance has interned.
pub(crate) fn import_reach<E: EpochId + 'static, I: Instant>(
    scope: &ImportScope<DioringIds<E, I>>,
    domains: &HashMap<FlushDomain, DomainId>,
    live: impl Fn(Lineage) -> bool,
) -> Result<ImportReach, UnknownLineage<DioringIds<E, I>>> {
    Ok(match scope {
        ImportScope::All => (Reach::All, None, Vec::new()),
        ImportScope::Lineage(lineage) if live(*lineage) => {
            (Reach::All, Some(lineage.seq), Vec::new())
        }
        ImportScope::Lineage(lineage) => return Err(UnknownLineage(*lineage)),
        ImportScope::Domains(names) => {
            let mut known = Vec::with_capacity(names.len());
            let mut unmatched = Vec::new();
            for domain in names {
                match domains.get(domain) {
                    Some(&id) => known.push(id),
                    None => unmatched.push(domain.clone()),
                }
            }
            (Reach::Domains(known.into()), None, unmatched)
        }
    })
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

/// The first value `values` yields twice.
pub fn first_repeat<T: Eq + std::hash::Hash + Clone>(
    values: impl IntoIterator<Item = T>,
) -> Option<T> {
    let mut seen = HashSet::new();
    values.into_iter().find(|value| !seen.insert(value.clone()))
}

/// Intern `domains`, returning their ids sorted and without repeats.
pub(crate) fn intern_all(
    table: &mut HashMap<FlushDomain, DomainId>,
    domains: impl IntoIterator<Item = FlushDomain>,
) -> Arc<[DomainId]> {
    let mut ids: Vec<DomainId> = domains
        .into_iter()
        .map(|domain| {
            let next = DomainId(
                u32::try_from(table.len()).expect("fewer than 2^32 distinct flush domains"),
            );
            *table.entry(domain).or_insert(next)
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into()
}
