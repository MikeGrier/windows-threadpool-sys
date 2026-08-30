// Copyright (c) 2026 Mike Grier
use super::EventDelivery;
use crate::IoRing;

#[test]
fn new_succeeds_and_the_ring_stays_reachable_for_pushes() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    let info = delivery.scope().info().expect("query info");
    assert!(info.submission_queue_size > 0);
}

#[test]
fn dropping_with_nothing_outstanding_does_not_hang() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");
    drop(delivery);
}

// --- the scope's read-only surface (M18.4) -----------------------------------
//
// M18.6 gave `RingScope` the whole read-only surface of `IoRing` on the stated
// principle that a platform layer is not narrowed to its current caller. M18.3's
// mutation run then showed that *every one* of those accessors survived, because
// no test called any of them: the principle was sound and the coverage was
// absent. These are the tests that were missing.

use crate::{Op, PushOptions};

#[test]
fn a_scope_reports_the_rings_static_properties() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let expected_version = ring.version();
    let expected_read = ring.supports(Op::Read);
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");

    let scope = delivery.scope();

    assert_eq!(
        scope.version(),
        expected_version,
        "the scope must report the ring's own version, not a fresh one"
    );
    assert_eq!(scope.supports(Op::Read), expected_read);
    assert!(
        scope.supports(Op::Read),
        "a host running these tests supports Read"
    );
    // Both directions: one alone leaves the accessor indistinguishable from the
    // constant that happens to match it.
    assert!(
        !scope.supports_raw(0xFFFF),
        "a reserved opcode is not supported"
    );
    assert!(
        scope.supports_raw(Op::Read.code()),
        "a real opcode is supported, so `supports_raw` cannot be a constant false"
    );
    assert_eq!(scope.info().expect("query info").submission_queue_size, 8);
}

#[test]
fn a_scope_reports_registration_counts_that_change_with_registrations() {
    let ring = IoRing::new(8, 8).expect("create ring");
    let delivery = EventDelivery::new(ring, |_completion| {}, None).expect("wire event delivery");

    assert_eq!(delivery.scope().registered_file_count(), 0);
    assert_eq!(delivery.scope().registered_buffer_count(), 0);

    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        let pending = batch
            .register_buffers(vec![vec![0_u8; 32], vec![0_u8; 32], vec![0_u8; 32]])
            .expect("queue buffer registration");
        batch.submit().expect("submit");
        // Deliberately leaked: the completion is delivered to the pool, so this
        // thread never claims it, and the registration must outlive the buffers.
        std::mem::forget(pending);
    }

    // The count is reserved at push time, so it is observable without waiting
    // for the pool to deliver anything.
    assert_eq!(
        delivery.scope().registered_buffer_count(),
        3,
        "the count must follow the registration rather than being a constant"
    );

    // Files too: a test that only ever sees zero cannot tell the accessor from
    // a constant zero, which is how this survived M18.3.
    let path = std::env::temp_dir().join(format!("ioring-scope-files-{}.tmp", std::process::id()));
    std::fs::write(&path, b"x").expect("write fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);
    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        // SAFETY: `file` outlives the registration -- it is dropped at the end
        // of this test, after the delivery that owns the ring.
        let pending =
            unsafe { batch.register_files(&[handle, handle]) }.expect("queue file registration");
        batch.submit().expect("submit");
        std::mem::forget(pending);
    }
    assert_eq!(
        delivery.scope().registered_file_count(),
        2,
        "the file count must follow the registration too"
    );

    drop(delivery);
    drop(file);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_scope_reports_outstanding_work() {
    let path = std::env::temp_dir().join(format!(
        "ioring-scope-outstanding-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, vec![5_u8; 4096]).expect("write fixture");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open fixture");
    let handle = std::os::windows::io::AsRawHandle::as_raw_handle(&file);

    let ring = IoRing::new(8, 8).expect("create ring");
    let (tx, rx) = std::sync::mpsc::channel();
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = tx.send(completion.user_data());
        },
        None,
    )
    .expect("wire event delivery");

    assert_eq!(
        delivery.scope().outstanding(),
        0,
        "a fresh ring owes nothing"
    );

    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        // SAFETY: `file` outlives the operation -- this test waits for its
        // completion before returning.
        let token = unsafe { batch.read_raw(handle, vec![0_u8; 512], 0, PushOptions::new()) }
            .expect("queue read");
        batch.submit().expect("submit");
        assert_eq!(
            scope.outstanding(),
            1,
            "one submitted-and-unpopped operation must be reported as outstanding"
        );
        std::mem::forget(token);
    }

    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("the completion is delivered");

    drop(delivery);
    drop(file);
    let _ = std::fs::remove_file(&path);
}
