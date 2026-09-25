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
        let mut ring = IoRing::new(64, 128).expect("a ring");
        let (file, path) = scratch("reachable");

        let token = {
            let mut batch = Batch::new(&mut ring);
            let token = batch
                .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
            batch.submit().expect("the submit is answered");
            token
        };

        assert_eq!(
            ring.outstanding(),
            1,
            "the crate's accounting counts the operation whether the kernel saw it or not"
        );

        let completion = ring
            .pop_within(std::time::Duration::from_secs(5))
            .expect("the pop is answered")
            .expect("a completion arrives within the bound");
        assert!(
            token.claim_if(&completion).is_ok(),
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
            let mut ring = IoRing::new(64, 128).expect("a ring");
            let (file, path) = scratch("rundown");

            let mut tokens = Vec::new();
            let mut batch = Batch::new(&mut ring);
            for _ in 0..6 {
                tokens.push(
                    batch
                        .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                        .expect("a flush builds"),
                );
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
fn a_declined_submit_reaches_run_down_as_an_error() {
    // The finding the test above declares, pinned as its own test so it is a
    // recorded observation rather than a comment. This asserts what the crate
    // *does* today, not what it should do -- so when `M26.8` settles the
    // question, this test is the one that has to change, and changing it is
    // the signal that the behaviour did.
    //
    // Seed 0x1A is the one the sweep above found. Pinned rather than swept
    // because the point is reproducing one observation exactly.
    let resolver = Resolver::new(0x1A);
    let replay = resolver.replay_hint();
    let (path, refused) = resolver.scoped(|_| {
        let mut ring = IoRing::new(64, 128).expect("a ring");
        let (file, path) = scratch("declined");

        let mut batch = Batch::new(&mut ring);
        for _ in 0..6 {
            let _token = batch
                .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                .expect("a flush builds");
        }
        let _ = batch.submit();

        let refused = ring.run_down().is_err();
        // Drain whatever is left so the ring is not dropped mid-flight; this
        // is the recovery a caller has no documented route to today, which is
        // itself part of what M26.8 is about.
        while ring.outstanding() > 0 {
            let _ = ring.run_down();
        }
        (path, refused)
    });
    let _ = std::fs::remove_file(path);

    assert!(
        refused,
        "{replay}: this seed declines a submit under RS-P-7; if run_down no longer reports \
         that as an error, M26.8 has been answered and this test records the old behaviour"
    );
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
    let mut ring = IoRing::new(64, 128).expect("a ring");

    {
        let resolver = Resolver::with_config(1, ResolverConfig::narrowest());
        let watch = resolver.scoped(|watch| watch.clone());
        // The guard has dropped. Nothing installed on this thread now.
        assert_eq!(watch.stats().built, 0, "the scope did nothing of its own");

        let mut batch = Batch::new(&mut ring);
        let token = batch
            .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
            .expect("a flush builds");
        batch.submit().expect("the kernel accepts the submit");
        let completion = ring
            .pop_within(std::time::Duration::from_secs(5))
            .expect("the kernel answers")
            .expect("a real completion arrives");
        assert!(token.claim_if(&completion).is_ok());
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
