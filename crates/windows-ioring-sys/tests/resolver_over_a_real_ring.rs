// Copyright (c) Mike Grier
//! The response-space resolver, answering for a real `IoRing` (M26.3).
//!
//! # Why this is an integration test
//!
//! `M26.2`'s seam deliberately leaves the ring's *lifecycle* calls real
//! ([D-60](../DESIGN-NOTES.md#d-60)), so a resolver runs against a ring the
//! kernel actually created. That is an operating-system boundary, which this
//! repository's Quality rule places in `tests/` rather than in the lib suite
//! -- and `check-ring-tests.ps1` exists to keep that population from drifting
//! back the other way.
//!
//! # What this file is for, and what it is not
//!
//! The resolver's own unit tests drive [`Responses`] directly, because whether
//! it occupies the space it claims to is a property of the resolver alone.
//! What they cannot show is that it is **reachable** -- that installing one
//! actually diverts a real `IoRing`'s calls, that an operation the kernel
//! never saw still completes through the crate's ordinary accounting, and that
//! the ring afterwards runs down rather than blocking on work that does not
//! exist.
//!
//! That is this file. It is a reachability check, not a property suite;
//! `M26.4` is where the properties that must hold under *every* resolution are
//! written, and `M26.5` is where the resolution is calibrated by re-injecting
//! defects this crate actually shipped.

#![cfg(all(windows, feature = "kernel-seam"))]

use std::os::windows::io::AsRawHandle;

use windows_ioring_sys::sys::{Resolver, ResolverConfig};
use windows_ioring_sys::{Batch, FlushCoverage, FlushMode, IoRing, SharedFile};

/// The ring these tests drive.
///
/// Every operation here is a flush, which carries no buffer -- so the entry
/// exists only to hold the file guard the token used to carry (`D-73`), and
/// `held.is_some()` is the identity check `claim_if` used to make.
type ResolverRing = IoRing;

/// A file to aim flushes at. Its contents never matter: under a resolver the
/// operation never reaches the kernel, and the point of the handle is that the
/// crate's own `Build*` path is exercised exactly as it would be otherwise.
fn scratch(tag: &str) -> (SharedFile, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-m26-3-{}-{tag}.tmp",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .expect("a scratch file");
    assert!(!file.as_raw_handle().is_null());
    (SharedFile::new(file.into()), path)
}

#[test]
fn an_installed_resolver_answers_a_real_rings_operations() {
    // The narrowest point in the space: this asserts reachability, so every
    // freedom that could make the outcome depend on the seed is off. A test
    // about reachability that could fail for an ordering reason would be two
    // tests wearing one name.
    let resolver = Resolver::with_config(0x5EED, ResolverConfig::narrowest());
    let replay = resolver.replay_hint();

    let (path, outcome) = resolver.scoped(|watch| {
        // Created *inside* the scope, so the ring's rundown is answered by the
        // resolver that owns its operations. Outside it, rundown would ask a
        // real ring to wait for completions the kernel has no record of.
        let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
        let (file, path) = scratch("reachable");

        {
            let mut batch = Batch::new(&mut ring);
            batch
                .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
            batch.submit().expect("the submit is answered");
        }

        assert_eq!(
            ring.outstanding(),
            1,
            "the crate's accounting counts the operation whether the kernel saw it or not"
        );

        let (_completion, held) = ring
            .pop_within_held(std::time::Duration::from_secs(5))
            .expect("the pop is answered")
            .expect("a completion arrives within the bound");
        assert!(
            held.is_some(),
            "RS-C-2: the completion must identify the operation that produced it"
        );
        assert_eq!(
            ring.outstanding(),
            0,
            "and the crate's accounting must clear on a resolved completion"
        );

        ring.run_down().expect("a ring under a resolver runs down");
        (path, watch.stats())
    });
    let _ = std::fs::remove_file(path);

    // The assertion that makes this a reachability test rather than a ring
    // test: had the seam not diverted the call, the kernel would have answered
    // and these counters would all be zero while every assertion above still
    // passed.
    assert_eq!(
        outcome.built, 1,
        "{replay}: the resolver did not see the Build* call, so the seam did not divert it"
    );
    assert_eq!(outcome.submitted, 1, "{replay}: nor the submit");
    assert_eq!(
        outcome.posted, 1,
        "{replay}: nor did it post the completion"
    );
    assert_eq!(outcome.popped, 1, "{replay}: nor hand it over");
}

#[test]
fn a_ring_runs_down_under_the_widest_resolution() {
    // Rundown is where a resolver that fails to satisfy RS-C-1 stops being a
    // failing test and becomes a hung one: `IoRing::run_down` loops while
    // anything is outstanding. Driving it at the widest point -- operations
    // pend, completions reorder, individual operations fail, waits expire and
    // wake empty -- is the case where a resolver that made no progress would
    // park here.
    //
    // Swept over seeds rather than run once, because "it terminated" at one
    // seed says nothing about a space whose whole purpose is that the choices
    // differ.
    //
    // RS-P-7 IS DELIBERATELY NARROWED OFF, AND THAT IS A FINDING RATHER THAN
    // A CONVENIENCE. At the genuinely widest point this test fails: a submit
    // declined under RS-P-7 propagates out of `run_down` as an error, leaving
    // `outstanding() > 0`, after which `Drop` asserts and calls `CloseIoRing`
    // anyway -- the exact hazard M21.6 fixed for `ERROR_TIMEOUT`, reachable
    // again through a different HRESULT. Measured: with this one permission
    // off, all 32 seeds pass; with it on, seed 0x1A fails at `0x80070008`.
    //
    // That is not fixed here, because the fix is a decision rather than a
    // correction: `run_down`'s own documentation argues that blocking is the
    // safe failure mode and closing early is not, which says it should keep
    // looping -- but a permanently failing submit then hangs, and "no hang" is
    // one of the properties M26.4 is about to write. The two pull opposite
    // ways and an engineer chooses. Queued as `M26.8`.
    for seed in 0..32_u64 {
        let resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_fail_submits: false,
                ..ResolverConfig::default()
            },
        );
        let replay = resolver.replay_hint();
        let path = resolver.scoped(|watch| {
            let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
            let (file, path) = scratch("rundown");

            let mut batch = Batch::new(&mut ring);
            for _ in 0..6 {
                batch
                    .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
                    .expect("a flush builds");
            }
            batch.submit().expect("the submit is answered");

            ring.run_down()
                .unwrap_or_else(|error| panic!("{replay}: rundown failed: {error}"));
            assert_eq!(
                ring.outstanding(),
                0,
                "{replay}: rundown returned with work still outstanding"
            );

            let stats = watch.stats();
            assert_eq!(
                stats.built, 6,
                "{replay}: the resolver must have seen every build"
            );
            assert_eq!(
                stats.posted, 6,
                "{replay}: RS-C-1 -- every submitted operation completes exactly once"
            );
            path
        });
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn a_declined_submit_leaves_the_ring_resumable_and_the_policy_to_the_caller() {
    // What M26.8 settled, replacing the test that pinned the old behaviour.
    //
    // `SubmitIoRing` documents that an error other than IORING_E_WAIT_TIMEOUT
    // leaves **all entries in the submission queue**. So a declined submit has
    // not lost the operations, and rundown reporting the error is correct --
    // what was missing was the guarantee that makes reporting it useful: the
    // ring is resumable, and calling again is what runs the queued entries.
    //
    // Deciding *when* to call again is deliberately not this crate's business.
    // This test therefore plays the caller: it sees the error, chooses to try
    // again, and the work completes.
    //
    // Seed 0x1A is the one M26.3's sweep found. Pinned rather than swept
    // because the point is reproducing one observation exactly.
    let resolver = Resolver::new(0x1A);
    let replay = resolver.replay_hint();
    let (path, refusals, finished) = resolver.scoped(|_| {
        let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
        let (file, path) = scratch("declined");

        let mut batch = Batch::new(&mut ring);
        for _ in 0..6 {
            let _token = batch
                .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
        }
        let _ = batch.submit();

        // The caller's policy, which is all this crate asks of it: keep
        // going while it chooses to. A real consumer would back off here; the
        // point is that it is *their* loop and not ours.
        let mut refusals = 0_usize;
        let mut finished = false;
        for _ in 0..64 {
            match ring.run_down_within(std::time::Duration::from_millis(50)) {
                Ok(true) => {
                    finished = true;
                    break;
                }
                Ok(false) => {}
                Err(_) => refusals += 1,
            }
        }
        (path, refusals, finished)
    });
    let _ = std::fs::remove_file(path);

    assert!(
        refusals > 0,
        "{replay}: this seed declines a submit, which is the condition under test"
    );
    assert!(
        finished,
        "{replay}: after a declined submit the entries remain queued, so a caller that tries \
         again must be able to finish -- that is the guarantee SubmitIoRing's documentation \
         gives and what makes reporting the error useful rather than terminal"
    );
}

#[test]
fn an_expired_wait_is_a_successful_submit() {
    // M26.8's correction, and the reason it is not a judgement call:
    // `SubmitIoRing` documents IORING_E_WAIT_TIMEOUT as "All operations were
    // submitted without error and the subsequent wait timed out". Reporting
    // that as an Err is M21.6's defect at the one site that sweep missed --
    // and the damage is not merely a wrong sign, because an Err from a submit
    // means the entries are still queued, so a caller who frees their buffers
    // on seeing one hands the kernel freed memory next time.
    //
    // RS-P-4 makes an expired wait reachable on demand, so this is a test
    // rather than an argument about a rare timing. Swept rather than pinned to
    // one seed: the clause is a permission the resolver takes sometimes, and
    // picking a seed that happens to take it would make the test a hostage to
    // the mixer. Every seed that expires must report success.
    let mut expiring_seeds = 0_usize;
    for seed in 0..64_u64 {
        let resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_expire_waits: true,
                may_pend: true,
                ..ResolverConfig::narrowest()
            },
        );
        let replay = resolver.replay_hint();
        let path = resolver.scoped(|watch| {
            let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
            let (file, path) = scratch("expired-submit");

            let outcome = {
                let mut batch = Batch::new(&mut ring);
                for _ in 0..4 {
                    let _token = batch
                        .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                        .expect("a flush builds");
                }
                // Ask to wait, which is what lets RS-P-4 apply.
                batch.submit_and_wait(4, 50)
            };

            if watch.stats().expired_waits > 0 {
                expiring_seeds += 1;
                assert!(
                    outcome.is_ok(),
                    "{replay}: an expired wait means every entry was submitted, so \
                     submit_and_wait must report success -- see SubmitIoRing's documented \
                     return values. Got {outcome:?}"
                );
            }

            // Whatever the wait did, the operations were submitted -- so they
            // run down normally rather than needing recovery.
            ring.run_down().expect("rundown");
            path
        });
        let _ = std::fs::remove_file(path);
    }

    assert!(
        expiring_seeds > 0,
        "no seed expired a wait, so this test checked nothing about IORING_E_WAIT_TIMEOUT"
    );
}

#[test]
fn run_down_within_honours_its_bound_and_reports_rather_than_deciding() {
    // The shape the audit found wrong: `run_down` waited in segments with no
    // period to sit inside, which made "how long to keep trying" this crate's
    // policy. The bounded form hands that back.
    //
    // A resolution that has not completed the work yet is the case where the
    // bound has anything to do, so RS-P-1 supplies one.
    let resolver = Resolver::with_config(
        11,
        ResolverConfig {
            may_pend: true,
            ..ResolverConfig::narrowest()
        },
    );
    let replay = resolver.replay_hint();
    let path = resolver.scoped(|_| {
        let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
        let (file, path) = scratch("bounded-rundown");
        {
            let mut batch = Batch::new(&mut ring);
            for _ in 0..4 {
                let _token = batch
                    .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                    .expect("a flush builds");
            }
            batch.submit().expect("the submit is answered");
        }

        // A zero bound is the honest spelling of "do not wait": it reports
        // rather than blocking, which is the whole point of the shape.
        let started = std::time::Instant::now();
        let finished = ring
            .run_down_within(std::time::Duration::ZERO)
            .expect("a zero-bound rundown does not fail");
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "{replay}: a zero bound must not block"
        );
        assert!(
            !finished || ring.outstanding() == 0,
            "{replay}: reporting finished must mean nothing is outstanding"
        );

        // And the caller's own loop finishes it, because that is their policy.
        for _ in 0..256 {
            if ring
                .run_down_within(std::time::Duration::from_millis(20))
                .expect("rundown")
            {
                break;
            }
        }
        assert_eq!(
            ring.outstanding(),
            0,
            "{replay}: a caller who keeps calling reaches quiescence"
        );
        path
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_pending_completion_defeats_try_pop_and_not_pop_within() {
    // The defect class M26.7 audited, demonstrated rather than described.
    //
    // Thirty-one assertions across five kernel tests read `try_pop()` straight
    // after `submit_and_wait` and expected a completion to be *there*. That is
    // not something this crate promises -- `pop_within`'s own documentation
    // says a submit-side wait's return "promises nothing about poppability",
    // and RS-P-5 states it as a permission the platform holds. The assertions
    // passed anyway, because on a buffered handle the operation completes
    // inside the submit; D-40 measured that at 80 of 80 attempts. Change the
    // handle and the same assertion gives the opposite answer, which is what
    // makes it a frozen observation rather than a contract.
    //
    // A resolver makes the pending case reachable on demand, so the difference
    // between the two spellings is a test rather than an argument. This is the
    // guard for the restatement: if `try_pop` ever starts satisfying this, the
    // premise of the audit was wrong and this test says so.
    let resolver = Resolver::with_config(
        0x9,
        ResolverConfig {
            may_pend: true,
            ..ResolverConfig::narrowest()
        },
    );
    let replay = resolver.replay_hint();

    let path = resolver.scoped(|_| {
        let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");
        let (file, path) = scratch("pending");

        {
            let mut batch = Batch::new(&mut ring);
            batch
                .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
            batch.submit().expect("the submit is answered");
        }

        // The frozen-observation spelling. Under a resolution that pends, the
        // completion is not there yet -- so a test written this way would have
        // failed here rather than at anything it meant to check.
        assert!(
            ring.try_pop().expect("try_pop").is_none(),
            "{replay}: this resolution pends, so nothing is poppable the instant the submit \
             returns -- if that changed, this test's premise is gone"
        );
        assert_eq!(
            ring.outstanding(),
            1,
            "{replay}: the operation is outstanding, not lost"
        );

        // The restated spelling: this crate's own contract, which holds under
        // every resolution rather than on one kind of handle.
        let (_completion, held) = ring
            .pop_within_held(std::time::Duration::from_secs(5))
            .expect("pop_within")
            .expect("a completion arrives within the bound");
        assert!(
            held.is_some(),
            "{replay}: the completion identifies its operation"
        );

        ring.run_down().expect("rundown");
        path
    });
    let _ = std::fs::remove_file(path);
}

#[test]
fn a_thread_with_nothing_installed_still_talks_to_the_kernel() {
    // The seam's transparency, asserted from the far side. This is the same
    // property `M26.2` verified by running its suite both ways, restated here
    // because a resolver is the thing most likely to break it: a responder
    // that leaked past its guard would divert every ring in the process, and
    // the failure would look like a flaky kernel rather than like a harness
    // defect.
    let (file, path) = scratch("uninstalled");
    let mut ring = ResolverRing::with_inventory(64, 128).expect("a ring");

    {
        let resolver = Resolver::with_config(1, ResolverConfig::narrowest());
        let watch = resolver.scoped(|watch| watch.clone());
        // The guard has dropped. Nothing installed on this thread now.
        assert_eq!(watch.stats().built, 0, "the scope did nothing of its own");

        let mut batch = Batch::new(&mut ring);
        batch
            .flush_owned(&file, (), FlushCoverage::Unordered, FlushMode::Default)
            .expect("a flush builds");
        batch.submit().expect("the kernel accepts the submit");
        let (_completion, held) = ring
            .pop_within_held(std::time::Duration::from_secs(5))
            .expect("the kernel answers")
            .expect("a real completion arrives");
        assert!(held.is_some());
        assert_eq!(
            watch.stats().built,
            0,
            "an uninstalled resolver must not have seen the call -- the seam leaked"
        );
    }

    ring.run_down().expect("rundown");
    drop(file);
    let _ = std::fs::remove_file(path);
}
