// Copyright (c) 2026 Mike Grier
//! M6.2: a worked example of Model A -- threadless delivery through the
//! thread pool, the shape most consumers should start with (see
//! "Two delivery architectures" in `DESIGN-NOTES.md` for when Model B, a
//! pinned thread per execution domain, is worth the extra structure
//! instead).
//!
//! This submits several reads and returns immediately (`wait_operations =
//! 0`): the submitting thread never blocks waiting for a completion. Every
//! completion is instead delivered on a thread-pool callback thread, via
//! [`EventDelivery`]'s wired-up `SetIoRingCompletionEvent`.

use std::collections::HashMap;
use std::os::windows::io::AsRawHandle;
use std::sync::Mutex;
use std::sync::mpsc;

use windows_ioring_sys::{Batch, EventDelivery, IoRing, PushOptions, Token};

const CHUNKS: usize = 8;
const CHUNK_LEN: usize = 4096;

fn main() -> std::io::Result<()> {
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

    let ring = IoRing::new(64, 64)?;
    let delivery = EventDelivery::new(
        ring,
        move |completion| {
            let _ = results_tx.send(completion);
        },
        None,
    )?;

    // Hold each chunk's token until its completion arrives, keyed by the
    // `UserData` identity `Batch::read` minted for it.
    let tokens: Mutex<HashMap<usize, Token<Vec<u8>>>> = Mutex::new(HashMap::new());
    {
        let mut ring = delivery.ring().lock().expect("lock ring");
        let mut batch = Batch::new(&mut ring);
        let mut tokens = tokens.lock().expect("lock tokens");
        for chunk_index in 0..CHUNKS {
            let buffer = vec![0_u8; CHUNK_LEN];
            let offset = (chunk_index * CHUNK_LEN) as u64;
            let token = batch.read(handle, buffer, offset, PushOptions::new())?;
            tokens.insert(token.id(), token);
        }
        // `wait_operations = 0`: submit and return immediately. This thread
        // is done with the ring the instant this call returns.
        batch.submit_and_wait(0, 0)?;
    }

    let mut verified = 0;
    while verified < CHUNKS {
        let completion = results_rx
            .recv()
            .expect("a completion for every submitted read");
        let transferred = completion.result()?;
        let token = tokens
            .lock()
            .expect("lock tokens")
            .remove(&completion.user_data())
            .expect("completion matches a held token");
        let buffer = token
            .claim_if(completion.user_data())
            .expect("token claims its own completion");
        assert_eq!(transferred, CHUNK_LEN);
        println!(
            "chunk at user_data {} verified, first byte {}",
            completion.user_data(),
            buffer[0]
        );
        verified += 1;
    }

    println!("all {CHUNKS} chunks delivered on pool threads; this thread never waited for one");
    drop(delivery);
    let _ = std::fs::remove_file(&path);
    Ok(())
}
