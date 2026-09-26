// Copyright (c) 2026 Mike Grier
//! M6.2: a worked example of Model A -- threadless delivery through the
//! thread pool, the shape most consumers should start with (see
//! "Two delivery architectures" in `DESIGN-NOTES.md` for when Model B, a
//! pinned thread per execution domain, is worth the extra structure
//! instead).
//!
//! This submits several reads and returns immediately (`wait_operations =
//! 0`): the submitting thread never blocks waiting for a completion. Every
//! completion is instead delivered on a thread-pool callback thread, via the
//! ring's own completion event, which [`EventDelivery`] takes from
//! [`IoRing::completion_event`] and hands to a `ThreadpoolWait`.

use std::os::windows::io::AsRawHandle;
use std::sync::mpsc;

use windows_ioring_sys::{EventDelivery, IoRing, PushOptions};

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 4096;

/// Where this example's progress reports go, kept as one seam (the repo's
/// architectural pre-step) rather than scattering `println!` across the
/// file.
struct Output<O>(O);

impl<O: std::io::Write> Output<O> {
    fn report(&mut self, message: &str) {
        let _ = writeln!(self.0, "{message}");
    }
}

fn main() -> std::io::Result<()> {
    let mut output = Output(std::io::stdout());
    let path = std::env::temp_dir().join(format!(
        "windows-ioring-sys-model-a-example-{}.tmp",
        std::process::id()
    ));
    let content: Vec<u8> = (0..CHUNKS * CHUNK_LEN).map(|i| (i % 251) as u8).collect();
    std::fs::write(&path, &content)?;
    let file = std::fs::File::open(&path)?;
    let handle = file.as_raw_handle();

    // Every completion this ring ever produces arrives through this channel,
    // from whichever pool thread happened to run the callback -- never from
    // the thread that called `submit_and_wait` below.
    let (results_tx, results_rx) = mpsc::channel();

    // The ring holds each read's buffer and hands it back with the
    // completion, so this example keeps no map of outstanding operations --
    // and therefore needs no lock around one. That matters more here than in
    // a single-threaded example: the submitting thread and the pool thread
    // that runs the callback would otherwise both need access to it.
    let ring = IoRing::<Vec<u8>>::with_inventory(64, 64)?;
    let delivery = EventDelivery::new(
        ring,
        move |completion, held| {
            let _ = results_tx.send((completion, held));
        },
        None,
    )?;

    {
        let mut scope = delivery.scope();
        let mut batch = scope.batch();
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            // SAFETY: `handle` stays open for this whole example.
            unsafe { batch.read_raw_owned(handle, buffer, (), offset, PushOptions::new()) }?;
        }
        // `wait_operations = 0`: submit and return immediately. This thread
        // is done with the ring the instant this call returns.
        batch.submit_and_wait(0, 0)?;
    }

    let mut verified = 0;
    while verified < CHUNKS {
        let (completion, held) = results_rx
            .recv()
            .expect("a completion for every submitted read");
        let transferred = completion.result()?;
        let (buffer, ()) = held.expect("the ring was holding this read's buffer");
        let buffer = buffer.expect("a read carries a buffer");
        assert_eq!(transferred, CHUNK_LEN);
        output.report(&format!(
            "chunk at user_data {} verified, first byte {}",
            completion.user_data(),
            buffer[0]
        ));
        verified += 1;
    }

    output.report(&format!(
        "all {CHUNKS} chunks delivered on pool threads; this thread never waited for one"
    ));
    drop(delivery);
    let _ = std::fs::remove_file(&path);
    Ok(())
}
