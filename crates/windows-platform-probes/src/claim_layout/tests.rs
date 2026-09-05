// Copyright (c) Mike Grier.

//! Correctness checks for the three claim-word layouts.
//!
//! A measurement of a queue that loses or duplicates items is worthless, so
//! each layout is checked to deliver exactly what was pushed before it is
//! timed. These are not a substitute for `windows-waitable-queues`' own suite;
//! they establish that the duplicated protocol in this crate behaves like the
//! one it is standing in for.

use std::collections::HashSet;
use std::sync::Arc;
use std::thread;

use super::{deep, narrow, wide};

/// How many items each producer pushes in the concurrent checks.
const PER_PRODUCER: u64 = 2_000;

/// How many producers the concurrent checks run.
const PRODUCERS: u64 = 4;

macro_rules! layout_suite {
    ($module:ident, $name:ident) => {
        mod $name {
            use super::*;

            #[test]
            fn delivers_in_order_from_one_producer() {
                let queue = $module::Queue::with_capacity(8);
                for value in 0..64u64 {
                    while !queue.push(value) {
                        assert!(queue.pop().is_some(), "the queue may only refuse when full");
                    }
                }
                let mut drained = Vec::new();
                while let Some(value) = queue.pop() {
                    drained.push(value);
                }
                assert!(
                    drained.windows(2).all(|pair| pair[0] < pair[1]),
                    "a single producer's items must arrive in the order it pushed them"
                );
            }

            #[test]
            fn refuses_when_full_rather_than_overwriting() {
                let queue = $module::Queue::with_capacity(4);
                for value in 0..4u64 {
                    assert!(queue.push(value), "the first four fit");
                }
                assert!(!queue.push(4), "the fifth must be refused, not overwrite");
                for expected in 0..4u64 {
                    assert_eq!(queue.pop(), Some(expected));
                }
                assert_eq!(queue.pop(), None, "the refused item was never accepted");
            }

            #[test]
            fn loses_nothing_under_concurrent_producers() {
                let queue = Arc::new($module::Queue::with_capacity(64));
                let mut handles = Vec::new();
                for producer in 0..PRODUCERS {
                    let queue = Arc::clone(&queue);
                    handles.push(thread::spawn(move || {
                        for index in 0..PER_PRODUCER {
                            let value = producer * PER_PRODUCER + index;
                            while !queue.push(value) {
                                std::thread::yield_now();
                            }
                        }
                    }));
                }

                let expected = (PRODUCERS * PER_PRODUCER) as usize;
                let mut seen = HashSet::with_capacity(expected);
                while seen.len() < expected {
                    if let Some(value) = queue.pop() {
                        assert!(seen.insert(value), "item {value} was delivered twice");
                    }
                }

                for handle in handles {
                    handle.join().expect("no producer panicked");
                }
                assert_eq!(seen.len(), expected, "every pushed item was delivered");
                assert_eq!(queue.pop(), None, "nothing extra was delivered");
            }
        }
    };
}

layout_suite!(narrow, narrow_layout);
layout_suite!(deep, deep_layout);
layout_suite!(wide, wide_layout);
