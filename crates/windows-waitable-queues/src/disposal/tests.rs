// Copyright (c) Mike Grier.

//! Tests for the teardown policy in isolation, with no queue attached.
//!
//! The policy's behaviour *through* a queue is asserted in each shape's own
//! suite, because each walks its own layout to find the survivors and covering
//! one would say nothing about the others. What is tested here is the part
//! they share: that the default destroys, that a sink receives, and that a
//! panicking sink does not strand the items behind it.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::{Disposal, Teardown};

/// Counts its own drops, so a test can tell "handed to the sink" from
/// "destroyed where it lay" -- which is the entire distinction this module
/// exists to draw.
#[derive(Debug)]
struct DropCounter(Arc<AtomicUsize>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn the_default_policy_destroys_the_item() {
    let drops = Arc::new(AtomicUsize::new(0));
    let mut teardown = Teardown::drop_in_place();

    teardown.dispose(DropCounter(Arc::clone(&drops)));
    assert_eq!(
        drops.load(Ordering::Relaxed),
        1,
        "with no sink there is nowhere else for it to go, so it is destroyed here"
    );
}

#[test]
fn a_sink_receives_the_item_instead_of_it_being_destroyed() {
    let drops = Arc::new(AtomicUsize::new(0));
    let collected = Arc::new(AtomicUsize::new(0));

    let seen = Arc::clone(&collected);
    let mut teardown = Teardown::handing_off(Disposal::new(move |item: DropCounter| {
        seen.fetch_add(1, Ordering::Relaxed);
        // Deliberately kept alive past the sink call, which is the whole point:
        // the owner decides when -- and on which thread -- the destructor runs.
        std::mem::forget(item);
    }));

    teardown.dispose(DropCounter(Arc::clone(&drops)));

    assert_eq!(collected.load(Ordering::Relaxed), 1, "the sink saw it");
    assert_eq!(
        drops.load(Ordering::Relaxed),
        0,
        "and teardown did not destroy it behind the owner's back"
    );
}

#[test]
fn every_item_reaches_the_sink_in_order() {
    let order = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&order);
    let mut teardown = Teardown::handing_off(Disposal::new(move |item: u32| {
        seen.lock().expect("no test holds this poisoned").push(item);
    }));

    for value in 0..10 {
        teardown.dispose(value);
    }

    assert_eq!(
        *order.lock().expect("no test holds this poisoned"),
        (0..10).collect::<Vec<_>>(),
        "a sink is handed the survivors one at a time, in the order teardown walks them"
    );
}

#[test]
fn a_panicking_sink_does_not_strand_the_items_behind_it() {
    // The property that matters, and the reason the call is wrapped. A sink
    // that panics on one item is a caller bug; losing every *later* item to it
    // would turn that bug into the exact leak this mechanism exists to
    // prevent, and inside an unwind it would abort the process outright.
    let disposed = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&disposed);

    let mut teardown = Teardown::handing_off(Disposal::new(move |item: u32| {
        seen.fetch_add(1, Ordering::Relaxed);
        assert_ne!(item, 3, "deliberate panic from a caller-supplied sink");
    }));

    for value in 0..10 {
        teardown.dispose(value);
    }

    assert_eq!(
        disposed.load(Ordering::Relaxed),
        10,
        "the walk must continue past a panicking sink, or one bad item loses the rest"
    );
}

#[test]
fn a_panicking_sink_still_consumes_the_item_it_panicked_on() {
    // The item was moved into the sink before it panicked, so it is destroyed
    // by the unwind rather than leaked. Asserted so that "the panic is caught"
    // is not mistaken for "the item is still somewhere".
    let drops = Arc::new(AtomicUsize::new(0));
    let mut teardown = Teardown::handing_off(Disposal::new(|_item: DropCounter| {
        panic!("deliberate panic from a caller-supplied sink");
    }));

    teardown.dispose(DropCounter(Arc::clone(&drops)));
    assert_eq!(
        drops.load(Ordering::Relaxed),
        1,
        "the item was already the sink's, so unwinding destroys it"
    );
}

#[test]
fn a_sink_may_be_stateful_across_items() {
    // `FnMut` rather than `Fn`, because the useful sinks accumulate: pushing
    // into a channel, counting, or batching for a reaper.
    let mut total = 0_u32;
    let sum = Arc::new(AtomicUsize::new(0));
    let report = Arc::clone(&sum);

    let mut teardown = Teardown::handing_off(Disposal::new(move |item: u32| {
        total += item;
        report.store(total as usize, Ordering::Relaxed);
    }));

    for value in 1..=4 {
        teardown.dispose(value);
    }
    assert_eq!(sum.load(Ordering::Relaxed), 10);
}

#[test]
fn the_debug_form_says_which_policy_is_in_force() {
    // Teardown is invisible until something goes wrong, so the one place it can
    // be observed should say which of the two it is.
    let plain: Teardown<u32> = Teardown::drop_in_place();
    assert!(format!("{plain:?}").contains("hands_off: false"));

    let handing: Teardown<u32> = Teardown::handing_off(Disposal::new(|_| {}));
    assert!(format!("{handing:?}").contains("hands_off: true"));
}
