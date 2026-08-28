// Copyright (c) Mike Grier.

//! Tests for the declared aspects.
//!
//! WOW64 redirection cannot be exercised positively in a 64-bit process -- there
//! is no redirector to disable -- so its tests assert the *reported failure*
//! rather than pretending to measure a disable that did not happen. That is the
//! honest shape: a caller who asked for redirection to be disabled and silently
//! did not get it would be reading a different filesystem than it believes.

use super::{BackgroundMode, Declared, DeclaredAspect, MemoryPriority, Wow64Redirection};

/// Is this a 64-bit process, where redirection does not exist?
const SIXTY_FOUR_BIT: bool = cfg!(target_pointer_width = "64");

#[test]
fn none_declares_nothing() {
    let declared = Declared::none();
    assert!(declared.is_empty());
    assert_eq!(declared, Declared::default());
}

#[test]
fn builders_accumulate_independently() {
    let declared = Declared::none()
        .with_memory_priority(MemoryPriority::Low)
        .with_background_mode(BackgroundMode::Begin);
    assert!(!declared.is_empty());
    assert_eq!(declared.memory_priority, Some(MemoryPriority::Low));
    assert_eq!(declared.background_mode, Some(BackgroundMode::Begin));
    assert_eq!(
        declared.wow64_redirection, None,
        "an unset aspect stays unset"
    );
}

#[test]
fn every_memory_priority_round_trips_through_its_raw_value() {
    for priority in [
        MemoryPriority::VeryLow,
        MemoryPriority::Low,
        MemoryPriority::Medium,
        MemoryPriority::BelowNormal,
        MemoryPriority::Normal,
    ] {
        assert_eq!(MemoryPriority::from_raw(priority.as_raw()), Some(priority));
    }
}

#[test]
fn an_unknown_memory_priority_value_is_not_invented() {
    assert_eq!(MemoryPriority::from_raw(0), None);
    assert_eq!(MemoryPriority::from_raw(99), None);
}

#[test]
fn a_thread_reports_a_readable_memory_priority() {
    // The fact that makes this aspect's declared status a choice rather than a
    // limitation: unlike redirection, it can be read.
    MemoryPriority::current().expect("memory priority is readable");
}

#[test]
fn declaring_nothing_runs_the_operation_and_touches_nothing() {
    let before = MemoryPriority::current().expect("readable");
    let value = Declared::none().with_applied(|| 7).expect("nothing to do");
    assert_eq!(value, 7);
    assert_eq!(MemoryPriority::current().expect("readable"), before);
}

#[test]
fn a_declared_memory_priority_is_installed_and_restored() {
    let before = MemoryPriority::current().expect("readable");
    let declared = Declared::none().with_memory_priority(MemoryPriority::Low);
    let during = declared
        .with_applied(|| MemoryPriority::current().expect("readable"))
        .expect("apply");
    assert_eq!(
        during,
        MemoryPriority::Low,
        "the priority was not installed"
    );
    assert_eq!(
        MemoryPriority::current().expect("readable"),
        before,
        "the entry priority was not restored"
    );
}

#[test]
fn declaring_the_priority_a_thread_already_has_is_not_special() {
    let before = MemoryPriority::current().expect("readable");
    let declared = Declared::none().with_memory_priority(before);
    let during = declared
        .with_applied(|| MemoryPriority::current().expect("readable"))
        .expect("apply");
    assert_eq!(during, before);
    assert_eq!(MemoryPriority::current().expect("readable"), before);
}

#[test]
fn background_mode_is_entered_and_left() {
    // Background mode lowers memory priority too, which is the coupling the type
    // documents; observing it here is what proves the coupling is real rather
    // than folklore.
    let before = MemoryPriority::current().expect("readable");
    let declared = Declared::none().with_background_mode(BackgroundMode::Begin);
    let during = declared
        .with_applied(|| MemoryPriority::current().expect("readable"))
        .expect("apply");
    assert_ne!(
        during, before,
        "entering background mode did not move memory priority, so the \
         documented coupling of CPU, I/O and memory priority did not hold"
    );
    assert_eq!(
        MemoryPriority::current().expect("readable"),
        before,
        "leaving background mode did not restore memory priority"
    );
}

#[test]
fn the_operations_return_value_is_passed_through() {
    let value = Declared::none()
        .with_memory_priority(MemoryPriority::BelowNormal)
        .with_applied(|| String::from("carried"))
        .expect("apply");
    assert_eq!(value, "carried");
}

#[test]
fn wow64_redirection_is_refused_in_a_sixty_four_bit_process() {
    if !SIXTY_FOUR_BIT {
        eprintln!("skipped: this assertion is about 64-bit processes");
        return;
    }
    // Reported, not swallowed. A caller that asked for redirection to be
    // disabled and silently did not get it would be reading a different
    // filesystem than it believes.
    let error = Declared::none()
        .with_wow64_redirection(Wow64Redirection::Disabled)
        .with_applied(|| ())
        .expect_err("there is no redirector to disable in a 64-bit process");
    assert_eq!(error.aspect(), DeclaredAspect::Wow64Redirection);
}

#[test]
fn a_failing_aspect_releases_the_ones_already_installed() {
    // The unwind-order property that matters: redirection is installed last, so
    // its failure must still release the memory priority applied before it.
    if !SIXTY_FOUR_BIT {
        eprintln!("skipped: relies on redirection failing");
        return;
    }
    let before = MemoryPriority::current().expect("readable");
    let declared = Declared::none()
        .with_memory_priority(MemoryPriority::Low)
        .with_wow64_redirection(Wow64Redirection::Disabled);
    let error = declared
        .with_applied(|| ())
        .expect_err("redirection fails in a 64-bit process");
    assert_eq!(error.aspect(), DeclaredAspect::Wow64Redirection);
    assert_eq!(
        MemoryPriority::current().expect("readable"),
        before,
        "a failed later aspect left an earlier one installed"
    );
}

#[test]
fn the_operation_does_not_run_when_an_aspect_cannot_be_installed() {
    if !SIXTY_FOUR_BIT {
        eprintln!("skipped: relies on redirection failing");
        return;
    }
    let mut ran = false;
    let _ = Declared::none()
        .with_wow64_redirection(Wow64Redirection::Disabled)
        .with_applied(|| ran = true);
    assert!(
        !ran,
        "the operation ran despite an aspect failing to install"
    );
}

#[test]
fn declared_values_survive_being_moved_to_another_thread() {
    let declared = Declared::none().with_memory_priority(MemoryPriority::Medium);
    let observed = std::thread::spawn(move || {
        let before = MemoryPriority::current().expect("readable");
        let during = declared
            .with_applied(|| MemoryPriority::current().expect("readable"))
            .expect("apply on the worker");
        (before, during, MemoryPriority::current().expect("readable"))
    })
    .join()
    .expect("the worker did not panic");

    assert_eq!(
        observed.1,
        MemoryPriority::Medium,
        "the value did not arrive"
    );
    assert_eq!(observed.0, observed.2, "the worker was left contaminated");
}

#[test]
fn nesting_restores_through_each_layer() {
    let entry = MemoryPriority::current().expect("readable");
    let outer = Declared::none().with_memory_priority(MemoryPriority::Low);
    let inner = Declared::none().with_memory_priority(MemoryPriority::VeryLow);

    let deepest = outer
        .with_applied(|| {
            assert_eq!(
                MemoryPriority::current().expect("readable"),
                MemoryPriority::Low
            );
            let deepest = inner
                .with_applied(|| MemoryPriority::current().expect("readable"))
                .expect("inner apply");
            assert_eq!(
                MemoryPriority::current().expect("readable"),
                MemoryPriority::Low,
                "the inner release skipped the outer state"
            );
            deepest
        })
        .expect("outer apply");

    assert_eq!(deepest, MemoryPriority::VeryLow);
    assert_eq!(MemoryPriority::current().expect("readable"), entry);
}

#[test]
fn declared_is_copy_and_send_so_it_can_reach_a_worker() {
    fn assert_send_copy<T: Send + Copy>() {}
    assert_send_copy::<Declared>();
    assert_send_copy::<MemoryPriority>();
    assert_send_copy::<BackgroundMode>();
    assert_send_copy::<Wow64Redirection>();
}
