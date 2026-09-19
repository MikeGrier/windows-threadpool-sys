// Copyright (c) Mike Grier.
use super::*;

#[test]
fn budget_rejects_expiry_and_cancellation_but_accepts_live_work() {
    let cancel = AtomicBool::new(false);
    let mut budget = Budget {
        deadline: Instant::now() + Duration::from_secs(1),
        cancel: &cancel,
    };
    assert!(budget.check().is_ok());
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        budget.check().unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    cancel.store(false, Ordering::Relaxed);
    budget.deadline = Instant::now();
    assert_eq!(budget.check().unwrap_err().kind(), io::ErrorKind::TimedOut);
}

#[test]
fn failure_guard_cancels_on_failure_and_unwind_not_success() {
    let cancel = AtomicBool::new(false);
    drop(CancelOnFailure {
        cancel: &cancel,
        armed: false,
    });
    assert!(!cancel.load(Ordering::Relaxed));
    drop(CancelOnFailure {
        cancel: &cancel,
        armed: true,
    });
    assert!(cancel.load(Ordering::Relaxed));
    cancel.store(false, Ordering::Relaxed);
    let result = std::panic::catch_unwind(|| {
        let _failure = CancelOnFailure {
            cancel: &cancel,
            armed: true,
        };
        panic!("injected worker failure");
    });
    assert!(result.is_err());
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn checksum_budget_failure_propagates() {
    let mut checks = 0;
    let result = checksum_checked(&[0; 1024], 5, || {
        checks += 1;
        if checks == 3 {
            Err(failed("injected checksum failure"))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(checks, 3);
}

fn batch_config(batch_size: usize) -> Config {
    Config {
        file: "unused".into(),
        block_bytes: 64,
        depth: 2,
        buffer_count: Some(64),
        batch_size,
        queue_capacity: 64,
        checksum_passes: 1,
        repetitions: 1,
        timeout_ms: 1000,
        processors: None,
    }
}

fn batch_job(id: usize) -> Job {
    let submitted = Instant::now();
    Job {
        id,
        buffer: vec![id as u8; id + 1],
        submitted,
        read_done: submitted,
    }
}

#[test]
fn processor_batches_flush_exact_full_and_partial_groups() {
    for (blocks, quantum) in [
        (0, 4),
        (1, 1),
        (1, 16),
        (2, 1),
        (2, 2),
        (3, 2),
        (7, 3),
        (8, 4),
        (9, 4),
        (17, 16),
        (33, 1024),
    ] {
        let (send, receive) = spsc::bounded(64).unwrap();
        let (returns, returned) = spsc::bounded(64).unwrap();
        for id in 0..blocks {
            assert!(send.push(batch_job(id)).is_ok());
        }
        let cancel = AtomicBool::new(false);
        let budget = Budget {
            deadline: Instant::now() + Duration::from_secs(1),
            cancel: &cancel,
        };
        let mut batches = BatchStats::default();
        processor_loop(
            &receive,
            &returns,
            blocks,
            &batch_config(quantum),
            &budget,
            &mut batches,
        )
        .unwrap();
        assert_eq!(batches.jobs, blocks);
        assert_eq!(batches.count, blocks.div_ceil(quantum));
        assert_eq!(batches.partial, usize::from(blocks % quantum != 0));
        assert_eq!(batches.max_jobs, blocks.min(quantum));
        for id in 0..blocks {
            let result = returned.pop().unwrap().result;
            assert_eq!(result.id, id);
            assert_eq!(result.hash, crate::checksum(&vec![id as u8; id + 1], 1));
        }
        assert!(matches!(returned.pop(), Err(TryRecvError::Empty)));
    }
}

#[test]
fn processor_batches_stop_on_cancel_expiry_and_disconnection() {
    for cancelled in [false, true] {
        let (send, receive) = spsc::bounded(2).unwrap();
        let (returns, returned) = spsc::bounded(2).unwrap();
        assert!(send.push(batch_job(0)).is_ok());
        let cancel = AtomicBool::new(cancelled);
        let budget = Budget {
            deadline: Instant::now(),
            cancel: &cancel,
        };
        let error = processor_loop(
            &receive,
            &returns,
            1,
            &batch_config(16),
            &budget,
            &mut BatchStats::default(),
        )
        .unwrap_err();
        assert_eq!(
            error.kind(),
            if cancelled {
                io::ErrorKind::Interrupted
            } else {
                io::ErrorKind::TimedOut
            }
        );
        assert!(matches!(returned.pop(), Err(TryRecvError::Empty)));
    }
    let cancel = AtomicBool::new(false);
    let budget = Budget {
        deadline: Instant::now() + Duration::from_secs(1),
        cancel: &cancel,
    };
    let (send, receive) = spsc::bounded(2).unwrap();
    let (returns, returned) = spsc::bounded(2).unwrap();
    drop(send);
    assert!(
        processor_loop(
            &receive,
            &returns,
            1,
            &batch_config(16),
            &budget,
            &mut BatchStats::default()
        )
        .unwrap_err()
        .to_string()
        .contains("reader stopped")
    );
    let (send, receive) = spsc::bounded(2).unwrap();
    assert!(send.push(batch_job(0)).is_ok());
    drop(returned);
    assert!(
        processor_loop(
            &receive,
            &returns,
            1,
            &batch_config(16),
            &budget,
            &mut BatchStats::default()
        )
        .unwrap_err()
        .to_string()
        .contains("reader disconnected")
    );
}
