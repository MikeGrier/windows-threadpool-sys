// Copyright (c) 2026 Mike Grier
//! Thread-pool waits preserving the sealed event boundary.

use std::os::windows::io::{AsHandle, AsRawHandle};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::{ACTIVATION_TIMEOUT, Activations, take};
use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
use crate::wait::{SealedThreadpoolWait, SealedWaitActivation, SealedWaitableHandle, WaitResult};
use win_sync_sys::{Event, ResetMode};

fn recording_wait(
    mode: ResetMode,
    signalled: bool,
) -> (Event, SealedThreadpoolWait, Arc<Activations>) {
    let owner = Event::new(mode, signalled).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| recorder.record(activation.result()),
        None,
    )
    .expect("wait");
    (owner, wait, seen)
}

#[test]
fn the_owner_signal_runs_a_sealed_callback() {
    let owner = Event::new(ResetMode::Manual, false).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| {
            recorder.record(activation.result());
        },
        None,
    )
    .expect("wait");
    wait.arm(None);
    owner.set().expect("set");
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn target_and_wait_share_the_original_sealed_handle_until_drop() {
    let owner = Event::new(ResetMode::Manual, true).expect("event");
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let raw = unsafe { sealed.as_waitable_handle() }.as_raw_handle();
    let observer = unsafe { sealed.clone().into_shared_handle() };
    let target = SealedWaitableHandle::from(sealed);
    assert_eq!(format!("{target:?}"), "SealedWaitableHandle { .. }");
    let observer = observer
        .try_into_owned()
        .expect_err("target shares ownership");
    let wait = SealedThreadpoolWait::new(target, |_| {}, None).expect("wait");
    assert_eq!(format!("{wait:?}"), "SealedThreadpoolWait { .. }");
    let observer = observer
        .try_into_owned()
        .expect_err("wait shares ownership");
    drop(owner);
    assert!(take(observer.as_handle()));
    drop(wait);
    let owned = observer
        .try_into_owned()
        .expect("last holder after the wait drops");
    assert_eq!(owned.as_raw_handle(), raw);
}

#[test]
fn an_unused_target_releases_its_shared_owner() {
    let owner = Event::new(ResetMode::Manual, false).expect("event");
    let sealed = owner.duplicate_sealed_wait_only().expect("sealed event");
    let observer = unsafe { sealed.clone().into_shared_handle() };
    let target = SealedWaitableHandle::from(sealed);
    assert_eq!(format!("{target:#?}"), "SealedWaitableHandle { .. }");
    drop(target);
    assert!(observer.try_into_owned().is_ok());
}

#[test]
fn the_wait_is_send_and_sync_and_the_activation_is_borrowed() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SealedWaitableHandle>();
    assert_send_sync::<SealedThreadpoolWait>();
    assert_send_sync::<SealedWaitActivation<'_>>();
}

#[test]
fn the_wait_keeps_the_event_alive_after_every_other_holder_drops() {
    let (owner, wait, seen) = recording_wait(ResetMode::Manual, true);
    drop(owner);
    wait.arm(None);
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn an_auto_reset_sealed_wait_consumes_one_signal() {
    let (owner, wait, seen) = recording_wait(ResetMode::Auto, false);
    wait.arm(None);
    owner.set().expect("set");
    seen.wait_for(1);
    wait.wait();
    assert!(!take(owner.as_handle()));
    wait.arm(Some(Duration::from_millis(10)));
    let results = seen.wait_for(2);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled, WaitResult::TimedOut]);
}

#[test]
fn a_manual_reset_owner_resets_before_the_callback_rearms() {
    let owner = Event::new(ResetMode::Manual, false).expect("event");
    let resetter = owner.clone();
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| {
            resetter.reset().expect("owner reset");
            activation.rearm(None);
            recorder.record(activation.result());
        },
        None,
    )
    .expect("wait");
    wait.arm(None);
    owner.set().expect("first signal");
    seen.wait_for(1);
    assert!(!take(owner.as_handle()));
    owner.set().expect("second signal after rearm");
    let results = seen.wait_for(2);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled; 2]);
}

#[test]
fn rearming_a_signalled_manual_event_runs_again_without_another_set() {
    let owner = Event::new(ResetMode::Manual, true).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let calls = AtomicUsize::new(0);
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                activation.rearm(None);
            }
            recorder.record(activation.result());
        },
        None,
    )
    .expect("wait");
    wait.arm(None);
    let results = seen.wait_for(2);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled; 2]);
    assert!(take(owner.as_handle()));
}

#[test]
fn activation_reports_timeouts_and_signals_without_a_handle_in_debug() {
    let owner = Event::new(ResetMode::Manual, false).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let (sender, receiver) = mpsc::channel();
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| {
            let _ = sender.send((
                activation.result(),
                activation.is_signalled(),
                format!("{activation:?}"),
            ));
        },
        None,
    )
    .expect("wait");
    wait.arm(Some(Duration::from_millis(10)));
    let timed_out = receiver
        .recv_timeout(ACTIVATION_TIMEOUT)
        .expect("timeout callback");
    wait.arm(None);
    owner.set().expect("set");
    let signalled = receiver
        .recv_timeout(ACTIVATION_TIMEOUT)
        .expect("signal callback");
    wait.stop_and_drain();
    assert_eq!(
        timed_out,
        (
            WaitResult::TimedOut,
            false,
            "SealedWaitActivation { result: TimedOut, .. }".to_owned()
        )
    );
    assert_eq!(
        signalled,
        (
            WaitResult::Signalled,
            true,
            "SealedWaitActivation { result: Signalled, .. }".to_owned()
        )
    );
}

#[test]
fn a_callback_can_rearm_with_a_timeout() {
    let owner = Event::new(ResetMode::Auto, false).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| {
            // Recorded first: the rearm permits an overlapping timeout callback.
            recorder.record(activation.result());
            if activation.is_signalled() {
                activation.rearm(Some(Duration::from_millis(10)));
            }
        },
        None,
    )
    .expect("wait");
    wait.arm(None);
    owner.set().expect("set");
    let results = seen.wait_for(2);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled, WaitResult::TimedOut]);
}

#[test]
fn disarm_prevents_a_signal_until_explicitly_armed_again() {
    let (owner, wait, seen) = recording_wait(ResetMode::Manual, false);
    wait.arm(None);
    wait.disarm();
    owner.set().expect("set while disarmed");
    wait.wait();
    assert_eq!(seen.count(), 0);
    wait.arm(None);
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[test]
fn a_drained_wait_is_reusable() {
    let (owner, wait, seen) = recording_wait(ResetMode::Manual, false);
    wait.stop_and_drain();
    for wanted in 1..=2 {
        wait.arm(None);
        owner.set().expect("set");
        seen.wait_for(wanted);
        wait.stop_and_drain();
        owner.reset().expect("reset");
    }
    assert_eq!(seen.results(), vec![WaitResult::Signalled; 2]);
}

#[test]
fn wait_and_drop_finish_an_executing_callback() {
    for use_wait in [false, true] {
        let owner = Event::new(ResetMode::Manual, false).expect("event");
        let target = owner
            .duplicate_sealed_wait_only()
            .expect("sealed event")
            .into();
        let seen = Activations::new();
        let recorder = Arc::clone(&seen);
        let done = Arc::new(AtomicUsize::new(0));
        let completed = Arc::clone(&done);
        let wait = SealedThreadpoolWait::new(
            target,
            move |activation| {
                recorder.record(activation.result());
                std::thread::sleep(Duration::from_millis(20));
                completed.store(1, Ordering::SeqCst);
            },
            None,
        )
        .expect("wait");
        wait.arm(None);
        owner.set().expect("set");
        seen.wait_for(1);
        if use_wait {
            wait.wait();
            assert_eq!(done.load(Ordering::SeqCst), 1);
            wait.stop_and_drain();
        }
        drop(wait);
        assert_eq!(done.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn dropping_an_idle_wait_releases_its_callback_capture() {
    let owner = Event::new(ResetMode::Manual, false).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let captured = Arc::new(());
    let weak = Arc::downgrade(&captured);
    let wait = SealedThreadpoolWait::new(
        target,
        move |_| {
            std::hint::black_box(&captured);
        },
        None,
    )
    .expect("wait");
    assert!(weak.upgrade().is_some());
    drop(wait);
    assert!(weak.upgrade().is_none());
}

#[test]
fn a_private_pool_remains_usable_after_untracked_cancellation_and_repair() {
    let pool = ThreadpoolPool::new().expect("pool");
    pool.set_min_threads(1).expect("minimum");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    let owner = Event::new(ResetMode::Auto, false).expect("event");
    let target = owner
        .duplicate_sealed_wait_only()
        .expect("sealed event")
        .into();
    let seen = Activations::new();
    let recorder = Arc::clone(&seen);
    let wait = SealedThreadpoolWait::new(
        target,
        move |activation| recorder.record(activation.result()),
        Some(&mut env),
    )
    .expect("wait");
    let repair = crate::work::ThreadpoolWork::new(|| {}, Some(&mut env)).expect("repair work");
    wait.arm(None);
    wait.disarm();
    unsafe { wait.try_cancel_pending_no_heal_tracking() };
    repair.submit();
    repair.stop_and_drain();
    wait.arm(None);
    owner.set().expect("set");
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[cfg(feature = "self-heal")]
#[test]
fn tracked_cancellation_preserves_wait_reuse() {
    let (owner, wait, seen) = recording_wait(ResetMode::Auto, false);
    wait.arm(None);
    wait.disarm();
    wait.try_cancel_pending();
    wait.arm(None);
    owner.set().expect("set");
    let results = seen.wait_for(1);
    wait.stop_and_drain();
    assert_eq!(results, vec![WaitResult::Signalled]);
}

#[cfg(feature = "trace")]
#[test]
fn lifecycle_forwarders_reach_the_native_operations() {
    crate::trace::in_a_trace_armed_child(
        "wait::tests::sealed::lifecycle_forwarders_reach_the_native_operations",
        "wait,syscall",
        || {
            fn assert_call(name: &str, value: &str) {
                assert!(
                    crate::trace::dump().lines().any(|line| {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        fields.len() == 6
                            && fields[2] == "syscall-enter"
                            && fields[3] == name
                            && fields[5] == value
                    }),
                    "missing call {name} with argument {value}"
                );
            }
            let (_owner, wait, _seen) = recording_wait(ResetMode::Manual, false);
            crate::trace::clear();
            wait.wait();
            assert_call("WaitForThreadpoolWaitCallbacks", "0");
            crate::trace::clear();
            wait.disarm();
            assert_call("SetThreadpoolWait(disarm)", "0");
            crate::trace::clear();
            wait.stop_and_drain();
            assert_call("SetThreadpoolWait(disarm)", "0");
            assert_call("WaitForThreadpoolWaitCallbacks", "0");
            #[cfg(feature = "self-heal")]
            {
                crate::trace::clear();
                wait.try_cancel_pending();
                assert_call("WaitForThreadpoolWaitCallbacks(cancel)", "1");
            }
            let repair = crate::work::ThreadpoolWork::new(|| {}, None).expect("repair work");
            crate::trace::clear();
            unsafe { wait.try_cancel_pending_no_heal_tracking() };
            repair.submit();
            repair.stop_and_drain();
            assert_call("WaitForThreadpoolWaitCallbacks(cancel)", "1");
        },
    );
}

#[cfg(feature = "trace")]
#[test]
fn trace_redacts_sealed_targets_but_preserves_ordinary_targets() {
    crate::trace::in_a_trace_armed_child(
        "wait::tests::sealed::trace_redacts_sealed_targets_but_preserves_ordinary_targets",
        "wait,syscall",
        || {
            use std::os::windows::io::AsRawHandle;
            use std::sync::atomic::{AtomicUsize, Ordering};

            fn assert_targets(dump: &str, expected: usize) {
                let expected_records = [
                    ("wait", "created"),
                    ("wait", "armed"),
                    ("wait", "rearm-left"),
                    ("wait", "drop-begin"),
                    ("wait", "drop-closed"),
                    ("syscall-enter", "SetThreadpoolWait"),
                    ("syscall-leave", "SetThreadpoolWait"),
                ];
                for (target, event) in expected_records {
                    let mut found = false;
                    for line in dump.lines() {
                        let fields: Vec<_> = line.split_whitespace().collect();
                        if fields.len() == 6 && fields[2] == target && fields[3] == event {
                            found = true;
                            assert_eq!(
                                fields[5].parse::<usize>().expect("target value"),
                                expected,
                                "{line}"
                            );
                        }
                    }
                    assert!(found, "missing {target} {event}: {dump}");
                }
            }

            let owner = Event::new(ResetMode::Manual, true).expect("event");
            let kept = owner.duplicate_sealed_wait_only().expect("sealed event");
            let raw = unsafe { kept.as_waitable_handle() }.as_raw_handle() as usize;
            let target = kept.clone().into();
            let seen = Activations::new();
            let recorder = Arc::clone(&seen);
            let calls = AtomicUsize::new(0);
            let wait = SealedThreadpoolWait::new(
                target,
                move |activation| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        activation.rearm(None);
                    }
                    recorder.record(activation.result());
                },
                None,
            )
            .expect("wait");
            wait.arm(None);
            seen.wait_for(2);
            wait.stop_and_drain();
            drop(wait);
            let dump = crate::trace::dump();
            assert_targets(&dump, 0);
            for line in dump.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() == 6 {
                    for field in &fields[4..] {
                        assert_ne!(
                            field.parse::<usize>().expect("trace argument"),
                            raw,
                            "leaked target: {line}"
                        );
                    }
                }
            }

            crate::trace::clear();
            let seen = Activations::new();
            let recorder = Arc::clone(&seen);
            let calls = AtomicUsize::new(0);
            let wait = super::ThreadpoolWait::new(
                owner.clone().into(),
                move |activation| {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        activation.rearm(None);
                    }
                    recorder.record(activation.result());
                },
                None,
            )
            .expect("ordinary wait");
            wait.arm(None);
            seen.wait_for(2);
            wait.stop_and_drain();
            drop(wait);
            assert_targets(&crate::trace::dump(), owner.as_raw_handle() as usize);
        },
    );
}
