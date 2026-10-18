// Copyright (c) 2026 Mike Grier
//! A reader is refused for each size that cannot work, with the cause, before anything is
//! allocated or opened (TW-9.5, TW-D-16).
//!
//! Constructing a [`TailReader`] is where the options are checked, and it opens no file: a file is
//! opened by `register`, which a refused reader never offers. So that no file is opened is the same
//! fact as the construction failing.

use win_multi_file_tail_watcher::CreateError;
use win_multi_file_tail_watcher::reader::{
    Cause, OptionsError, TailReader, TailReaderError, TailReaderOptions,
};

/// The error a reader made from `options` is refused with.
fn refused(options: &TailReaderOptions) -> OptionsError {
    match TailReader::<u32>::new(options, 64) {
        Err(TailReaderError::Options(error)) => error,
        Err(other) => panic!("{options:?} was refused for another reason: {other}"),
        Ok(_) => panic!("{options:?} was accepted"),
    }
}

/// `options` with `set` applied to the defaults.
fn with(set: impl FnOnce(&mut TailReaderOptions)) -> TailReaderOptions {
    let mut options = TailReaderOptions::default();
    set(&mut options);
    options
}

fn assert_refused(options: &TailReaderOptions, field: &str, cause: Cause) {
    let error = refused(options);
    assert_eq!(error.cause(), Some(cause), "{options:?}");
    let message = error.to_string();
    assert!(message.contains(field), "{message} does not name {field}");
    // The error that comes out of the reader says it too, so the caller can show it as it is.
    let outer = TailReader::<u32>::new(options, 64)
        .expect_err("refused")
        .to_string();
    assert!(outer.contains(field), "{outer} does not name {field}");
}

#[test]
fn a_zero_size_is_refused_for_every_option() {
    assert_refused(&with(|o| o.buffer_len = 0), "buffer_len", Cause::Zero);
    assert_refused(&with(|o| o.buffer_count = 0), "buffer_count", Cause::Zero);
    assert_refused(
        &with(|o| o.client_sq_capacity = 0),
        "client_sq_capacity",
        Cause::Zero,
    );
    assert_refused(&with(|o| o.ring_sq_size = 0), "ring_sq_size", Cause::Zero);
    assert_refused(&with(|o| o.ring_cq_size = 0), "ring_cq_size", Cause::Zero);
}

#[test]
fn a_size_that_is_not_a_power_of_two_is_refused_where_one_is_required() {
    for size in [3usize, 5, 6, 7, 12, 100, 1000] {
        assert_refused(
            &with(|o| o.client_sq_capacity = size),
            "client_sq_capacity",
            Cause::NotAPowerOfTwo,
        );
        let ring = u32::try_from(size).expect("fits");
        assert_refused(
            &with(|o| o.ring_sq_size = ring),
            "ring_sq_size",
            Cause::NotAPowerOfTwo,
        );
        assert_refused(
            &with(|o| o.ring_cq_size = ring.max(129)),
            "ring_cq_size",
            Cause::NotAPowerOfTwo,
        );
    }
}

#[test]
fn a_buffer_size_or_count_that_is_not_a_power_of_two_is_accepted() {
    // Only the queue sizes are powers of two; the buffers are any size and any number.
    let options = with(|o| {
        o.buffer_len = 100;
        o.buffer_count = 7;
    });
    TailReader::<u32>::new(&options, 64).expect("a reader");
}

#[test]
fn a_ring_size_below_the_least_is_refused_and_not_raised() {
    assert_refused(
        &with(|o| o.ring_sq_size = 4),
        "ring_sq_size",
        Cause::Below {
            least: u64::from(TailReaderOptions::MIN_RING_SQ_SIZE),
        },
    );
    let sq = TailReaderOptions::DEFAULT_RING_SQ_SIZE;
    assert_refused(
        &with(|o| o.ring_cq_size = sq),
        "ring_cq_size",
        Cause::Below {
            least: u64::from(sq * TailReaderOptions::MIN_RING_CQ_PER_SQ),
        },
    );
}

#[test]
fn a_client_queue_of_one_is_refused_as_below_the_least() {
    let error = refused(&with(|o| o.client_sq_capacity = 1));
    assert!(
        matches!(error.cause(), Some(Cause::Below { least }) if least >= 2),
        "{error}"
    );
}

#[test]
fn a_size_above_the_most_is_refused() {
    assert_refused(
        &with(|o| o.buffer_len = 1 << 32),
        "buffer_len",
        Cause::Above {
            most: u64::from(u32::MAX),
        },
    );
    // The most a ring reports is not a figure to write down, so only that it is exceeded is asserted.
    let error = refused(&with(|o| o.ring_sq_size = 1 << 31));
    assert!(matches!(
        error,
        OptionsError::RingSqSize {
            cause: Cause::Above { .. },
            ..
        }
    ));
    let error = refused(&with(|o| o.ring_cq_size = 1 << 31));
    assert!(matches!(
        error,
        OptionsError::RingCqSize {
            cause: Cause::Above { .. },
            ..
        }
    ));
}

#[test]
fn the_first_refused_option_is_the_one_reported() {
    // Every option is wrong; the one reported is the first checked.
    let options = with(|o| {
        o.buffer_len = 0;
        o.buffer_count = 0;
        o.client_sq_capacity = 3;
        o.ring_sq_size = 3;
        o.ring_cq_size = 3;
    });
    assert!(matches!(refused(&options), OptionsError::BufferLen { .. }));
}

#[test]
fn buffers_the_allocator_will_not_give_are_an_error_and_not_an_abort() {
    // The options accept any count; the allocator answers at construction.
    for (len, count) in [(64, 1usize << 40), (1 << 20, usize::MAX)] {
        let options = with(|o| {
            o.buffer_len = len;
            o.buffer_count = count;
        });
        match TailReader::<u32>::new(&options, 64) {
            Err(TailReaderError::Allocation(_)) => {}
            Err(other) => panic!("{len} x {count}: {other}"),
            Ok(_) => panic!("{len} x {count} was given"),
        }
    }
}

#[test]
fn queues_the_allocator_will_not_give_are_an_error_and_not_an_abort() {
    // A capacity the queue accepts, so the options pass it: the memory is what is missing. `1 << 40`
    // is the capacity that used to abort the process, and the queue's own largest is refused before
    // the allocator is asked.
    for capacity in [1usize << 40, 1 << (usize::BITS - 2)] {
        let client = with(|o| o.client_sq_capacity = capacity);
        client
            .validate()
            .expect("the options accept a valid capacity");
        match TailReader::<u32>::new(&client, 64) {
            Err(TailReaderError::ClientSq(CreateError::Allocation(error))) => {
                assert_eq!(error.requested, capacity);
            }
            Err(other) => panic!("a client queue of {capacity}: {other:?}"),
            Ok(_) => panic!("a client queue of {capacity} was given"),
        }

        match TailReader::<u32>::new(&TailReaderOptions::default(), capacity) {
            Err(TailReaderError::Watcher(CreateError::Allocation(error))) => {
                assert_eq!(error.requested, capacity);
            }
            Err(other) => panic!("a watcher queue of {capacity}: {other:?}"),
            Ok(_) => panic!("a watcher queue of {capacity} was given"),
        }
    }
}

#[test]
fn the_default_options_are_accepted() {
    TailReader::<u32>::new(&TailReaderOptions::default(), 64).expect("a reader");
}
