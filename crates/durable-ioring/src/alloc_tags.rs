// Copyright (c) 2026 Mike Grier
//! Allocation tags ([DI-D-104]): every call in the core that can allocate runs inside the
//! crate-private `tagged`, which names it with a [`Tag`] of its own.
//!
//! A global allocator is given only a `Layout`, so nothing tells it which code asked for memory.
//! The core says so instead: `tagged(TAG, || self.map.insert(key, value))` sets the thread's
//! current tag for exactly the call it is given, and restores the tag it replaced when the call
//! returns or unwinds, so tags nest. An allocation made inside a container's mutation is counted
//! under the tag of the code that called it, which is as fine as attribution can go when the
//! container's code is not ours.
//!
//! The current tag exists only under the `alloc-tags` feature, which only the allocation test turns
//! on. Without it, `tagged` calls its closure and nothing else, so the core pays nothing for it.
//!
//! **The rule, which the allocation test enforces:** a call that can allocate is made inside
//! `tagged`, with a tag no other call uses. An allocation counted with no tag set fails the test,
//! and so does one tag seen from two places in the source.
//!
//! **A tagged closure holds the allocating call and nothing else.** Its arguments, and any call
//! into other code of the core, are made before it: an allocation inside the closure is counted
//! under its tag, so a call into code that allocates without a tag of its own would be hidden
//! there rather than reported as untagged. Calls into a closure's own tagged calls nest, and the
//! innermost tag wins.
//!
//! **Calls into what is plugged into the core are tagged too:** a backend's `push`, `check`,
//! `submit` and `pop`, a provider, a notification's wait, a diagnostic sink's `wants` and
//! `report`, and the clock. What they allocate is theirs to decide ([DI-D-97]), and is counted
//! under the call the core made to them, so a plugged-in type that allocates is seen, and seen
//! where it was called. A diagnostic sink that fans out to others is a sink, and its sinks are
//! counted under its caller's tag.
//!
//! [DI-D-97]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/DESIGN-NOTES.md#di-d-97
//!
//! [DI-D-104]: https://github.com/MikeGrier/windows-threadpool-sys/blob/main/crates/durable-ioring/DESIGN-NOTES.md#di-d-104

#[cfg(feature = "alloc-tags")]
use core::cell::Cell;
#[cfg(feature = "alloc-tags")]
use core::panic::Location;

#[cfg(test)]
mod tests;

/// The name of one allocating call in the core. Two calls never share one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Tag(&'static str);

impl Tag {
    const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The tag's name: the module, the function and what the call allocates for.
    #[must_use]
    #[cfg_attr(
        not(feature = "alloc-tags"),
        allow(
            dead_code,
            reason = "read only by an allocator counting allocations, under the feature"
        )
    )]
    pub const fn name(self) -> &'static str {
        self.0
    }
}

/// A tag and the place in the source the call carrying it was made.
#[cfg(feature = "alloc-tags")]
#[derive(Clone, Copy, Debug)]
pub struct Site {
    /// The call's tag.
    pub tag: Tag,
    /// Where the call was made.
    pub location: &'static Location<'static>,
}

#[cfg(feature = "alloc-tags")]
thread_local! {
    /// The innermost tagged call this thread is inside, if any. Const-initialised and without a
    /// destructor, so reading it from inside an allocator neither allocates nor fails.
    static CURRENT: Cell<Option<Site>> = const { Cell::new(None) };
}

/// The innermost tagged call this thread is inside: what an allocator counting allocations reads.
#[cfg(feature = "alloc-tags")]
#[must_use]
pub fn current() -> Option<Site> {
    CURRENT.try_with(Cell::get).ok().flatten()
}

/// Restores the tag it replaced when dropped.
#[cfg(feature = "alloc-tags")]
struct Guard(Option<Site>);

#[cfg(feature = "alloc-tags")]
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = CURRENT.try_with(|current| current.set(self.0));
    }
}

/// Run `call` with `tag` as the thread's current allocation tag, and return what it returns.
#[track_caller]
#[inline]
pub(crate) fn tagged<R>(tag: Tag, call: impl FnOnce() -> R) -> R {
    #[cfg(feature = "alloc-tags")]
    let _guard = {
        let site = Site {
            tag,
            location: Location::caller(),
        };
        Guard(
            CURRENT
                .try_with(|current| current.replace(Some(site)))
                .ok()
                .flatten(),
        )
    };
    #[cfg(not(feature = "alloc-tags"))]
    let _ = tag;
    call()
}

/// Every tag the core uses, one per allocating call, grouped by the file the call is in. A tag's
/// name is the module, the function, and what the call allocates for.
pub(crate) mod tag {
    use super::Tag;

    // io_core.rs
    pub(crate) const IO_CORE_NEW_SERVED: Tag = Tag::new("io_core::new::served");
    pub(crate) const IO_CORE_ADD_TARGET_RECORD: Tag = Tag::new("io_core::add_target::record");
    pub(crate) const IO_CORE_REMOVE_TARGET_LOWEST: Tag = Tag::new("io_core::remove_target::lowest");
    pub(crate) const IO_CORE_REMOVE_TARGET_LOWEST_GATES: Tag =
        Tag::new("io_core::remove_target::lowest_gates");
    pub(crate) const IO_CORE_COMPLETE_QUEUE: Tag = Tag::new("io_core::complete::queue");
    pub(crate) const IO_CORE_REFUSED_UNPUSHED: Tag = Tag::new("io_core::refused::unpushed");
    pub(crate) const IO_CORE_CLOSE_HELD: Tag = Tag::new("io_core::close::held");
    pub(crate) const IO_CORE_EPOCHS: Tag = Tag::new("io_core::epochs");
    pub(crate) const IO_CORE_ISSUE_BACKEND_CHECK: Tag = Tag::new("io_core::issue::backend_check");
    pub(crate) const IO_CORE_ISSUE_HELD: Tag = Tag::new("io_core::issue::held");
    pub(crate) const IO_CORE_ISSUE_BACKEND_PUSH: Tag = Tag::new("io_core::issue::backend_push");
    pub(crate) const IO_CORE_ISSUED_IN_FLIGHT: Tag = Tag::new("io_core::issued::in_flight");
    pub(crate) const IO_CORE_ISSUED_PER_TARGET: Tag = Tag::new("io_core::issued::per_target");
    pub(crate) const IO_CORE_ACT_QUEUE: Tag = Tag::new("io_core::act::queue");
    pub(crate) const IO_CORE_ACT_COMMIT_IN_FLIGHT: Tag = Tag::new("io_core::act::commit_in_flight");
    pub(crate) const IO_CORE_ACT_COMMIT_SUBMIT: Tag = Tag::new("io_core::act::commit_submit");
    pub(crate) const IO_CORE_ACT_REQUESTS: Tag = Tag::new("io_core::act::requests");
    pub(crate) const IO_CORE_ACT_REQUEST: Tag = Tag::new("io_core::act::request");
    pub(crate) const IO_CORE_NEVER_ISSUED_QUEUE: Tag = Tag::new("io_core::never_issued::queue");
    pub(crate) const IO_CORE_RELEASE_SUBMIT: Tag = Tag::new("io_core::release::submit");

    // durability.rs
    pub(crate) const DURABILITY_NEW_DEFAULT_LINEAGE: Tag =
        Tag::new("durability::new::default_lineage");
    pub(crate) const DURABILITY_NEW_SERVED: Tag = Tag::new("durability::new::served");
    pub(crate) const DURABILITY_PUSHED_IN_FLIGHT_BY_EPOCH: Tag =
        Tag::new("durability::pushed::in_flight_by_epoch");
    pub(crate) const DURABILITY_PUSHED_WRITE: Tag = Tag::new("durability::pushed::write");
    pub(crate) const DURABILITY_RESOLVE_ABANDONED: Tag = Tag::new("durability::resolve::abandoned");
    pub(crate) const DURABILITY_RESOLVE_ABANDONED_EVENT: Tag =
        Tag::new("durability::resolve::abandoned_event");
    pub(crate) const DURABILITY_RESOLVE_HEAL_PENDING: Tag =
        Tag::new("durability::resolve::heal_pending");
    pub(crate) const DURABILITY_FAILURES_INFOS: Tag = Tag::new("durability::failures::infos");
    pub(crate) const DURABILITY_FAILURES_CAUSE: Tag = Tag::new("durability::failures::cause");
    pub(crate) const DURABILITY_FAILURES_MARKINGS: Tag = Tag::new("durability::failures::markings");
    pub(crate) const DURABILITY_FAILURES_INFO: Tag = Tag::new("durability::failures::info");
    pub(crate) const DURABILITY_OBSERVE_SUSPECT: Tag = Tag::new("durability::observe::suspect");
    pub(crate) const DURABILITY_OBSERVE_EPOCHS_LINEAGE: Tag =
        Tag::new("durability::observe::epochs_lineage");
    pub(crate) const DURABILITY_OBSERVE_EPOCHS_EPOCH: Tag =
        Tag::new("durability::observe::epochs_epoch");
    pub(crate) const DURABILITY_OBSERVE_CAUSE: Tag = Tag::new("durability::observe::cause");
    pub(crate) const DURABILITY_OBSERVE_FAILURE: Tag = Tag::new("durability::observe::failure");
    pub(crate) const DURABILITY_OBSERVE_EVENT: Tag = Tag::new("durability::observe::event");
    pub(crate) const DURABILITY_MARK_MARKING: Tag = Tag::new("durability::mark::marking");
    pub(crate) const DURABILITY_MARK_EVENT: Tag = Tag::new("durability::mark::event");
    pub(crate) const DURABILITY_RELEASE_HEALED_EVENT: Tag =
        Tag::new("durability::release_healed::event");
    pub(crate) const DURABILITY_ADVANCE_LIVE: Tag = Tag::new("durability::advance::live");
    pub(crate) const DURABILITY_ADVANCE_MARK_DURABLE: Tag =
        Tag::new("durability::advance_mark::durable");
    pub(crate) const DURABILITY_ADVANCE_MARK_BLOCKED: Tag =
        Tag::new("durability::advance_mark::blocked");
    pub(crate) const DURABILITY_START_SEALS_FILE: Tag = Tag::new("durability::start_seals::file");
    pub(crate) const DURABILITY_START_SEALS_FLUSH: Tag = Tag::new("durability::start_seals::flush");
    pub(crate) const DURABILITY_START_SEALS_DOMAINS: Tag =
        Tag::new("durability::start_seals::domains");
    pub(crate) const DURABILITY_START_SEALS_FLUSH_PAIR: Tag =
        Tag::new("durability::start_seals::flush_pair");
    pub(crate) const DURABILITY_START_SEALS_CALL: Tag = Tag::new("durability::start_seals::call");
    pub(crate) const DURABILITY_SEAL_RECORD: Tag = Tag::new("durability::seal::record");
    pub(crate) const DURABILITY_END_PAIRS_FLUSHES: Tag = Tag::new("durability::end_pairs::flushes");
    pub(crate) const DURABILITY_END_SEAL_REPORT: Tag = Tag::new("durability::end_seal::report");
    pub(crate) const DURABILITY_END_FLUSH_REPORT: Tag = Tag::new("durability::end_flush::report");
    pub(crate) const DURABILITY_INCONSISTENT_WANTS: Tag =
        Tag::new("durability::inconsistent::wants");
    pub(crate) const DURABILITY_INCONSISTENT_REPORT: Tag =
        Tag::new("durability::inconsistent::report");
    pub(crate) const DURABILITY_SEAL_REPORT: Tag = Tag::new("durability::seal::report");
    pub(crate) const DURABILITY_OBSERVE_CLOCK: Tag = Tag::new("durability::observe::clock");
    pub(crate) const DURABILITY_MARK_CLOCK: Tag = Tag::new("durability::mark::clock");
    pub(crate) const DURABILITY_START_SEALS_REPORT: Tag =
        Tag::new("durability::start_seals::report");

    // durability/gates.rs
    pub(crate) const GATES_RAISE_BOUND: Tag = Tag::new("durability::gates::raise::bound");
    pub(crate) const GATES_WRITE_CYCLE_SEEDS: Tag =
        Tag::new("durability::gates::write_cycle::seeds");
    pub(crate) const GATES_SEAL_CYCLE_SEEDS: Tag = Tag::new("durability::gates::seal_cycle::seeds");
    pub(crate) const GATES_SEAL_CYCLE_SEED: Tag = Tag::new("durability::gates::seal_cycle::seed");
    pub(crate) const GATES_NEEDS_WORK: Tag = Tag::new("durability::gates::needs::work");
    pub(crate) const GATES_TRACE_STEPS: Tag = Tag::new("durability::gates::trace::steps");
    pub(crate) const GATES_TRACE_CYCLE_GATE: Tag = Tag::new("durability::gates::trace::cycle_gate");
    pub(crate) const GATES_TRACE_CYCLE_WRITE: Tag =
        Tag::new("durability::gates::trace::cycle_write");
    pub(crate) const GATES_TRACE_STEP: Tag = Tag::new("durability::gates::trace::step");
    pub(crate) const GATES_TRACE_UNFOUND: Tag = Tag::new("durability::gates::trace::unfound");
    pub(crate) const GATES_HOLD_GATED: Tag = Tag::new("durability::gates::hold::gated");
    pub(crate) const GATES_HOLD_HELD_BY_EPOCH: Tag =
        Tag::new("durability::gates::hold::held_by_epoch");
    pub(crate) const GATES_HOLD_WAITING: Tag = Tag::new("durability::gates::hold::waiting");
    pub(crate) const GATES_HOLD_HELD: Tag = Tag::new("durability::gates::hold::held");
    pub(crate) const GATES_ISSUED_KEPT: Tag = Tag::new("durability::gates::issued::kept");
    pub(crate) const GATES_RELEASE_THROUGH_REST: Tag =
        Tag::new("durability::gates::release_through::rest");
    pub(crate) const GATES_RELEASE_THROUGH_RELEASED: Tag =
        Tag::new("durability::gates::release_through::released");
    pub(crate) const GATES_RELEASE_THROUGH_DUE: Tag =
        Tag::new("durability::gates::release_through::due");
    pub(crate) const GATES_STOP_HELD_STOPPED: Tag =
        Tag::new("durability::gates::stop_held::stopped");
    pub(crate) const GATES_STOP_HELD_EVENT: Tag = Tag::new("durability::gates::stop_held::event");

    // durability/files.rs
    pub(crate) const FILES_FILE_HOLDS_SUSPECT: Tag =
        Tag::new("durability::files::file_holds::suspect");
    pub(crate) const FILES_FILE_HOLDS_FAILURE: Tag =
        Tag::new("durability::files::file_holds::failure");
    pub(crate) const FILES_FILE_HOLDS_THROUGH: Tag =
        Tag::new("durability::files::file_holds::through");
    pub(crate) const FILES_FILE_HOLDS_UNCOVERED_THROUGH: Tag =
        Tag::new("durability::files::file_holds::uncovered_through");

    // durability/providers.rs
    pub(crate) const PROVIDERS_NAME_PART: Tag = Tag::new("durability::providers::name::part");
    pub(crate) const PROVIDERS_NAME_WRITE: Tag = Tag::new("durability::providers::name::write");
    pub(crate) const PROVIDERS_NAME_WRITES: Tag = Tag::new("durability::providers::name::writes");
    pub(crate) const PROVIDERS_NAME_FILE: Tag = Tag::new("durability::providers::name::file");
    pub(crate) const PROVIDERS_SERVE_NAMES: Tag = Tag::new("durability::providers::serve::names");
    pub(crate) const PROVIDERS_PROVIDED_REACH: Tag =
        Tag::new("durability::providers::provided::reach");
    pub(crate) const PROVIDERS_COVER_OPS: Tag = Tag::new("durability::providers::cover::ops");

    // durability/lineages.rs
    pub(crate) const LINEAGES_MINT_DESCRIPTION: Tag =
        Tag::new("durability::lineages::mint::description");
    pub(crate) const LINEAGES_MINT_STATE: Tag = Tag::new("durability::lineages::mint::state");
    pub(crate) const LINEAGES_LINEAGES: Tag = Tag::new("durability::lineages::lineages");
    pub(crate) const LINEAGES_END_EVENT: Tag = Tag::new("durability::lineages::end::event");
    pub(crate) const LINEAGES_RETIRE_FAILURES: Tag =
        Tag::new("durability::lineages::retire::failures");

    // io_core/provider.rs
    pub(crate) const IO_CORE_WITH_PROVIDER_NAMED: Tag = Tag::new("io_core::with_provider::named");
    pub(crate) const IO_CORE_WITH_PROVIDER_SERVED_NAMES: Tag =
        Tag::new("io_core::with_provider::served_names");
    pub(crate) const IO_CORE_WITH_PROVIDER_NAMES: Tag = Tag::new("io_core::with_provider::names");
    pub(crate) const IO_CORE_REQUEST_DOMAINS: Tag = Tag::new("io_core::request::domains");
    pub(crate) const IO_CORE_REQUEST_TARGETS: Tag = Tag::new("io_core::request::targets");

    // domains.rs
    pub(crate) const DOMAINS_IMPORT_REACH_KNOWN: Tag = Tag::new("domains::import_reach::known");
    pub(crate) const DOMAINS_IMPORT_REACH_KNOWN_ID: Tag =
        Tag::new("domains::import_reach::known_id");
    pub(crate) const DOMAINS_IMPORT_REACH_UNMATCHED: Tag =
        Tag::new("domains::import_reach::unmatched");
    pub(crate) const DOMAINS_IMPORT_REACH_SHARED: Tag = Tag::new("domains::import_reach::shared");
    pub(crate) const DOMAINS_FIRST_REPEAT_COPY: Tag = Tag::new("domains::first_repeat::copy");
    pub(crate) const DOMAINS_FIRST_REPEAT_SEEN: Tag = Tag::new("domains::first_repeat::seen");
    pub(crate) const DOMAINS_INTERN_ALL_TABLE: Tag = Tag::new("domains::intern_all::table");
    pub(crate) const DOMAINS_INTERN_ALL_ID: Tag = Tag::new("domains::intern_all::id");
    pub(crate) const DOMAINS_INTERN_ALL_SHARED: Tag = Tag::new("domains::intern_all::shared");

    // router.rs
    pub(crate) const ROUTER_REGISTER_BOX: Tag = Tag::new("router::register::box");
    pub(crate) const ROUTER_REGISTER_PROVIDER: Tag = Tag::new("router::register::provider");
    pub(crate) const ROUTER_BUILD_ROUTES: Tag = Tag::new("router::build::routes");
    pub(crate) const ROUTER_BUILD_PROVIDER_DOMAINS: Tag =
        Tag::new("router::build::provider_domains");
    pub(crate) const ROUTER_BUILD_OWNER: Tag = Tag::new("router::build::owner");
    pub(crate) const ROUTER_BUILD_ROUTE: Tag = Tag::new("router::build::route");
    pub(crate) const ROUTER_DOMAINS: Tag = Tag::new("router::domains");
    pub(crate) const ROUTER_MAKE_DURABLE_PARTS: Tag = Tag::new("router::make_durable::parts");
    pub(crate) const ROUTER_MAKE_DURABLE_PART: Tag = Tag::new("router::make_durable::part");
    pub(crate) const ROUTER_MAKE_DURABLE_PROVIDER: Tag = Tag::new("router::make_durable::provider");

    // engine.rs
    pub(crate) const ENGINE_ACT_PENDING: Tag = Tag::new("engine::act::pending");
    pub(crate) const ENGINE_ACT_REQUESTS: Tag = Tag::new("engine::act::requests");
    pub(crate) const ENGINE_ACT_REFUSED: Tag = Tag::new("engine::act::refused");
    pub(crate) const ENGINE_PUSH_BACKEND: Tag = Tag::new("engine::push::backend");
    pub(crate) const ENGINE_PUSH_SUBMIT: Tag = Tag::new("engine::push::submit");
    pub(crate) const ENGINE_PUSH_BACKEND_AGAIN: Tag = Tag::new("engine::push::backend_again");
    pub(crate) const ENGINE_RING_SUBMIT: Tag = Tag::new("engine::ring::submit");
    pub(crate) const ENGINE_DRAIN_POP: Tag = Tag::new("engine::drain::pop");

    // pump.rs
    pub(crate) const PUMP_NEW_NOTIFICATION: Tag = Tag::new("pump::new::notification");
    pub(crate) const PUMP_NEW_HANDED_BACK: Tag = Tag::new("pump::new::handed_back");
    pub(crate) const PUMP_WITH_NOTIFICATION_SLOT: Tag = Tag::new("pump::with_notification::slot");
    pub(crate) const PUMP_PUMP_WAIT: Tag = Tag::new("pump::pump::wait");

    // ids.rs
    pub(crate) const IDS_LINEAGE_HANDLE_MINT: Tag = Tag::new("ids::lineage_handle::mint");
    pub(crate) const IDS_FAILURE_TOKEN_MINT: Tag = Tag::new("ids::failure_token::mint");

    // backend.rs
    pub(crate) const BACKEND_REFUSAL_FAULT_SHARED: Tag = Tag::new("backend::refusal_fault::shared");

    // types.rs
    pub(crate) const TYPES_SUSPECT_SET_NEW: Tag = Tag::new("types::suspect_set::new");
    pub(crate) const TYPES_FLUSH_DOMAIN_NEW: Tag = Tag::new("types::flush_domain::new");

    // diagnostics.rs
    pub(crate) const DIAGNOSTICS_FLUSH_PART_TEXT: Tag = Tag::new("diagnostics::flush_part::text");

    // io_target.rs
    pub(crate) const IO_TARGET_DESCRIBE_DOMAINS: Tag = Tag::new("io_target::describe::domains");

    // lock_watch.rs
    pub(crate) const LOCK_WATCH_LOCK_BEGAN: Tag = Tag::new("lock_watch::lock::began");
    pub(crate) const LOCK_WATCH_LOCK_ENDED: Tag = Tag::new("lock_watch::lock::ended");

    // correlation.rs
    pub(crate) const CORRELATION_MINT_FOR_WANTS: Tag = Tag::new("correlation::mint_for::wants");
}
