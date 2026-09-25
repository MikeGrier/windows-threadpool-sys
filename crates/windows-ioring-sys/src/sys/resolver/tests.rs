// Copyright (c) Mike Grier
//! Tests for the response-space resolver (M26.3).
//!
//! # What these assert, and what they deliberately do not
//!
//! Each test names the clause it is about and asserts two things: that the
//! resolver *may* do what the clause permits, and -- where the clause is a
//! freedom rather than a shape -- that it actually **did**, by reading
//! [`ResolverStats`]. The second half is the one worth having. A resolver that
//! permits reordering and never reorders passes any test written only against
//! the first, and a permission nothing exercises is indistinguishable from one
//! nothing implemented.
//!
//! These drive the resolver through [`Responses`] directly rather than through
//! an `IoRing`. That is not a convenience: this file is about whether the
//! resolver occupies the space it claims to, which is a property of the
//! resolver alone. `M26.4` is where a real ring is driven across it, and
//! `M26.5` is where the resolution is calibrated against defects this crate
//! actually shipped.

use std::ffi::c_void;
use std::ptr;

use windows_sys::Win32::Storage::FileSystem::{
    IORING_CQE, IORING_HANDLE_REF, IORING_HANDLE_REF_0, IORING_REF_RAW,
    IOSQE_FLAGS_DRAIN_PRECEDING_OPS,
};

use super::{Resolver, ResolverConfig, ResolverStats};
use crate::sys::Responses;

/// Seeds every ordering test sweeps.
///
/// Fixed rather than drawn, because a unit test must be reproducible without
/// an announcement (the repository's standing rule on random sampling). The
/// *environment* seed exists for the generated suites, which announce it;
/// here a fixed sweep is what makes a failure mean the same thing twice.
///
/// Raised from 64 to 2048 for breadth. The cost is a property of the sweep and
/// not of the seed count alone -- measured, a seed here is tens of
/// microseconds, so the whole file stays far inside the sub-second budget this
/// repository sets for a submodule's unit tests.
const SEEDS: std::ops::Range<u64> = 0..2048;

/// How many seeds [`SEEDS`] covers.
///
/// Spelled out because `Range<u64>` is not an `ExactSizeIterator` -- 64-bit
/// ranges can exceed `usize` -- so there is no `len()` to ask for.
const SEED_COUNT: usize = 2048;

/// Distinct orders six independently-resolving operations can be posted in.
///
/// `6!`. This is the ceiling on what
/// [`different_seeds_reach_different_resolutions`] can observe, and it is
/// *below* [`SEED_COUNT`] -- which is exactly the trap that assertion fell
/// into when the sweep was widened. A threshold phrased as a fraction of the
/// seed count silently becomes unsatisfiable once the seeds outnumber the
/// outcomes, so the bound is stated against the space the test can actually
/// reach.
///
/// Measured over the resolver's own mixer: 64 seeds reach 64 of these, 1024
/// reach 539, 2048 reach 670, and 8192 are needed for all 720. So the sweep is
/// still gaining breadth at its current size rather than re-drawing orders it
/// has already seen.
const DISTINCT_ORDERS_OF_SIX: usize = 720;

/// A handle reference that is never dereferenced.
///
/// The resolver answers every call it is given without touching the caller's
/// pointers, so a null one is the honest argument: if some future edit did
/// start dereferencing, these tests would fault rather than silently pass on
/// a plausible-looking fake. That is the same reasoning `M23.5` recorded about
/// ring handles -- a fake that looks valid is worse than one that cannot be.
fn handle() -> IORING_HANDLE_REF {
    IORING_HANDLE_REF {
        Kind: IORING_REF_RAW,
        Handle: IORING_HANDLE_REF_0 {
            Handle: ptr::null_mut(),
        },
    }
}

/// Drive a flush build, which is the cheapest operation to synthesise and the
/// only one that can carry a barrier through this crate's public surface.
fn build_flush(resolver: &mut Resolver, user_data: usize, barrier: bool) {
    let flags = if barrier {
        IOSQE_FLAGS_DRAIN_PRECEDING_OPS
    } else {
        0
    };
    // SAFETY: the resolver dereferences none of these; see `handle`.
    let hr = unsafe { resolver.build_flush(ptr::null_mut(), handle(), 0, user_data, flags) };
    assert_eq!(hr, 0, "a build should report success");
}

/// Submit without waiting.
fn submit(resolver: &mut Resolver) -> i32 {
    let mut submitted = 0_u32;
    // SAFETY: `submitted` is a valid out-pointer; the ring pointer is unused.
    unsafe { resolver.submit(ptr::null_mut(), 0, 0, &raw mut submitted) }
}

/// Submit asking to wait, which is what `RS-P-4` and `RS-P-5` are about.
fn submit_waiting(resolver: &mut Resolver) -> i32 {
    let mut submitted = 0_u32;
    // SAFETY: as `submit`.
    unsafe { resolver.submit(ptr::null_mut(), 1, 50, &raw mut submitted) }
}

/// Pop one completion, or `None` when the queue is empty.
fn pop(resolver: &mut Resolver) -> Option<(usize, i32)> {
    let mut cqe = IORING_CQE {
        UserData: 0,
        ResultCode: 0,
        Information: 0,
    };
    // SAFETY: `cqe` is a valid out-pointer.
    let hr = unsafe { resolver.pop(ptr::null_mut(), &raw mut cqe) };
    if hr == 1 {
        return None;
    }
    assert_eq!(
        hr, 0,
        "a pop should either succeed or report an empty queue"
    );
    Some((cqe.UserData, cqe.ResultCode))
}

/// Drain everything the resolver will ever produce, submitting to drive
/// resolution, and return the completions in the order they were handed over.
///
/// Bounded so a resolver that stopped making progress fails as a test rather
/// than as a hang -- which is the whole difference between a caught defect and
/// a CI job somebody cancels.
fn drain(resolver: &mut Resolver, expected: usize) -> Vec<(usize, i32)> {
    let mut seen = Vec::new();
    for _ in 0..(expected + 2) * (super::MAX_DEFERRALS as usize + 2) {
        while let Some(completion) = pop(resolver) {
            seen.push(completion);
        }
        if seen.len() == expected {
            return seen;
        }
        submit(resolver);
    }
    panic!(
        "resolution did not finish: {} of {expected} completions after the bound, which is \
         RS-C-1 not holding in finite time",
        seen.len()
    );
}

/// Run `body` over every seed in [`SEEDS`] and return the accumulated stats.
fn sweep(config: ResolverConfig, mut body: impl FnMut(&mut Resolver)) -> Vec<ResolverStats> {
    SEEDS
        .map(|seed| {
            let mut resolver = Resolver::with_config(seed, config);
            let watch = resolver.watch();
            body(&mut resolver);
            watch.stats()
        })
        .collect()
}

// ------------------------------------------------------- RS-C-1 and RS-C-2 ---

#[test]
fn every_submitted_operation_completes_exactly_once_carrying_its_own_identity() {
    // RS-C-1 and RS-C-2 together, over the widest configuration: whatever else
    // the resolver chooses, the multiset of completions equals the multiset of
    // submissions. This is the constraint every other test rests on, so it is
    // swept rather than sampled.
    for seed in SEEDS {
        let mut resolver = Resolver::new(seed);
        for id in 1..=8_usize {
            build_flush(&mut resolver, id, false);
        }
        submit(&mut resolver);
        let mut ids: Vec<usize> = drain(&mut resolver, 8)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort_unstable();
        assert_eq!(
            ids,
            (1..=8).collect::<Vec<_>>(),
            "seed {seed}: every operation must complete exactly once, carrying the user data \
             it was built with"
        );
    }
}

#[test]
fn an_operation_that_was_never_submitted_does_not_complete() {
    // RS-C-3. Built is not submitted: D-5 records that there is no rewind once
    // `Build*` returns, but that is about the operation being *queued*, not
    // about it having run.
    //
    // POLLED PAST THE DEFERRAL BOUND ON PURPOSE, and the reason is a defect
    // this test had until the sabotage sweep found it. A single pop asserts
    // only that nothing completed *yet*, and a resolver that had wrongly made
    // the staged operations eligible would still answer that pop with nothing
    // -- the starvation rescue posts only what has run out of deferrals, and
    // a freshly built operation has not. The claim is that unsubmitted work is
    // never ELIGIBLE, so the test has to outlast the bound that would make an
    // eligible operation surface.
    let mut resolver = Resolver::new(1);
    for id in 1..=4_usize {
        build_flush(&mut resolver, id, false);
    }
    for poll in 0..=(super::MAX_DEFERRALS + 2) {
        assert_eq!(
            pop(&mut resolver),
            None,
            "poll {poll}: nothing may complete before a submit carries it"
        );
    }
    let stats = resolver.watch().stats();
    assert_eq!(stats.built, 4, "four operations were built");
    assert_eq!(
        stats.submitted, 0,
        "none was submitted, so none may have become eligible"
    );
    assert_eq!(stats.posted, 0, "and none may have been posted");
}

// ----------------------------------------------------------------- RS-C-4 ---

#[test]
fn nothing_queued_before_a_drained_flush_completes_after_it() {
    // RS-C-4, the one place this space is narrower than "anything may happen",
    // swept across every seed because it is a claim about *all* resolutions.
    for seed in SEEDS {
        let mut resolver = Resolver::new(seed);
        // Three ordinary operations, then a barrier, then three more. The
        // trailing three are deliberately present: the hold-back half is NOT
        // constrained (D-24 claimed it, D-47 withdrew it), so they may appear
        // anywhere at all -- including before the barrier.
        for id in 1..=3_usize {
            build_flush(&mut resolver, id, false);
        }
        build_flush(&mut resolver, 100, true);
        for id in 4..=6_usize {
            build_flush(&mut resolver, id, false);
        }
        submit(&mut resolver);

        let order: Vec<usize> = drain(&mut resolver, 7)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        let barrier_at = order
            .iter()
            .position(|&id| id == 100)
            .expect("the barrier must complete");
        for id in 1..=3_usize {
            let at = order
                .iter()
                .position(|&candidate| candidate == id)
                .expect("every operation completes");
            assert!(
                at < barrier_at,
                "seed {seed}: operation {id} was queued before the drained flush and completed \
                 after it, which RS-C-4 forbids -- order was {order:?}"
            );
        }
    }
}

#[test]
fn the_hold_back_half_of_the_drain_flag_is_not_constrained() {
    // The other side of RS-C-4, and the reason the clause is one-sided. D-47
    // withdrew D-24's claim that a drained flush holds back what follows, so a
    // resolver that enforced both halves would be narrower than the space and
    // would hide exactly the defect class D-47 found.
    let overtook = SEEDS
        .filter(|&seed| {
            let mut resolver = Resolver::new(seed);
            build_flush(&mut resolver, 1, false);
            build_flush(&mut resolver, 100, true);
            build_flush(&mut resolver, 2, false);
            submit(&mut resolver);
            let order: Vec<usize> = drain(&mut resolver, 3)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            let barrier_at = order.iter().position(|&id| id == 100).unwrap();
            let after_at = order.iter().position(|&id| id == 2).unwrap();
            after_at < barrier_at
        })
        .count();
    assert!(
        overtook > 0,
        "no seed let an operation queued after a drained flush complete before it, so the \
         resolver is enforcing a hold-back D-47 withdrew"
    );
}

#[test]
fn a_barrier_is_actually_held_back_rather_than_vacuously_satisfied() {
    // RS-C-4 could be satisfied by a resolver that never had a barrier with
    // anything in front of it. The counter distinguishes "the constraint bit"
    // from "the constraint never applied", which is the difference between a
    // test and a tautology.
    let stats = sweep(ResolverConfig::default(), |resolver| {
        build_flush(resolver, 1, false);
        build_flush(resolver, 2, false);
        build_flush(resolver, 100, true);
        submit(resolver);
        drain(resolver, 3);
    });
    let held: usize = stats.iter().map(|s| s.barrier_holds).sum();
    assert!(
        held > 0,
        "no seed ever held a barrier back, so RS-C-4 was satisfied vacuously and this suite \
         proves nothing about it"
    );
}

// ----------------------------------------------------------------- RS-P-1 ---

#[test]
fn an_operation_may_pend_rather_than_complete_inside_the_submit() {
    let stats = sweep(ResolverConfig::default(), |resolver| {
        for id in 1..=6_usize {
            build_flush(resolver, id, false);
        }
        submit(resolver);
        drain(resolver, 6);
    });
    assert!(
        stats.iter().any(|s| s.deferrals > 0),
        "no seed left an operation unresolved by the submit that carried it, so RS-P-1's \
         pending case is permitted but never exercised"
    );
}

#[test]
fn narrowing_rs_p_1_completes_everything_inside_the_submit() {
    // The converse, which is what makes the permission's switch meaningful: a
    // test isolating some other freedom needs `may_pend: false` to actually
    // remove this one.
    for seed in SEEDS {
        let mut resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_pend: false,
                ..ResolverConfig::narrowest()
            },
        );
        for id in 1..=5_usize {
            build_flush(&mut resolver, id, false);
        }
        submit(&mut resolver);
        let mut seen = Vec::new();
        while let Some((id, _)) = pop(&mut resolver) {
            seen.push(id);
        }
        assert_eq!(
            seen.len(),
            5,
            "seed {seed}: with RS-P-1 narrowed, every operation completes inside its submit"
        );
    }
}

// ----------------------------------------------------------------- RS-P-2 ---

#[test]
fn completion_order_is_unconstrained() {
    // RS-P-2, and the demonstration the probe already made: a FIFO-assuming
    // consumer breaks. Asserting that *some* seed reorders is the honest form
    // -- a specific permutation at a specific seed would be asserting against
    // the mixer rather than against the clause.
    let reordered = SEEDS
        .filter(|&seed| {
            let mut resolver = Resolver::with_config(
                seed,
                ResolverConfig {
                    may_reorder: true,
                    ..ResolverConfig::narrowest()
                },
            );
            for id in 1..=5_usize {
                build_flush(&mut resolver, id, false);
            }
            submit(&mut resolver);
            let order: Vec<usize> = drain(&mut resolver, 5)
                .into_iter()
                .map(|(id, _)| id)
                .collect();
            order != (1..=5).collect::<Vec<_>>()
        })
        .count();
    assert!(
        reordered > SEED_COUNT / 2,
        "only {reordered} of {SEED_COUNT} seeds reordered, which is too few for a clause that \
         permits any permutation"
    );
}

#[test]
fn narrowing_rs_p_2_posts_in_submission_order() {
    for seed in SEEDS {
        let mut resolver = Resolver::with_config(seed, ResolverConfig::narrowest());
        for id in 1..=5_usize {
            build_flush(&mut resolver, id, false);
        }
        submit(&mut resolver);
        let order: Vec<usize> = drain(&mut resolver, 5)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            order,
            (1..=5).collect::<Vec<_>>(),
            "seed {seed}: with RS-P-2 narrowed the order is submission order"
        );
    }
}

// ----------------------------------------------------------------- RS-P-3 ---

#[test]
fn an_operation_may_fail_while_its_neighbours_succeed() {
    // RS-P-3. The mixed case is the one that matters: M22.2's defect was an
    // ordering error in handling a *single* failed write among successes, and
    // a resolver that only ever failed all or none would not have reached it.
    let mixed = SEEDS
        .filter(|&seed| {
            let mut resolver = Resolver::with_config(
                seed,
                ResolverConfig {
                    may_fail_operations: true,
                    ..ResolverConfig::narrowest()
                },
            );
            for id in 1..=6_usize {
                build_flush(&mut resolver, id, false);
            }
            submit(&mut resolver);
            let results = drain(&mut resolver, 6);
            let failures = results.iter().filter(|(_, hr)| *hr != 0).count();
            failures > 0 && failures < results.len()
        })
        .count();
    assert!(
        mixed > 0,
        "no seed produced a batch in which some operations failed and others succeeded"
    );
}

#[test]
fn failure_codes_are_not_drawn_from_a_small_fixed_set() {
    // RS-P-3 says a consumer must not depend on the set of codes being small,
    // and enumerates none. A resolver returning one hard-coded error would
    // satisfy the clause's letter while teaching every consumer built against
    // it to match on that one code.
    let mut codes = std::collections::BTreeSet::new();
    for seed in SEEDS {
        let mut resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_fail_operations: true,
                ..ResolverConfig::narrowest()
            },
        );
        for id in 1..=4_usize {
            build_flush(&mut resolver, id, false);
        }
        submit(&mut resolver);
        for (_, hr) in drain(&mut resolver, 4) {
            if hr != 0 {
                codes.insert(hr);
            }
        }
    }
    assert!(
        codes.len() > 16,
        "only {} distinct failure codes across {SEED_COUNT} seeds, which is a small fixed set \
         in all but name",
        codes.len()
    );
}

// --------------------------------------------------------- RS-P-4, RS-P-5 ---

#[test]
fn a_wait_may_expire() {
    // RS-P-4. `WAIT_EXPIRED` is not a ring failure -- M21.6 fixed a defect in
    // this crate that read it as one -- so a resolver that never produced it
    // would leave that correction untested.
    let expired = SEEDS
        .filter(|&seed| {
            let mut resolver = Resolver::with_config(
                seed,
                ResolverConfig {
                    may_expire_waits: true,
                    ..ResolverConfig::narrowest()
                },
            );
            build_flush(&mut resolver, 1, false);
            let hr = submit_waiting(&mut resolver);
            let expired = hr == super::WAIT_EXPIRED;
            drain(&mut resolver, 1);
            expired
        })
        .count();
    assert!(
        expired > 0,
        "no seed expired a wait, so RS-P-4 is permitted but never exercised"
    );
}

#[test]
fn a_wait_may_expire_even_when_a_completion_is_available() {
    // The over-provision half of RS-P-4, stated separately because it is the
    // part no measurement here established: a consumer that treats a
    // successful return as "therefore something is poppable", or an expiry as
    // "therefore nothing is", is wrong in both directions.
    let expired_with_work = SEEDS
        .filter(|&seed| {
            let mut resolver = Resolver::with_config(
                seed,
                ResolverConfig {
                    may_expire_waits: true,
                    ..ResolverConfig::narrowest()
                },
            );
            build_flush(&mut resolver, 1, false);
            let hr = submit_waiting(&mut resolver);
            let available = !resolver.posted.is_empty();
            drain(&mut resolver, 1);
            hr == super::WAIT_EXPIRED && available
        })
        .count();
    assert!(
        expired_with_work > 0,
        "no seed expired a wait while a completion sat in the queue, so RS-P-4's \
         over-provision is unexercised and a consumer could still read an expiry as emptiness"
    );
}

#[test]
fn a_successful_wait_promises_nothing_about_poppability() {
    // RS-P-5, which this crate's own `pop_within` documentation already
    // states, and which D-19's edge-triggered measurement explains.
    let stats = sweep(
        ResolverConfig {
            may_wake_empty: true,
            may_pend: true,
            ..ResolverConfig::narrowest()
        },
        |resolver| {
            for id in 1..=3_usize {
                build_flush(resolver, id, false);
            }
            submit_waiting(resolver);
            drain(resolver, 3);
        },
    );
    assert!(
        stats.iter().any(|s| s.empty_wakes > 0),
        "no seed returned a successful wait with an empty queue, so RS-P-5 is unexercised"
    );
}

// ----------------------------------------------------------------- RS-P-6 ---

#[test]
fn a_completion_posted_behind_another_raises_no_signal_of_its_own() {
    // RS-P-6, which is a measurement rather than an over-provision: D-19 found
    // the completion event edge triggered on empty-to-non-empty, and D-21 is
    // the consequence this crate drew from it.
    //
    // A real event handle, because the resolver's signalling is the behaviour
    // under test and a null handle would skip it entirely.
    // SAFETY: a manual-reset, initially-unsignalled, unnamed event.
    let event = unsafe {
        windows_sys::Win32::System::Threading::CreateEventW(ptr::null(), 1, 0, ptr::null())
    };
    assert!(!event.is_null(), "CreateEventW failed");

    let mut resolver = Resolver::with_config(
        7,
        ResolverConfig {
            may_pend: false,
            ..ResolverConfig::narrowest()
        },
    );
    // SAFETY: a live event this test owns for the resolver's whole lifetime.
    let hr = unsafe { resolver.set_completion_event(ptr::null_mut(), event.cast::<c_void>()) };
    assert_eq!(hr, 0);

    for id in 1..=4_usize {
        build_flush(&mut resolver, id, false);
    }
    submit(&mut resolver);
    assert_eq!(
        resolver.watch().stats().signals,
        1,
        "four completions posted back to back must raise exactly one signal -- the \
         empty-to-non-empty edge"
    );

    // Drain, then post again: the queue went empty, so the next post is a new
    // edge and signals again. Without this half the test would pass for a
    // resolver that signalled exactly once ever.
    while pop(&mut resolver).is_some() {}
    build_flush(&mut resolver, 5, false);
    submit(&mut resolver);
    assert_eq!(
        resolver.watch().stats().signals,
        2,
        "a post onto a queue that had drained is a fresh edge and must signal"
    );

    // SAFETY: this test owns the handle and the resolver is done with it.
    unsafe { windows_sys::Win32::Foundation::CloseHandle(event) };
}

// ----------------------------------------------------------------- RS-P-7 ---

#[test]
fn a_failed_submit_leaves_operations_queued_for_a_later_one() {
    // RS-P-7, which follows from D-5: the submission queue is ring state, not
    // batch state, and there is no rewind once `Build*` returns.
    let stats = sweep(
        ResolverConfig {
            may_fail_submits: true,
            ..ResolverConfig::narrowest()
        },
        |resolver| {
            for id in 1..=4_usize {
                build_flush(resolver, id, false);
            }
            submit(resolver);
            drain(resolver, 4);
        },
    );
    let failed: usize = stats.iter().map(|s| s.failed_submits).sum();
    assert!(
        failed > 0,
        "no seed declined a submit, so RS-P-7 is unexercised"
    );
    for (seed, s) in stats.iter().enumerate() {
        assert_eq!(
            s.popped, 4,
            "seed {seed}: a declined submit must leave the operations queued, not lose them"
        );
    }
}

// ----------------------------------------------------- the seed discipline ---

#[test]
fn one_seed_replays_a_whole_resolution() {
    // D-41's discipline, applied to this axis. Two resolvers on the same seed
    // must agree completely; if they did not, the replay command the resolver
    // prints would be a lie.
    let run = |seed| {
        let mut resolver = Resolver::new(seed);
        for id in 1..=6_usize {
            build_flush(&mut resolver, id, id == 3);
        }
        submit(&mut resolver);
        (drain(&mut resolver, 6), resolver.watch().stats())
    };
    for seed in SEEDS {
        assert_eq!(
            run(seed),
            run(seed),
            "seed {seed} did not replay identically"
        );
    }
}

#[test]
fn different_seeds_reach_different_resolutions() {
    // The converse, and not a formality: a resolver whose seed did nothing
    // would pass every replay test above while exploring one point forever.
    //
    // The bound is a fraction of [`DISTINCT_ORDERS_OF_SIX`], not of the seed
    // count, and the difference is load-bearing rather than pedantic: six
    // operations admit 720 orders, so a threshold of `SEED_COUNT / 2` becomes
    // arithmetically unsatisfiable the moment the sweep exceeds 1440 seeds --
    // the test would fail without anything having regressed.
    let orders: std::collections::BTreeSet<Vec<usize>> = SEEDS
        .map(|seed| {
            let mut resolver = Resolver::with_config(
                seed,
                ResolverConfig {
                    may_reorder: true,
                    ..ResolverConfig::narrowest()
                },
            );
            for id in 1..=6_usize {
                build_flush(&mut resolver, id, false);
            }
            submit(&mut resolver);
            drain(&mut resolver, 6)
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        })
        .collect();
    assert!(
        orders.len() > DISTINCT_ORDERS_OF_SIX / 2,
        "only {} of {DISTINCT_ORDERS_OF_SIX} possible orders across {SEED_COUNT} seeds",
        orders.len()
    );
}

#[test]
fn the_environment_seed_is_parsed_in_both_spellings_and_rejected_when_malformed() {
    // The variable is the whole replay mechanism, so the failure that matters
    // is a malformed value silently falling back to the clock: the run would
    // look pinned and would not be.
    //
    // `seed_from_env` reads a process-wide variable, so this drives the
    // parsing through the same expression rather than mutating the
    // environment -- setting one here would race every other test in this
    // binary, which runs its tests as threads in one process.
    let parse = |text: &str| -> Option<u64> {
        let trimmed = text.trim();
        trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
            .map_or_else(
                || trimmed.parse::<u64>().ok(),
                |hex| u64::from_str_radix(hex, 16).ok(),
            )
    };
    assert_eq!(parse("42"), Some(42));
    assert_eq!(parse("0x2A"), Some(42));
    assert_eq!(parse("0X2a"), Some(42));
    assert_eq!(parse("  0x2A  "), Some(42));
    assert_eq!(
        parse("2A"),
        None,
        "bare hex is not decimal and must not parse"
    );
    assert_eq!(parse("banana"), None);
    assert_eq!(parse(""), None);
}

#[test]
fn the_replay_hint_names_this_axis_and_the_seed_it_pins() {
    let resolver = Resolver::new(0x2A);
    assert_eq!(
        resolver.replay_hint(),
        format!("$env:{}='0x000000000000002A'", super::SEED_VAR)
    );
    assert_eq!(resolver.seed(), 0x2A);
}

// ------------------------------------------------------------ the defaults ---

#[test]
fn the_default_configuration_is_the_widest_point_in_the_space() {
    // The default matters because it decides what a test gets when it does not
    // choose. Wide-by-default means an unconsidered test fails loudly under a
    // freedom it did not handle; narrow-by-default would mean it passes while
    // exercising nothing.
    let wide = ResolverConfig::default();
    assert!(wide.may_pend);
    assert!(wide.may_reorder);
    assert!(wide.may_fail_operations);
    assert!(wide.may_expire_waits);
    assert!(wide.may_wake_empty);
    assert!(wide.may_fail_submits);

    let narrow = ResolverConfig::narrowest();
    assert!(!narrow.may_pend);
    assert!(!narrow.may_reorder);
    assert!(!narrow.may_fail_operations);
    assert!(!narrow.may_expire_waits);
    assert!(!narrow.may_wake_empty);
    assert!(!narrow.may_fail_submits);

    // Both keep the edge-triggered signal, because turning it *off* is the
    // widening. RS-P-6 carries no over-provision -- it is the measurement --
    // so the narrowest point in the space still has it.
    assert!(wide.edge_triggered_signal);
    assert!(narrow.edge_triggered_signal);
}

#[test]
fn the_configuration_has_a_switch_for_every_permission_and_none_for_any_constraint() {
    // The asymmetry the module documents, asserted rather than described. A
    // switch that relaxed an `RS-C-n` would let a test quietly assert against
    // a platform that cannot exist, so the count is pinned: seven permissions
    // in RESPONSE-SPACE.md, seven fields, and no eighth for a constraint.
    //
    // Counted through `Debug`, which lists exactly the struct's fields, so a
    // field added without a clause fails here rather than passing unnoticed.
    let rendered = format!("{:?}", ResolverConfig::default());
    let fields = rendered.matches(": ").count();
    assert_eq!(
        fields, 7,
        "ResolverConfig has {fields} fields; RESPONSE-SPACE.md specifies seven permissions \
         (RS-P-1..7), and a constraint must never become a switch -- {rendered}"
    );
}
