// Copyright (c) Mike Grier
//! Calibration: showing the resolver can go red (M26.5).
//!
//! # Why this file exists
//!
//! [D-41](../DESIGN-NOTES.md#d-41)'s corollary is the rule it enforces: **a
//! green result from an instrument nobody has shown can go red is not
//! evidence**. `M26.3` built a resolver and `M26.4` stated five properties
//! over it; both currently report green, and neither has yet been shown
//! capable of reporting anything else against a defect that really happened.
//!
//! That is not a theoretical worry in this repository. `M17.4` calibrated the
//! generated-sequence suite by reverting `D-20`'s setup signal -- issue #47
//! exactly as it shipped -- and the suite reported **green**. It sampled the
//! right states and drained by polling, which recovers every completion
//! whether or not the ring ever signalled. Sampling the right state is not the
//! same as being sensitive to the defect that lives in it, and the difference
//! was invisible until somebody tried it. The design session behind `M26`
//! produced two more instruments of the same shape and said so.
//!
//! # The two defects, and why they are calibrated differently
//!
//! **[D-47](../DESIGN-NOTES.md#d-47): a consumer that believes a covering
//! flush holds back what follows it.** [D-24](../DESIGN-NOTES.md#d-24) claimed
//! that and `D-47` withdrew it over roughly 4,500 trials. The defect is in a
//! *consumer*, not in this crate, so there is nothing here to mutate -- the
//! defective consumer has to be written down, and it is, below. The test
//! asserts that the resolver **breaks** it.
//!
//! **`M21.6`: an expired wait treated as a failure.** That one was in this
//! crate: `pop_within` returned `Err` on every ordinary timeout until
//! `wait_outcome` was given its `IORING_E_WAIT_TIMEOUT` arm. Re-injecting it means
//! mutating the crate, which a test cannot do, so it lives in
//! [sabotage.json](../sabotage.json) instead and is swept with everything
//! else. What *this* file contributes is the precondition that sabotage needs
//! to mean anything: that `RS-P-4` actually reaches `pop_within` at all. A
//! sabotage of code the suite never executes would be caught for some
//! unrelated reason, or not at all, and either way would say nothing.
//!
//! # These tests fail if the resolver gets *narrower*
//!
//! That is their whole purpose and it is worth being explicit, because it
//! inverts the usual reading. A failure here does not mean the crate broke; it
//! means the instrument stopped being able to see something it must see.

#![cfg(all(windows, feature = "kernel-seam"))]

use std::time::Duration;

use windows_ioring_sys::sys::{Resolver, ResolverConfig};
use windows_ioring_sys::{
    Batch, Completion, FlushCoverage, FlushMode, IoRing, PushOptions, SharedFile, Token,
    WriteCaching,
};

/// Seeds each calibration sweeps.
///
/// Fixed rather than clock-derived, unlike the generated suites: a calibration
/// that sometimes could not demonstrate its own sensitivity would be the exact
/// failure it exists to prevent, arriving as a flake.
const SEEDS: std::ops::Range<u64> = 0..2048;
const SEED_COUNT: usize = 2048;

/// Bytes per write.
const BUF_LEN: usize = 256;

/// How many consultations one sweep may take to drain.
const BUDGET: usize = 512;

fn scratch(tag: &str) -> (SharedFile, std::path::PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-m26-5-{}-{tag}.tmp",
        std::process::id()
    ));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .expect("a scratch file");
    (SharedFile::new(file.into()), path)
}

/// Tokens awaiting completion, claimed by identity as completions arrive.
#[derive(Default)]
struct Held {
    flushes: Vec<Token<SharedFile>>,
    writes: Vec<Token<(Vec<u8>, SharedFile)>>,
}

impl Held {
    /// Claim whichever token this completion belongs to.
    ///
    /// Returns false if none matches, which would itself be `RS-C-2` broken --
    /// asserted by the caller rather than ignored, so a calibration cannot
    /// pass by losing track of an operation.
    fn claim(&mut self, completion: &Completion) -> bool {
        let user_data = completion.user_data();
        if let Some(at) = self
            .flushes
            .iter()
            .position(|token| token.id() == user_data)
        {
            return self.flushes.remove(at).claim_if(completion).is_ok();
        }
        if let Some(at) = self.writes.iter().position(|token| token.id() == user_data) {
            return self.writes.remove(at).claim_if(completion).is_ok();
        }
        false
    }
}

/// Drain the ring, returning completion identities in the order they arrived.
fn drain_in_order(ring: &mut IoRing, held: &mut Held, expected: usize) -> Vec<usize> {
    let mut order = Vec::new();
    for _ in 0..BUDGET {
        if order.len() == expected {
            return order;
        }
        // A declined submit under RS-P-7 surfaces here as an error, which is
        // within `pop_within`'s documented contract and is retried rather than
        // treated as a failure -- see M26.4, which established that.
        match ring.pop_within(Duration::from_millis(5)) {
            Ok(Some(completion)) => {
                assert!(
                    held.claim(&completion),
                    "a completion arrived for {:#x} with no live token to match it",
                    completion.user_data()
                );
                order.push(completion.user_data());
            }
            Ok(None) | Err(_) => continue,
        }
    }
    panic!(
        "drain did not finish: {} of {expected} completions within the budget",
        order.len()
    );
}

// ------------------------------------------------------ D-47's defect ------

/// The consumer `D-24` licensed and `D-47` withdrew, written out so the
/// resolver has something to break.
///
/// The belief is stated the way a consumer would actually hold it: a covering
/// flush closes an epoch, so any operation queued *after* that flush belongs
/// to the next epoch and cannot be seen until the flush itself has been. A
/// real consumer with this belief does something consequential on it -- rolls
/// an epoch counter, reuses an arena, reports a record durable -- and this one
/// only records that the belief failed, because the failure is what is being
/// measured.
#[derive(Debug, Default)]
struct HoldBackBeliever {
    flush_seen: bool,
    /// Set when an operation queued after the covering flush arrived first.
    broken_by: Option<usize>,
    /// Set if an operation queued *before* the flush arrived after it, which
    /// would be `RS-C-4` violated -- a different failure entirely, and one
    /// this test must not mistake for the one it is looking for.
    drain_half_broken_by: Option<usize>,
}

impl HoldBackBeliever {
    fn observe(&mut self, user_data: usize, flush: usize, before: &[usize], after: &[usize]) {
        if user_data == flush {
            self.flush_seen = true;
            return;
        }
        if after.contains(&user_data) && !self.flush_seen {
            self.broken_by.get_or_insert(user_data);
        }
        if before.contains(&user_data) && self.flush_seen {
            self.drain_half_broken_by.get_or_insert(user_data);
        }
    }
}

#[test]
fn the_resolver_breaks_a_consumer_that_believes_the_drain_flag_holds_back() {
    // D-47's defect, re-injected. The claim being calibrated is that the
    // resolver can *see* this class of error -- so the assertion is that some
    // seed breaks the believer, and a run where none did would mean the
    // instrument had gone narrow.
    let (file, path) = scratch("holdback");
    let mut broken = 0_usize;
    let mut drain_half_failures = Vec::new();

    for seed in SEEDS {
        // Narrowed to the one freedom under test. Operation failures and
        // declined submits would still leave the sweep correct, but they would
        // make a failure here ambiguous about which freedom caused it, and a
        // calibration that cannot say what it detected is not calibrated.
        let resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_reorder: true,
                may_pend: true,
                ..ResolverConfig::narrowest()
            },
        );
        let outcome = resolver.scoped(|_| {
            let mut ring = IoRing::new(64, 128).expect("a ring");
            let mut held = Held::default();
            let (before, flush, after) = {
                let mut batch = Batch::new(&mut ring);
                let mut before = Vec::new();
                for _ in 0..2 {
                    let token = batch
                        .write(
                            &file,
                            vec![0_u8; BUF_LEN],
                            0,
                            PushOptions::new(),
                            WriteCaching::Cached,
                        )
                        .expect("a write builds");
                    before.push(token.id());
                    held.writes.push(token);
                }
                let flush_token = batch
                    .flush(
                        &file,
                        FlushCoverage::CoversPrecedingOperations,
                        FlushMode::Default,
                    )
                    .expect("a covering flush builds");
                let flush = flush_token.id();
                held.flushes.push(flush_token);

                let mut after = Vec::new();
                for _ in 0..3 {
                    let token = batch
                        .write(
                            &file,
                            vec![0_u8; BUF_LEN],
                            0,
                            PushOptions::new(),
                            WriteCaching::Cached,
                        )
                        .expect("a write builds");
                    after.push(token.id());
                    held.writes.push(token);
                }
                batch.submit().expect("the submit is answered");
                (before, flush, after)
            };

            let order = drain_in_order(&mut ring, &mut held, 6);
            let mut believer = HoldBackBeliever::default();
            for user_data in &order {
                believer.observe(*user_data, flush, &before, &after);
            }
            ring.run_down().expect("rundown");
            believer
        });

        if outcome.broken_by.is_some() {
            broken += 1;
        }
        if let Some(user_data) = outcome.drain_half_broken_by {
            drain_half_failures.push((seed, user_data));
        }
    }
    let _ = std::fs::remove_file(path);

    // The half that must NOT break. RS-C-4 forbids it, and a resolver that
    // broke both halves would make the assertion below pass for the wrong
    // reason -- the believer would be "broken" by a violation of the
    // guarantee this crate's durability story rests on, rather than by the
    // one-sidedness D-47 measured.
    assert!(
        drain_half_failures.is_empty(),
        "RS-C-4 was violated, so this calibration is measuring the wrong failure: {:?}",
        drain_half_failures
    );

    assert!(
        broken > 0,
        "no seed of {SEED_COUNT} broke a consumer that assumes a covering flush holds back what \
         follows it. D-47 withdrew that assumption over roughly 4,500 trials, so a resolver \
         that cannot exhibit the one-sidedness has gone narrower than the space and would \
         report green on the defect class it exists to catch"
    );
    // Printed rather than only asserted: the threshold is "more than none",
    // which is the honest bar for a demonstration of sensitivity, but a reader
    // deciding how much to trust this instrument wants to know whether that
    // was one seed or most of them.
    //
    // Expect this to be far higher than the rate D-47 measured on real
    // hardware, and note that the difference is not the resolver being
    // unfaithful. RESPONSE-SPACE.md deliberately carries no rates: a resolver
    // reproducing an observed frequency would be a model of Windows, which is
    // the trap D-52 was opened to escape. Its job is to explore the space, and
    // a defect class that occurs in under one trial in a hundred on hardware
    // is precisely the one a rate-free resolver earns its keep on.
    eprintln!("D-47 calibration: {broken} of {SEED_COUNT} seeds broke the believer");
}

// ------------------------------------------------------ M21.6's defect -----

#[test]
fn an_expired_wait_reaches_pop_within() {
    // The precondition `M21.6`'s sabotage needs. That case reverts
    // `wait_outcome`'s IORING_E_WAIT_TIMEOUT arm, which only means something if an
    // expired wait actually arrives there -- a mutation to code the suite
    // never executes is caught for an unrelated reason or not at all, and
    // either way measures nothing.
    //
    // Asserted through the resolver's own counter rather than inferred from a
    // timing, because "the call took a while" is not evidence that a wait
    // expired.
    let (file, path) = scratch("expired");
    let mut seeds_with_an_expired_wait = 0_usize;

    for seed in SEEDS {
        let resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_expire_waits: true,
                may_pend: true,
                ..ResolverConfig::narrowest()
            },
        );
        let expired = resolver.scoped(|watch| {
            let mut ring = IoRing::new(64, 128).expect("a ring");
            let mut held = Held::default();
            {
                let mut batch = Batch::new(&mut ring);
                for _ in 0..4 {
                    let token = batch
                        .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                        .expect("a flush builds");
                    held.flushes.push(token);
                }
                batch.submit().expect("the submit is answered");
            }
            // `pop_within` is the only caller here, so any expired wait the
            // resolver records was answered to it.
            drain_in_order(&mut ring, &mut held, 4);
            ring.run_down().expect("rundown");
            watch.stats().expired_waits
        });
        if expired > 0 {
            seeds_with_an_expired_wait += 1;
        }
    }
    let _ = std::fs::remove_file(path);

    assert!(
        seeds_with_an_expired_wait > SEED_COUNT / 4,
        "only {seeds_with_an_expired_wait} of {SEED_COUNT} seeds expired a wait inside \
         pop_within, which is too few for the M21.6 sabotage to be reliably exercised -- that \
         case would then be caught by accident or not at all"
    );
    eprintln!(
        "M21.6 calibration: {seeds_with_an_expired_wait} of {SEED_COUNT} seeds expired a wait \
         inside pop_within"
    );
}

#[test]
fn an_expired_wait_is_not_reported_as_a_failure() {
    // `M21.6`'s correction, stated as the property it is rather than left to
    // the sabotage alone. An expired wait is the ordinary outcome of asking to
    // block for a bounded time; `pop_within` documents `Ok(None)` for it, and
    // returning `Err` instead is what the defect did.
    //
    // This is the assertion the sabotage turns red, so the two are a pair: the
    // sabotage shows the assertion is load-bearing, and the assertion is what
    // gives the sabotage something to break.
    let (file, path) = scratch("notafailure");
    for seed in SEEDS {
        let resolver = Resolver::with_config(
            seed,
            ResolverConfig {
                may_expire_waits: true,
                may_pend: true,
                ..ResolverConfig::narrowest()
            },
        );
        let replay = resolver.replay_hint();
        resolver.scoped(|watch| {
            let mut ring = IoRing::new(64, 128).expect("a ring");
            let mut held = Held::default();
            {
                let mut batch = Batch::new(&mut ring);
                for _ in 0..3 {
                    held.flushes.push(
                        batch
                            .flush(&file, FlushCoverage::Unordered, FlushMode::Default)
                            .expect("a flush builds"),
                    );
                }
                batch.submit().expect("the submit is answered");
            }

            let mut seen = 0_usize;
            for _ in 0..BUDGET {
                if seen == 3 {
                    break;
                }
                match ring.pop_within(Duration::from_millis(5)) {
                    Ok(Some(completion)) => {
                        assert!(held.claim(&completion));
                        seen += 1;
                    }
                    Ok(None) => {}
                    Err(error) => panic!(
                        "{replay}: pop_within reported an error after {} expired wait(s): \
                         {error}. An expired wait is not a failure of the ring (M21.6)",
                        watch.stats().expired_waits
                    ),
                }
            }
            assert_eq!(seen, 3, "{replay}: not every operation completed");
            ring.run_down().expect("rundown");
        });
    }
    let _ = std::fs::remove_file(path);
}
