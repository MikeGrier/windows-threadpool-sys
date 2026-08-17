// Copyright (c) 2026 Mike Grier
//! Integration test: several endpoints share one `CompletionPort` whose
//! completion stream is drained concurrently by multiple worker threads, then
//! run down. Exercises the multi-endpoint / multi-threaded drain decision:
//! attribution by completion key and by operation identity, correct accounting
//! under concurrent `get`, and a clean rundown afterwards.

#![cfg(windows)]

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_overlapped_io_sys::{
    CompletionPort, Issued, Operation, Submitted, UnassociatedEndpoint,
};
use windows_sys::Win32::Foundation::ERROR_IO_PENDING;
use windows_sys::Win32::Storage::FileSystem::ReadFile;

fn temp_file_with(content: &[u8], tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "windows-overlapped-io-sys-shared-drain-{tag}-{}.tmp",
        std::process::id()
    ));
    std::fs::write(&path, content).expect("write temp file");
    path
}

fn open_overlapped(path: &Path) -> UnassociatedEndpoint {
    UnassociatedEndpoint::open(path, true, false, 0).expect("open overlapped endpoint")
}

#[test]
fn multi_threaded_drain_across_several_endpoints() {
    const ENDPOINTS: usize = 4;
    const OPS_PER_ENDPOINT: usize = 32;
    const WORKERS: usize = 3;
    const TOTAL: usize = ENDPOINTS * OPS_PER_ENDPOINT;

    // Each file holds OPS_PER_ENDPOINT distinct bytes.
    let content: Vec<u8> = (0..OPS_PER_ENDPOINT).map(|i| i as u8).collect();

    let port = CompletionPort::new(0).expect("create port");

    // Associate several endpoints, each under its own key equal to its index.
    let mut paths = Vec::with_capacity(ENDPOINTS);
    let mut endpoints = Vec::with_capacity(ENDPOINTS);
    for e in 0..ENDPOINTS {
        let path = temp_file_with(&content, &format!("ep{e}"));
        let endpoint = port
            .associate(open_overlapped(&path), e)
            .expect("associate");
        paths.push(path);
        endpoints.push(endpoint);
    }

    // Stable per-endpoint buffers; kept alive until every completion is claimed.
    let mut buffers: Vec<Vec<u8>> = (0..ENDPOINTS)
        .map(|_| vec![0_u8; OPS_PER_ENDPOINT])
        .collect();
    let bases: Vec<*mut u8> = buffers.iter_mut().map(|b| b.as_mut_ptr()).collect();

    // Submit OPS_PER_ENDPOINT one-byte reads on each endpoint; the payload
    // carries the (endpoint, slot) identity for attribution on the worker side.
    for e in 0..ENDPOINTS {
        let base = bases[e];
        for s in 0..OPS_PER_ENDPOINT {
            let mut operation = Operation::new((e, s));
            operation.set_offset(s as u64);
            // SAFETY: issues exactly one 1-byte overlapped ReadFile into this
            // endpoint's own buffer at slot s; base points to a live
            // OPS_PER_ENDPOINT-byte Vec and s < OPS_PER_ENDPOINT.
            let submitted = unsafe {
                endpoints[e].submit(operation, |handle, overlapped| {
                    let ok = ReadFile(
                        handle.as_raw_handle(),
                        base.add(s),
                        1,
                        ptr::null_mut(),
                        overlapped,
                    );
                    if ok != 0 {
                        return Ok(Issued::Pending);
                    }
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                        Ok(Issued::Pending)
                    } else {
                        Err(error)
                    }
                })
            };
            match submitted {
                Submitted::Pending(_) => {}
                Submitted::Completed { .. } => {
                    panic!("skip mode not set; no inline completion expected")
                }
                Submitted::Failed { error, .. } => panic!("submit failed at ({e}, {s}): {error}"),
            }
        }
    }
    assert_eq!(port.outstanding(), TOTAL);

    // Drain concurrently from several worker threads. A `Completion` is not
    // `Send`, so each worker claims the completions it dequeues on its own
    // thread and returns only the identities it observed.
    let remaining = AtomicUsize::new(TOTAL);
    let mut seen: Vec<(usize, usize)> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            let port = &port;
            let remaining = &remaining;
            handles.push(scope.spawn(move || {
                let mut local = Vec::new();
                while remaining.load(Ordering::SeqCst) > 0 {
                    // A finite timeout lets a worker re-check `remaining` after
                    // another thread has taken the last packet; None is a timeout.
                    if let Some(completion) = port.get(500).expect("get") {
                        assert!(completion.error().is_none());
                        assert_eq!(completion.bytes_transferred(), 1);
                        let key = completion.key();
                        // SAFETY: this completion is an Operation<(usize, usize)>
                        // submitted above and is claimed exactly once here.
                        let operation = unsafe { completion.claim::<(usize, usize)>() };
                        let (e, s) = *operation.payload();
                        assert_eq!(key, e, "completion key must identify its endpoint");
                        local.push((e, s));
                        remaining.fetch_sub(1, Ordering::SeqCst);
                    }
                }
                local
            }));
        }
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("worker thread"))
            .collect()
    });

    assert_eq!(port.outstanding(), 0);
    assert_eq!(seen.len(), TOTAL);

    // Every (endpoint, slot) identity was observed exactly once.
    seen.sort_unstable();
    let expected: Vec<(usize, usize)> = (0..ENDPOINTS)
        .flat_map(|e| (0..OPS_PER_ENDPOINT).map(move |s| (e, s)))
        .collect();
    assert_eq!(seen, expected);

    // A final voluntary rundown is a no-op now that the count is zero.
    port.run_down().expect("run_down");

    drop(endpoints);
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
