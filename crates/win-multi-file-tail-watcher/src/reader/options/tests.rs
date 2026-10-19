// Copyright (c) 2026 Mike Grier
use std::io;

use windows_ioring_sys::{IoRing, capabilities};

use super::{RingLimits, TailReaderOptions, client_queue_bounds};
use crate::Watcher;
use crate::pool::BufferPool;
use crate::read_engine::{ReadEngine, Stop};
use crate::read_sq::ReadEntry;
use crate::reader::{Cause, OptionsError};
use crate::sq::queue_of;
use crate::test_support::{TempDir, wtf};

fn options() -> TailReaderOptions {
    TailReaderOptions::default()
}

fn refused(options: &TailReaderOptions) -> OptionsError {
    options
        .validate()
        .expect_err("these options should be refused")
}

#[test]
fn the_defaults_are_valid() {
    assert!(options().validate().is_ok());
}

#[test]
fn the_default_ring_is_the_ring_the_caller_gets() {
    let defaults = options();
    assert_eq!(
        ring_gives(defaults.ring_sq_size, defaults.ring_cq_size),
        Some((defaults.ring_sq_size, defaults.ring_cq_size)),
        "the ring raises nothing about the default sizes"
    );
}

#[test]
fn a_buffer_len_of_zero_is_refused_and_one_is_not() {
    let mut options = options();
    options.buffer_len = 0;
    assert!(matches!(
        refused(&options),
        OptionsError::BufferLen {
            given: 0,
            cause: Cause::Zero
        }
    ));
    options.buffer_len = 1;
    assert!(options.validate().is_ok());
}

#[test]
fn a_buffer_len_is_at_most_what_a_ring_read_takes() {
    let most = usize::try_from(u32::MAX).expect("u32 fits usize here");
    let mut options = options();
    options.buffer_len = most;
    assert!(
        options.validate().is_ok(),
        "the largest read is accepted, and nothing is allocated to find out"
    );
    #[cfg(target_pointer_width = "64")]
    {
        options.buffer_len = most + 1;
        assert_eq!(
            refused(&options).cause(),
            Some(Cause::Above {
                most: u64::from(u32::MAX)
            })
        );
        options.buffer_len = usize::MAX;
        assert_eq!(
            refused(&options).cause(),
            Some(Cause::Above {
                most: u64::from(u32::MAX)
            })
        );
    }
}

#[test]
fn a_buffer_count_of_zero_is_refused_and_any_other_is_accepted_unallocated() {
    let mut options = options();
    options.buffer_count = 0;
    assert!(matches!(
        refused(&options),
        OptionsError::BufferCount {
            given: 0,
            cause: Cause::Zero
        }
    ));
    for count in [1, 2, 3, 64, 1000, 1 << 20, usize::MAX] {
        options.buffer_count = count;
        assert!(
            options.validate().is_ok(),
            "a count of {count}: whether the memory is there is the allocator's answer when the layer is made"
        );
    }
}

#[test]
fn the_queue_bounds_are_read_from_its_refusal_of_zero() {
    let Err(refusal) = queue_of::<()>(0) else {
        panic!("the queue refuses a capacity of zero");
    };
    assert_eq!(
        client_queue_bounds(),
        (
            u64::try_from(refusal.capacity().min_valid).expect("fits"),
            u64::try_from(refusal.capacity().max_valid).expect("fits")
        )
    );
}

/// The client queue's capacity rule is written here a second time, so it is compared with the
/// queue's own verdict: the two must accept and refuse the same capacities.
#[test]
fn the_client_sq_capacity_is_refused_exactly_when_the_queue_refuses_it() {
    let capacities = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 100, 127, 128, 129, 255,
        256, 1000, 1024, 4096, 65_536,
    ];
    for capacity in capacities {
        let mut options = options();
        options.client_sq_capacity = capacity;
        let ours = options.validate().is_ok();
        let theirs = queue_of::<()>(capacity).is_ok();
        assert_eq!(ours, theirs, "a client queue of {capacity}");
    }

    let (least, most) = client_queue_bounds();
    let least_usize = usize::try_from(least).expect("fits");
    let most_usize = usize::try_from(most).expect("fits");
    // At the bounds, and past each, where the queue refuses before it allocates.
    let mut options = options();
    options.client_sq_capacity = most_usize;
    assert!(options.validate().is_ok(), "the largest the queue reports");
    options.client_sq_capacity = least_usize / 2;
    assert_eq!(
        refused(&options).cause(),
        Some(Cause::Below { least }),
        "below the least"
    );
    for capacity in [most_usize.checked_mul(2), Some(usize::MAX), Some(1 << 63)]
        .into_iter()
        .flatten()
    {
        options.client_sq_capacity = capacity;
        let ours = refused(&options).cause();
        let Err(theirs) = queue_of::<()>(capacity) else {
            panic!("the queue refuses a capacity of {capacity}");
        };
        let expected = if capacity.is_power_of_two() {
            Cause::Above {
                most: u64::try_from(theirs.capacity().max_valid).expect("fits"),
            }
        } else {
            Cause::NotAPowerOfTwo
        };
        assert_eq!(ours, Some(expected), "a client queue of {capacity}");
    }
    options.client_sq_capacity = 0;
    assert_eq!(refused(&options).cause(), Some(Cause::Zero));
    options.client_sq_capacity = 3;
    assert_eq!(refused(&options).cause(), Some(Cause::NotAPowerOfTwo));
}

fn limits(max_sq: u32, max_cq: u32) -> RingLimits {
    RingLimits { max_sq, max_cq }
}

fn ring_options(sq: u32, cq: u32) -> TailReaderOptions {
    let mut options = options();
    options.ring_sq_size = sq;
    options.ring_cq_size = cq;
    options
}

#[test]
fn the_ring_sizes_follow_the_rules_against_a_given_limit() {
    let limits = limits(1024, 4096);
    let least = u64::from(TailReaderOptions::MIN_RING_SQ_SIZE);
    let per_sq = u64::from(TailReaderOptions::MIN_RING_CQ_PER_SQ);

    // A submission queue, and its refusal if it is one.
    let sq_cases: [(u32, Option<Cause>); 8] = [
        (0, Some(Cause::Zero)),
        (1, Some(Cause::Below { least })),
        (7, Some(Cause::NotAPowerOfTwo)),
        (4, Some(Cause::Below { least })),
        (8, None),
        (9, Some(Cause::NotAPowerOfTwo)),
        (1024, None),
        (2048, Some(Cause::Above { most: 1024 })),
    ];
    for (sq, expected) in sq_cases {
        // A completion queue large enough to be accepted whatever the submission queue is, so a
        // refusal here is the submission queue's.
        let result = ring_options(sq, 4096).check_ring(limits);
        match expected {
            Some(cause) => assert!(
                matches!(&result, Err(OptionsError::RingSqSize { given, cause: c }) if *given == sq && *c == cause),
                "a submission queue of {sq}: {result:?}"
            ),
            None => assert!(
                !matches!(result, Err(OptionsError::RingSqSize { .. })),
                "a submission queue of {sq}: {result:?}"
            ),
        }
    }

    // A completion queue given a valid submission queue, and its refusal if it is one.
    let cq_cases: [(u32, u32, Option<Cause>); 11] = [
        (8, 0, Some(Cause::Zero)),
        (8, 15, Some(Cause::NotAPowerOfTwo)),
        (8, 8, Some(Cause::Below { least: 8 * per_sq })),
        (8, 16, None),
        (8, 4096, None),
        (8, 8192, Some(Cause::Above { most: 4096 })),
        (
            1024,
            1024,
            Some(Cause::Below {
                least: 1024 * per_sq,
            }),
        ),
        (1024, 2048, None),
        (1024, 4096, None),
        (1024, 8192, Some(Cause::Above { most: 4096 })),
        (64, 128, None),
    ];
    for (sq, cq, expected) in cq_cases {
        let result = ring_options(sq, cq).check_ring(limits);
        match expected {
            Some(cause) => assert!(
                matches!(
                    &result,
                    Err(OptionsError::RingCqSize { given, ring_sq_size, cause: c })
                        if *given == cq && *ring_sq_size == sq && *c == cause
                ),
                "a ring of {sq} and {cq}: {result:?}"
            ),
            None => assert!(result.is_ok(), "a ring of {sq} and {cq}: {result:?}"),
        }
    }
}

#[test]
fn when_no_completion_queue_can_satisfy_both_bounds_the_one_it_is_nearer_is_named() {
    // A submission queue so large that twice it is past the largest completion queue: below the
    // least is reported for a size under it, and above the most for a size at or over it.
    let limits = limits(1024, 1024);
    let below = ring_options(1024, 1024).check_ring(limits);
    assert!(matches!(
        below,
        Err(OptionsError::RingCqSize {
            cause: Cause::Below { least: 2048 },
            ..
        })
    ));
    let above = ring_options(1024, 2048).check_ring(limits);
    assert!(matches!(
        above,
        Err(OptionsError::RingCqSize {
            cause: Cause::Above { most: 1024 },
            ..
        })
    ));
}

/// The sizes the ring gave, or `None` if it refused.
fn ring_gives(sq: u32, cq: u32) -> Option<(u32, u32)> {
    let ring = IoRing::<(), ()>::with_inventory(sq, cq).ok()?;
    let info = ring.info().ok()?;
    Some((info.submission_queue_size, info.completion_queue_size))
}

/// The rule written in the options against the ring itself, in both directions: a pair of sizes is
/// accepted when the ring gives exactly those sizes, and refused when it raises, rounds, replaces
/// or refuses them. A ring that behaved differently would fail this.
#[test]
fn the_ring_sizes_are_accepted_exactly_when_the_ring_gives_exactly_those_sizes() {
    let submission = [
        0, 1, 2, 3, 4, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 128, 1024, 4096, 65_536,
        65_537, 131_072,
    ];
    let completion = [
        0, 1, 2, 15, 16, 17, 32, 64, 100, 128, 256, 1000, 1024, 2048, 8192, 65_536, 131_072,
        131_073, 262_144,
    ];
    let mut accepted = 0_usize;
    let mut refused_pairs = 0_usize;
    for sq in submission {
        for cq in completion {
            let ours = ring_options(sq, cq).validate().is_ok();
            let exact = ring_gives(sq, cq) == Some((sq, cq));
            assert_eq!(
                ours,
                exact,
                "a ring of {sq} and {cq}: the options said {ours}, the ring gave {:?}",
                ring_gives(sq, cq)
            );
            if ours {
                accepted += 1;
            } else {
                refused_pairs += 1;
            }
        }
    }
    assert!(accepted > 10, "the grid reaches accepted pairs: {accepted}");
    assert!(
        refused_pairs > 100,
        "the grid reaches refused pairs: {refused_pairs}"
    );
}

#[test]
fn the_largest_sizes_the_ring_reports_are_accepted_and_one_more_is_refused() {
    let caps = capabilities().expect("the ring's capabilities");
    assert!(
        ring_options(
            caps.max_submission_queue_size,
            caps.max_completion_queue_size
        )
        .validate()
        .is_ok()
    );
    let past_sq = ring_options(caps.max_submission_queue_size * 2, 64).validate();
    assert!(matches!(
        past_sq,
        Err(OptionsError::RingSqSize {
            cause: Cause::Above { .. },
            ..
        })
    ));
    let past_cq = ring_options(64, caps.max_completion_queue_size * 2).validate();
    assert!(matches!(
        past_cq,
        Err(OptionsError::RingCqSize {
            cause: Cause::Above { .. },
            ..
        })
    ));
}

#[test]
fn the_first_option_to_fail_is_the_one_reported() {
    let mut options = TailReaderOptions {
        buffer_len: 0,
        buffer_count: 0,
        client_sq_capacity: 3,
        ring_sq_size: 3,
        ring_cq_size: 3,
    };
    assert!(matches!(refused(&options), OptionsError::BufferLen { .. }));
    options.buffer_len = 1;
    assert!(matches!(
        refused(&options),
        OptionsError::BufferCount { .. }
    ));
    options.buffer_count = 1;
    assert!(matches!(
        refused(&options),
        OptionsError::ClientSqCapacity { .. }
    ));
    options.client_sq_capacity = 2;
    assert!(matches!(refused(&options), OptionsError::RingSqSize { .. }));
    options.ring_sq_size = 8;
    assert!(matches!(refused(&options), OptionsError::RingCqSize { .. }));
    options.ring_cq_size = 16;
    assert!(options.validate().is_ok());
}

#[test]
fn the_message_names_the_option_the_value_and_the_cause() {
    let cases: [(OptionsError, &[&str]); 6] = [
        (
            OptionsError::BufferLen {
                given: 0,
                cause: Cause::Zero,
            },
            &["buffer_len", "0", "zero"],
        ),
        (
            OptionsError::BufferCount {
                given: 0,
                cause: Cause::Zero,
            },
            &["buffer_count", "zero"],
        ),
        (
            OptionsError::ClientSqCapacity {
                given: 3,
                cause: Cause::NotAPowerOfTwo,
            },
            &["client_sq_capacity", "3", "power of two"],
        ),
        (
            OptionsError::RingSqSize {
                given: 4,
                cause: Cause::Below { least: 8 },
            },
            &["ring_sq_size", "4", "below", "8"],
        ),
        (
            OptionsError::RingCqSize {
                given: 64,
                ring_sq_size: 64,
                cause: Cause::Below { least: 128 },
            },
            &["ring_cq_size", "64", "ring_sq_size", "128"],
        ),
        (
            OptionsError::RingSqSize {
                given: 1 << 20,
                cause: Cause::Above { most: 65_536 },
            },
            &["ring_sq_size", "above", "65536"],
        ),
    ];
    for (error, parts) in cases {
        let message = error.to_string();
        for part in parts {
            assert!(
                message.contains(part),
                "{message:?} should contain {part:?}"
            );
        }
        assert!(error.cause().is_some());
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[test]
fn a_failure_to_read_the_rings_limits_carries_the_operating_systems_error_as_its_source() {
    let error = OptionsError::RingCapabilities(io::Error::from_raw_os_error(5));
    assert_eq!(error.cause(), None);
    assert!(error.to_string().contains("limits"));
    let source = std::error::Error::source(&error).expect("a source");
    let source = source
        .downcast_ref::<io::Error>()
        .expect("the source is the io::Error");
    assert_eq!(source.raw_os_error(), Some(5));
}

/// A file read whole through an engine made from `options`, as the pieces came, put in file order.
fn read_through(options: &TailReaderOptions, content: &[u8]) -> Vec<u8> {
    let watcher = Watcher::new(1024).expect("a watcher");
    let pool = BufferPool::new(options.buffer_len, options.buffer_count).expect("a pool");
    let mut engine = match ReadEngine::new(
        watcher,
        pool,
        options.client_sq_capacity,
        options.ring_sq_size,
        options.ring_cq_size,
    ) {
        Ok(engine) => engine,
        Err(error) => panic!("no engine from {options:?}: {error}"),
    };
    let dir = TempDir::new("opt-engine");
    let path = dir.file("a.log", content);
    let _registration = engine.register(&wtf(&path), 0, 7).expect("register");
    engine.watcher().poll_here();

    let mut pieces: Vec<(u64, Vec<u8>)> = Vec::new();
    for _ in 0..200_000 {
        let stop = engine.run().expect("run");
        for entry in engine.read_sq().drain() {
            match entry {
                ReadEntry::Data(data) => {
                    let (_, range, chunk) = data.into_parts();
                    pieces.push((range.start, chunk.to_vec()));
                }
                ReadEntry::PollDone => {}
                _ => panic!("an unexpected entry"),
            }
        }
        match stop {
            Stop::Idle if engine.in_flight() == 0 => {
                pieces.sort_by_key(|piece| piece.0);
                return pieces.into_iter().flat_map(|piece| piece.1).collect();
            }
            Stop::ClientFull => {}
            _ => engine.wait_for_completion(5_000).expect("wait"),
        }
    }
    panic!("the engine never settled");
}

/// Every combination the measurements found valid reaches a working engine that delivers a file's
/// bytes, from the defaults to a set of the size a heavier load would choose.
#[test]
fn valid_options_reach_a_working_engine() {
    let caps = capabilities().expect("the ring's capabilities");
    let content_of = |len: u32| -> Vec<u8> {
        (0..len)
            .map(|n| u8::try_from(n % 251).expect("below 251"))
            .collect()
    };
    // Each set with the length of the file it reads: a buffer of one byte makes a read of each.
    let sets = [
        (options(), 20_000),
        // Far above the defaults, as a build service would choose.
        (
            TailReaderOptions {
                buffer_len: 65_536,
                buffer_count: 256,
                client_sq_capacity: 4096,
                ring_sq_size: 4096,
                ring_cq_size: 8192,
            },
            200_000,
        ),
        // The least of everything.
        (
            TailReaderOptions {
                buffer_len: 1,
                buffer_count: 1,
                client_sq_capacity: 2,
                ring_sq_size: TailReaderOptions::MIN_RING_SQ_SIZE,
                ring_cq_size: TailReaderOptions::MIN_RING_SQ_SIZE
                    * TailReaderOptions::MIN_RING_CQ_PER_SQ,
            },
            300,
        ),
        // A completion queue far above twice the submission queue.
        (
            TailReaderOptions {
                buffer_len: 16,
                buffer_count: 8,
                client_sq_capacity: 8,
                ring_sq_size: 8,
                ring_cq_size: 1024,
            },
            20_000,
        ),
        // The largest ring the machine reports, with a small pool.
        (
            TailReaderOptions {
                buffer_len: 16,
                buffer_count: 8,
                client_sq_capacity: 8,
                ring_sq_size: caps.max_submission_queue_size,
                ring_cq_size: caps.max_completion_queue_size,
            },
            20_000,
        ),
    ];
    for (set, len) in sets {
        assert!(set.validate().is_ok(), "{set:?} should be valid");
        assert_eq!(
            ring_gives(set.ring_sq_size, set.ring_cq_size),
            Some((set.ring_sq_size, set.ring_cq_size)),
            "{set:?}: the ring raises nothing"
        );
        let content = content_of(len);
        assert_eq!(read_through(&set, &content), content, "{set:?}");
    }
}
