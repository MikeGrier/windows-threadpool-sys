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
    let mut teardown = Teardown::new(None);

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
    let mut teardown = Teardown::new(Some(Disposal::new(move |item: DropCounter| {
        seen.fetch_add(1, Ordering::Relaxed);
        // Deliberately kept alive past the sink call, which is the whole point:
        // the owner decides when -- and on which thread -- the destructor runs.
        std::mem::forget(item);
    })));

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
    let mut teardown = Teardown::new(Some(Disposal::new(move |item: u32| {
        seen.lock().expect("no test holds this poisoned").push(item);
    })));

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

    let mut teardown = Teardown::new(Some(Disposal::new(move |item: u32| {
        seen.fetch_add(1, Ordering::Relaxed);
        assert_ne!(item, 3, "deliberate panic from a caller-supplied sink");
    })));

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
    let mut teardown = Teardown::new(Some(Disposal::new(|_item: DropCounter| {
        panic!("deliberate panic from a caller-supplied sink");
    })));

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

    let mut teardown = Teardown::new(Some(Disposal::new(move |item: u32| {
        total += item;
        report.store(total as usize, Ordering::Relaxed);
    })));

    for value in 1..=4 {
        teardown.dispose(value);
    }
    assert_eq!(sum.load(Ordering::Relaxed), 10);
}

#[test]
fn the_debug_form_says_which_policy_is_in_force() {
    // Teardown is invisible until something goes wrong, so the one place it can
    // be observed should say which of the two it is.
    let plain: Teardown<u32> = Teardown::new(None);
    assert!(format!("{plain:?}").contains("hands_off: false"));

    let handing: Teardown<u32> = Teardown::new(Some(Disposal::new(|_| {})));
    assert!(format!("{handing:?}").contains("hands_off: true"));
}

/// An item whose own destructor panics on one chosen value.
///
/// `T` is the caller's type, so its `Drop` is caller-supplied code exactly as a
/// sink is -- which is the whole point of the test below.
struct PanicsOnDrop {
    value: u32,
    dropped: Arc<AtomicUsize>,
}

impl Drop for PanicsOnDrop {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        assert_ne!(
            self.value, 3,
            "deliberate panic from a caller-supplied destructor"
        );
    }
}

#[test]
fn a_panicking_item_destructor_does_not_strand_the_items_behind_it() {
    // **The default policy had the defect the sink policy was written to
    // avoid.** With no sink the item was dropped directly, so a panicking
    // `T::drop` escaped this manual walk over the surviving slots -- abandoning
    // every item behind it, and inside an unwind aborting the process on the
    // second panic. The reasoning for catching the sink never distinguished the
    // two, and neither does the code now.
    let dropped = Arc::new(AtomicUsize::new(0));

    let mut teardown = Teardown::new(None);
    for value in 0..10 {
        teardown.dispose(PanicsOnDrop {
            value,
            dropped: Arc::clone(&dropped),
        });
    }

    assert_eq!(
        dropped.load(Ordering::Relaxed),
        10,
        "the walk must continue past a panicking destructor, or one bad item loses the rest"
    );
}

#[test]
fn a_panicking_destructor_still_destroys_the_item_it_panicked_on() {
    // Catching the panic must not turn into retaining the item: it was moved
    // into the closure, so the unwind destroys it. Asserted separately so that
    // "the panic is caught" cannot be mistaken for "the item survives".
    let dropped = Arc::new(AtomicUsize::new(0));

    let mut teardown = Teardown::new(None);
    teardown.dispose(PanicsOnDrop {
        value: 3,
        dropped: Arc::clone(&dropped),
    });

    assert_eq!(dropped.load(Ordering::Relaxed), 1);
}

#[test]
fn the_debug_rendering_names_the_type() {
    // `Disposal` holds a boxed closure, so its rendering is deliberately opaque
    // -- but opaque is not the same as empty. A `Debug` returning `Ok(default)`
    // writes nothing at all and passes any test that only checks it does not
    // panic, which is what a mutation run found here.
    let rendered = format!("{:?}", Disposal::new(|_: u32| {}));
    assert!(rendered.contains("Disposal"), "got {rendered}");
}
