// Copyright (c) 2026 Mike Grier
//! End-to-end test of Model A delivery: the completion event wired to the
//! thread pool (M4.4).

#![cfg(windows)]

use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use windows_ioring_sys::{Batch, EventDelivery, IoRing, PushOptions};

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 512;

fn temp_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "windows-ioring-sys-event-delivery-{tag}-{}.tmp",
        std::process::id()
    ))
}

fn filled_content() -> Vec<u8> {
    let mut content = vec![0_u8; CHUNKS * CHUNK_LEN];
    for (chunk_index, chunk) in content.chunks_mut(CHUNK_LEN).enumerate() {
        chunk.fill(chunk_index as u8);
    }
    content
}

#[test]
fn completions_are_delivered_on_pool_threads_without_the_submitting_thread_waiting() {
    let path = temp_file("delivery");
    let content = filled_content();
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let (tx, rx) = mpsc::channel();
    let submitting_thread = std::thread::current().id();
    let saw_foreign_thread = Arc::new(AtomicBool::new(false));
    let saw_foreign_thread_for_callback = Arc::clone(&saw_foreign_thread);

    let ring = IoRing::new(64, 64).expect("create ring");
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            if std::thread::current().id() != submitting_thread {
                saw_foreign_thread_for_callback.store(true, Ordering::SeqCst);
            }
            let _ = tx.send(completion);
        },
        None,
    )
    .expect("wire event delivery");

    // Buffers are held by the token map in `windows-ioring-sys`'s own
    // submission API; here it is enough to know each op's byte count and
    // offset without keeping the buffer, since Model A hands the buffer back
    // through the completion path this test does not need to exercise
    // (submission_lifecycle.rs already covers buffer round-tripping).
    {
        let mut ring = delivery.ring().lock().expect("lock ring");
        let mut batch = Batch::new(&mut ring);
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            let _token = unsafe { batch.read(handle, buffer, offset, PushOptions::new()) }
                .expect("queue read");
        }
        // `wait_operations = 0`: this thread submits and returns immediately,
        // never waiting for a single completion itself (M4.4).
        batch.submit_and_wait(0, 0).expect("submit without waiting");
    }

    let mut received = 0;
    while received < CHUNKS {
        let completion = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("completion delivered via the pool");
        completion.result().expect("read succeeded");
        received += 1;
    }

    assert!(
        saw_foreign_thread.load(Ordering::SeqCst),
        "completions must be delivered on a pool thread, not the submitter's"
    );

    drop(delivery);
}

#[test]
fn teardown_with_operations_in_flight_neither_hangs_nor_closes_the_ring_early() {
    let path = temp_file("teardown");
    let content = vec![9_u8; 4096];
    std::fs::write(&path, &content).expect("write fixture file");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .open(&path)
        .expect("open for read");
    let handle = file.as_raw_handle();

    let delivered = Arc::new(AtomicUsize::new(0));
    let delivered_for_callback = Arc::clone(&delivered);

    let ring = IoRing::new(64, 64).expect("create ring");
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = completion.result();
            delivered_for_callback.fetch_add(1, Ordering::SeqCst);
        },
        None,
    )
    .expect("wire event delivery");

    {
        let mut ring = delivery.ring().lock().expect("lock ring");
        let mut batch = Batch::new(&mut ring);
        for _ in 0..8 {
            let buffer = vec![0_u8; content.len()];
            let _token =
                unsafe { batch.read(handle, buffer, 0, PushOptions::new()) }.expect("queue read");
        }
        batch.submit_and_wait(0, 0).expect("submit without waiting");
        // Deliberately do not wait for any of these to complete: teardown
        // below races real in-flight operations rather than ones already
        // finished.
    }

    // `drop` must neither hang (M2.4/M4.3's rundown is still bounded and
    // rechecked) nor close the ring while the wait's callback might still be
    // touching it -- the field-drop order documented on `EventDelivery`
    // itself is what this exercises.
    drop(delivery);

    // The mutex guarding the shared counter dropped along with `delivery`,
    // but the counter's `Arc` outlives it, so it is still safe to inspect: a
    // sane outcome is *some* deliveries happened before or during teardown,
    // never a panic or a hang getting here.
    let _ = delivered.load(Ordering::SeqCst);
}
