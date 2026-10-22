// Copyright (c) Mike Grier.
// Split from queue_contention.rs at bf9786ef.

//! The measurement: `measure` and the timer for each shape and regime it runs.

use super::*;

#[cfg(test)]
mod tests;

/// Time every configuration, sampled as `sampling` says.
///
/// [`Sampling::default`] takes the published counts, interleaved; with
/// [`Order::Sequential`] it is the procedure the figures published before `M4.4`
/// came from.
#[must_use]
pub fn measure(sampling: &Sampling) -> Observation {
    let mut isolated = Vec::new();
    let mut drained = Vec::new();

    for &producers in &sampling.producer_counts {
        let (regimes, mut timers): (Vec<Regime>, Vec<(&'static str, Timer<'static>)>) =
            configurations()
                .into_iter()
                .map(|(regime, shape, timer)| (regime, (shape, timer)))
                .unzip();
        for (regime, run) in regimes
            .into_iter()
            .zip(runs(producers, sampling, &mut timers))
        {
            match regime {
                Regime::Isolated => isolated.push(run),
                Regime::Drained => drained.push(run),
            }
        }
    }

    Observation {
        isolated,
        drained,
        available_parallelism: thread::available_parallelism()
            .ok()
            .map(std::num::NonZeroUsize::get),
        sampling: sampling.clone(),
    }
}

/// Which table a configuration's row belongs in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Regime {
    Isolated,
    Drained,
}

/// Boxes a timer, so the configurations can be listed together.
fn timer(timer: fn(usize, usize) -> Repetition) -> Timer<'static> {
    Box::new(timer)
}

/// Every configuration timed at one producer count.
///
/// **In the order the measurement took before `M4.4`, and that order is load
/// bearing**: [`Order::Sequential`] walks this list, so it is what lets the
/// published figures be retaken. The two regimes are interleaved in it -- the
/// comparison shapes in both, then the layouts in both -- because that is how
/// they were measured, and in each regime the same-code control's two halves,
/// `reserving_mpsc` and `reserving(32/32)`, sit several entries apart.
fn configurations() -> Vec<(Regime, &'static str, Timer<'static>)> {
    let mut list = vec![
        (
            Regime::Isolated,
            shapes::BASELINE_FETCH_ADD,
            timer(time_contended_atomic),
        ),
        (
            Regime::Isolated,
            shapes::SLOTWISE_MPSC,
            timer(time_isolated_mpsc),
        ),
        (
            Regime::Isolated,
            shapes::RESERVING_MPSC,
            timer(time_isolated_reserving),
        ),
        (
            Regime::Isolated,
            shapes::PERMIT_MPSC,
            timer(time_isolated_permit),
        ),
        (
            Regime::Drained,
            shapes::SLOTWISE_MPSC,
            timer(time_drained_mpsc),
        ),
        (
            Regime::Drained,
            shapes::RESERVING_MPSC,
            timer(time_drained_reserving),
        ),
        (
            Regime::Drained,
            shapes::PERMIT_MPSC,
            timer(time_drained_permit),
        ),
        (
            Regime::Isolated,
            shapes::CLAIM_NARROW,
            timer(time_isolated_layout::<Balanced>),
        ),
        (
            Regime::Isolated,
            shapes::CLAIM_DEEP,
            timer(time_isolated_layout::<Enduring>),
        ),
        (
            Regime::Isolated,
            shapes::CLAIM_PERPETUAL,
            timer(time_isolated_layout::<Perpetual>),
        ),
    ];
    // Gated on the architectures where a 128-bit exchange is native; see the
    // `Wide` import in the facade. `#[cfg]` governs only the statement that
    // follows it, so each of the two pushes carries its own.
    #[cfg(any(
        all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
        target_arch = "aarch64"
    ))]
    list.push((
        Regime::Isolated,
        shapes::CLAIM_WIDE,
        timer(time_isolated_layout::<Wide>),
    ));
    list.push((
        Regime::Drained,
        shapes::CLAIM_NARROW,
        timer(time_drained_layout::<Balanced>),
    ));
    list.push((
        Regime::Drained,
        shapes::CLAIM_DEEP,
        timer(time_drained_layout::<Enduring>),
    ));
    list.push((
        Regime::Drained,
        shapes::CLAIM_PERPETUAL,
        timer(time_drained_layout::<Perpetual>),
    ));
    #[cfg(any(
        all(target_arch = "x86_64", target_feature = "cmpxchg16b"),
        target_arch = "aarch64"
    ))]
    list.push((
        Regime::Drained,
        shapes::CLAIM_WIDE,
        timer(time_drained_layout::<Wide>),
    ));
    list
}

/// The floor: N threads incrementing one shared counter.
///
/// Not a queue, and not trying to be. It is the cheapest possible operation on
/// a contended line, so it says how much of a queue's scaling curve is the
/// queue and how much is simply what this processor does when N cores fight
/// over one cache line.
fn time_contended_atomic(producers: usize, pushes: usize) -> Repetition {
    let counter = Arc::new(AtomicU64::new(0));
    // One party per worker plus this thread. Every worker is created, then waits
    // here; the clock starts as the barrier releases, so neither thread creation
    // nor a solo head start by an early worker is inside the measurement. See
    // `start_barrier`'s note for why that matters at these producer counts.
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for _ in 0..producers {
            let counter = Arc::clone(&counter);
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for _ in 0..pushes {
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    (measured_span(&spans), 0)
}

fn time_isolated_mpsc(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) =
        slotwise_mpsc::bounded::<u64>(capacity_for(producers, pushes)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..pushes {
                    tx.push((producer * pushes + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    // Drain before dropping: teardown would otherwise walk every slot, and that
    // is not part of what is being timed.
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

fn time_isolated_reserving(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded::<u64>(capacity_for(producers, pushes)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..pushes {
                    tx.push((producer * pushes + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// The experimental permit claim, with no consumer and no possibility of refusal.
///
/// Not "the regime that isolates the claim", which an earlier wording said: this
/// times the whole push path, including slot metadata, the item write,
/// publication and the doorbell's fence. See the module header.
///
/// A line-for-line twin of [`time_isolated_reserving`] with one shape
/// substituted. Deliberately not factored into a generic over the two, which
/// would need a trait both implement and would put a dynamic or monomorphised
/// indirection inside the timed region -- in a measurement whose whole output is
/// a difference of a few nanoseconds per push.
fn time_isolated_permit(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) =
        permit_mpsc::bounded::<u64>(capacity_for(producers, pushes)).expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..pushes {
                    tx.push((producer * pushes + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

fn time_drained_mpsc(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) = slotwise_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer is a gate participant, not merely spawned: spawning is not
    // readiness, and a consumer still in thread start-up while producers push
    // turns the opening of the run into an undrained regime.
    //
    // The gate alone does not finish the job, which is why `await_consumer`
    // exists. Arriving proves the consumer exists, is scheduled and is past
    // start-up; it does not prove the consumer has reached its first `pop`, and
    // the gate releases every party together. The handshake narrows that
    // remainder to a stated guarantee: no producer begins timing until the
    // consumer has executed its pop path at least once. Continuous draining is
    // not guaranteed and cannot be by a flag. This is the M4.3 change, and it
    // MOVED the drained numbers -- the figures taken before it are kept beside
    // the ones taken after rather than replaced, because they measure two
    // different pieces of code.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        // Spin rather than park: the doorbell's cost is `doorbell_cost`'s
        // question, and parking here would measure that instead of the claim.
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..pushes {
                    let mut item = (producer * pushes + index) as u64;
                    // Retry a FULL queue, which is what a real producer does;
                    // the refusal count is what makes that visible. Anything
                    // else is not retryable -- a disconnected queue never
                    // drains -- and retrying it is an infinite spin that
                    // presents as a hung probe rather than as the consumer
                    // failure it actually is. The queue crate says so itself:
                    // "retrying the first is sensible and retrying the second
                    // is a spin".
                    while let Err(error) = tx.push(item) {
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

fn time_drained_reserving(producers: usize, pushes: usize) -> Repetition {
    // **Defaults on both sides, and that is a correction.** This row previously
    // enabled high-water tracking here and nowhere else, to "also price the
    // switch M31.4 made opt-in". But the number it feeds is presented as the
    // cost of *reservation*, and tracking adds work to this shape's push path
    // alone, so the ratio measured reservation plus a handicap with no way for a
    // reader to separate them.
    //
    // **What the handicap actually is, corrected:** an earlier version of this
    // comment said tracking adds a load of the consumer's position. It does not.
    // `reserving_mpsc::publish` loads `head` **unconditionally** -- the slot
    // write needs that acquire edge whether or not anything is measured, as the
    // comment at that load says in as many words. What the switch adds is the
    // depth arithmetic and the metric update on the far side of a branch that is
    // taken either way. Smaller than claimed, and still not part of what this row
    // is presented as measuring.
    //
    // Nothing consumes the high-water figure here either, so the tracking was
    // paying a cost to produce a number nobody read. Pricing that switch is a
    // worthwhile measurement and needs its own row, with both shapes tracking,
    // rather than being folded into this comparison.
    let (tx, rx) = reserving_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer joins the gate here for the reason it does in the slotwise
    // twin: a run whose opening is undrained is not the regime being measured.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..pushes {
                    let mut item = (producer * pushes + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// The experimental permit claim, against a consumer looping on `pop`.
///
/// The regime that can price the claim honestly, for the same reason the
/// reserving twin needs it: the shared line a producer touches is only
/// expensive when a consumer is writing it. Measured in isolation, an
/// uncontended line looks free -- which would be a confident wrong answer, and
/// this shape has more riding on that answer than the others, because it trades
/// `reserving_mpsc`'s *load* of the consumer's position for a read-modify-write
/// on a count the consumer also writes.
fn time_drained_permit(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) = permit_mpsc::bounded::<u64>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..pushes {
                    let mut item = (producer * pushes + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);

    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}

/// One claim-word layout, with no consumer and no possibility of refusal.
///
/// As with the other isolated timers, this is the whole push path and not the
/// claim word alone; only the layout differs between these rows, so a difference
/// is still attributable to the layout, but its magnitude is a share of total
/// push cost rather than of the exchange.
///
/// **Generic over the layout, where [`time_isolated_permit`] is deliberately
/// duplicated, and the difference is the point.** That twin compares two
/// *different types*, which a generic could only unify behind a trait, putting
/// an indirection that might not inline identically inside the timed region.
/// These are the *same type* at different layout parameters, so this
/// monomorphises to exactly the code a hand-written copy would produce -- there
/// is nothing left to dispatch.
///
/// Measures `reserving_mpsc` itself rather than a stand-in. An earlier form of
/// this probe carried its own duplicated implementation of the claim protocol,
/// built so the layouts could be compared before the shipping crate had them;
/// it drifted from the original twice while doing so. The shipping type takes
/// the layout as a parameter now, so the duplicate is gone.
fn time_isolated_layout<L: ClaimLayout>(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) = reserving_mpsc::bounded_as::<u64, L>(capacity_for(producers, pushes))
        .expect("a valid capacity");
    let gate = start_gate(producers);
    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                let began = Instant::now();
                for index in 0..pushes {
                    tx.push((producer * pushes + index) as u64)
                        .expect("the run fits in the capacity");
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    let refusals = tx.refused();
    while rx.pop().is_ok() {}
    (elapsed, refusals)
}

/// One claim-word layout, against a consumer looping on `pop`.
///
/// Generic for [`time_isolated_layout`]'s reason.
fn time_drained_layout<L: ClaimLayout + 'static>(producers: usize, pushes: usize) -> Repetition {
    let (tx, rx) =
        reserving_mpsc::bounded_as::<u64, L>(DRAINED_CAPACITY).expect("a valid capacity");
    let done = Arc::new(AtomicBool::new(false));
    let consumer_done = Arc::clone(&done);
    // Set on every exit path, not just the one that returns. See StopOnDrop.
    let stop = StopOnDrop(done);
    // Closes the undrained opening the gate alone leaves. See await_consumer.
    let ready = Arc::new(AtomicBool::new(false));
    let consumer_ready = Arc::clone(&ready);
    // The consumer joins the gate for the reason its twins do: a run whose
    // opening is undrained is not the regime being measured.
    let gate = start_gate(producers + 1);
    let consumer_gate = Arc::clone(&gate);

    let consumer = thread::spawn(move || {
        if !consumer_gate.arrive_and_wait() {
            return rx.refused();
        }
        // **One drain attempt BEFORE announcing, not merely reaching the loop.**
        // Publishing first proves only that the consumer is about to drain: it
        // can be descheduled between the store and its first `pop`, which is the
        // same undrained opening in a narrower form. Popping first makes the
        // announcement mean `this consumer has executed the pop path`, which is a
        // fact rather than an intention. The queue is empty here, so it costs one
        // failed pop, and it happens before any producer has started timing.
        drain_then_announce(
            || {
                let _ = rx.pop();
            },
            &consumer_ready,
        );
        while !consumer_done.load(Ordering::Relaxed) {
            while rx.pop().is_ok() {}
            std::hint::spin_loop();
        }
        while rx.pop().is_ok() {}
        rx.refused()
    });

    let spans = thread::scope(|scope| {
        let _release = ReleaseOnDrop(Arc::clone(&gate));
        let mut workers = Vec::with_capacity(producers);
        for producer in 0..producers {
            let tx = tx.clone();
            let gate = Arc::clone(&gate);
            let ready = Arc::clone(&ready);
            workers.push(scope.spawn(move || {
                if !gate.arrive_and_wait() {
                    // Abandoned before the party completed; see `StartGate`.
                    return (Instant::now(), Instant::now());
                }
                await_consumer(&ready);
                let began = Instant::now();
                for index in 0..pushes {
                    let mut item = (producer * pushes + index) as u64;
                    while let Err(error) = tx.push(item) {
                        // Only a FULL queue is retryable; see the note on the
                        // first of these loops.
                        assert!(
                            error.is_retryable(),
                            "the consumer is gone, so this push can never \
                             succeed: {error}"
                        );
                        item = error.into_inner();
                        std::hint::spin_loop();
                    }
                }
                (began, Instant::now())
            }));
        }
        let _ = gate.arrive_and_wait();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("a producer must not panic"))
            .collect::<Vec<_>>()
    });
    let elapsed = measured_span(&spans);
    drop(stop);
    drop(tx);
    let refusals = consumer.join().expect("the consumer must not panic");
    (elapsed, refusals)
}
