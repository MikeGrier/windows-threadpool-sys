// Copyright (c) Mike Grier
//! Spike: what does `SetIoRingCompletionEvent` actually permit?
//!
//! Questions, from the response to the third-delivery-shape proposal:
//!   Q1 may it be called on a fresh ring?                (baseline; known yes)
//!   Q2 may it be called a second time, replacing the event?
//!   Q3 may it be called while operations are outstanding?
//!   Q4 does passing NULL clear it, and does the ring still work afterwards?
//!   Q5 auto-reset semantics: is the event set per completion, and is
//!      `wait; drain-to-empty` sufficient with no second drain?
//!   Q6 does a DuplicateHandle'd event still get signalled after the
//!      original handle is closed?  (validates handing the caller a duplicate)

use std::ffi::c_void;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, HANDLE, S_FALSE, S_OK,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingReadFile, CloseIoRing, CreateFileW, CreateIoRing, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    IORING_BUFFER_REF, IORING_BUFFER_REF_0, IORING_CAPABILITIES, IORING_CQE,
    IORING_CREATE_ADVISORY_FLAGS_NONE, IORING_CREATE_FLAGS, IORING_CREATE_REQUIRED_FLAGS_NONE,
    IORING_FEATURE_SET_COMPLETION_EVENT, IORING_FEATURE_UM_EMULATION, IORING_HANDLE_REF,
    IORING_HANDLE_REF_0, IORING_REF_RAW, IORING_VERSION_3, IOSQE_FLAGS_NONE, OPEN_EXISTING,
    PopIoRingCompletion, QueryIoRingCapabilities, SetIoRingCompletionEvent, SubmitIoRing,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, INFINITE, WaitForSingleObject,
};

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn hr(name: &str, value: i32) -> String {
    let tag = match value {
        S_OK => " (S_OK)",
        S_FALSE => " (S_FALSE)",
        _ => "",
    };
    format!("{name} -> 0x{:08X}{tag}", value as u32)
}

/// A fresh event handle.
fn make_event() -> HANDLE {
    // SAFETY: unnamed auto-reset event, default security, all pointers null.
    let handle = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) };
    assert!(!handle.is_null(), "CreateEventW failed");
    handle
}

/// Is `event` currently signalled? Consumes the signal on an auto-reset event.
fn poll(event: HANDLE) -> bool {
    // SAFETY: `event` is a live event handle.
    let result = unsafe { WaitForSingleObject(event, 0) };
    match result {
        WAIT_OBJECT_0 => true,
        WAIT_TIMEOUT => false,
        other => panic!("unexpected wait result {other}"),
    }
}

fn main() {
    // ---- capability gate -------------------------------------------------
    let mut caps = IORING_CAPABILITIES::default();
    // SAFETY: valid out-pointer.
    let rc = unsafe { QueryIoRingCapabilities(&raw mut caps) };
    assert_eq!(rc, S_OK, "QueryIoRingCapabilities failed: 0x{:08X}", rc as u32);
    let has_event = caps.FeatureFlags & IORING_FEATURE_SET_COMPLETION_EVENT != 0;
    let emulated = caps.FeatureFlags & IORING_FEATURE_UM_EMULATION != 0;
    println!("== environment ==");
    println!("max version      : {}", caps.MaxVersion);
    println!("feature flags    : 0x{:08X}", caps.FeatureFlags);
    println!("  SET_COMPLETION_EVENT : {has_event}");
    println!("  UM_EMULATION         : {emulated}");
    println!("max SQ / CQ      : {} / {}", caps.MaxSubmissionQueueSize, caps.MaxCompletionQueueSize);
    if !has_event {
        println!("\nSET_COMPLETION_EVENT unavailable; the spike cannot answer anything.");
        return;
    }

    // ---- a file to read from --------------------------------------------
    let path = std::env::current_exe().expect("current_exe");
    let path_w = wide(&path.to_string_lossy());
    // SAFETY: null-terminated wide path; standard open of an existing file.
    let file = unsafe {
        CreateFileW(
            path_w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    assert!(!file.is_null(), "CreateFileW failed");

    let mut ring: *mut c_void = ptr::null_mut();
    let flags = IORING_CREATE_FLAGS {
        Required: IORING_CREATE_REQUIRED_FLAGS_NONE,
        Advisory: IORING_CREATE_ADVISORY_FLAGS_NONE,
    };
    // SAFETY: valid out-pointer; sizes within reported caps.
    let rc = unsafe { CreateIoRing(IORING_VERSION_3, flags, 16, 32, &raw mut ring) };
    assert_eq!(rc, S_OK, "CreateIoRing failed: 0x{:08X}", rc as u32);

    let file_ref = IORING_HANDLE_REF {
        Kind: IORING_REF_RAW,
        Handle: IORING_HANDLE_REF_0 { Handle: file },
    };

    // Each queued read gets its own buffer; the kernel writes through these
    // for as long as the op is outstanding, so they must outlive every op.
    let mut buffers: Vec<Box<[u8; 512]>> = (0..8).map(|_| Box::new([0_u8; 512])).collect();

    let mut push = |ring: *mut c_void, slot: usize, user_data: usize| -> i32 {
        let address = buffers[slot].as_mut_ptr().cast::<c_void>();
        let buffer_ref = IORING_BUFFER_REF {
            Kind: IORING_REF_RAW,
            Buffer: IORING_BUFFER_REF_0 { Address: address },
        };
        // SAFETY: live ring, live file, buffer outlives the op (drained below).
        unsafe {
            BuildIoRingReadFile(
                ring,
                file_ref,
                buffer_ref,
                512,
                0,
                user_data,
                IOSQE_FLAGS_NONE,
            )
        }
    };

    // SAFETY (all SetIoRingCompletionEvent calls below): live ring; each event
    // handle stays open until after the ring is closed, except where a case
    // deliberately tests otherwise and says so.
    let set = |ring: *mut c_void, event: HANDLE| -> i32 {
        unsafe { SetIoRingCompletionEvent(ring, event) }
    };

    let submit = |ring: *mut c_void, wait_n: u32, timeout: u32| -> (i32, u32) {
        let mut submitted = 0_u32;
        // SAFETY: live ring; valid out-pointer.
        let rc = unsafe { SubmitIoRing(ring, wait_n, timeout, &raw mut submitted) };
        (rc, submitted)
    };

    let pop = |ring: *mut c_void| -> (i32, IORING_CQE) {
        let mut cqe = IORING_CQE { UserData: 0, ResultCode: 0, Information: 0 };
        // SAFETY: live ring; valid out-pointer.
        let rc = unsafe { PopIoRingCompletion(ring, &raw mut cqe) };
        (rc, cqe)
    };

    let drain = |ring: *mut c_void| -> usize {
        let mut count = 0;
        loop {
            let (rc, _) = pop(ring);
            if rc == S_FALSE {
                return count;
            }
            assert_eq!(rc, S_OK, "PopIoRingCompletion failed: 0x{:08X}", rc as u32);
            count += 1;
        }
    };

    // ---- Q1: fresh ring --------------------------------------------------
    println!("\n== Q1: set on a fresh ring ==");
    let event1 = make_event();
    println!("{}", hr("set(event1)", set(ring, event1)));

    // ---- Q2: replace, no ops outstanding ---------------------------------
    println!("\n== Q2: replace the event, nothing outstanding ==");
    let event2 = make_event();
    let rc2 = set(ring, event2);
    println!("{}", hr("set(event2)", rc2));
    if rc2 == S_OK {
        assert_eq!(S_OK, push(ring, 0, 0xA1), "push failed");
        let (rc, n) = submit(ring, 0, 0);
        println!("{}, submitted={n}", hr("submit", rc));
        // SAFETY: live event handles.
        let woke2 = unsafe { WaitForSingleObject(event2, 2000) } == WAIT_OBJECT_0;
        let old_signalled = poll(event1);
        println!("event2 signalled : {woke2}");
        println!("event1 signalled : {old_signalled}  (expect false if replace is real)");
        println!("drained          : {}", drain(ring));
    }

    // ---- Q3: set while operations are outstanding ------------------------
    println!("\n== Q3: set while operations are outstanding ==");
    for (slot, user_data) in (0xB0..0xB4).enumerate() {
        assert_eq!(S_OK, push(ring, slot, user_data), "push failed");
    }
    let (rc, n) = submit(ring, 0, 0);
    println!("{}, submitted={n} (not drained -- deliberately outstanding)", hr("submit", rc));
    let event3 = make_event();
    let rc3 = set(ring, event3);
    println!("{}", hr("set(event3) with ops in flight", rc3));
    if rc3 == S_OK {
        // SAFETY: live event handle.
        let woke3 = unsafe { WaitForSingleObject(event3, 2000) } == WAIT_OBJECT_0;
        println!("event3 signalled : {woke3}");
    }
    println!("drained          : {}", drain(ring));

    // ---- Q4: clear with NULL ---------------------------------------------
    println!("\n== Q4: clear with NULL ==");
    let rc4 = set(ring, ptr::null_mut());
    println!("{}", hr("set(NULL)", rc4));
    if rc4 == S_OK {
        assert_eq!(S_OK, push(ring, 0, 0xC1), "push failed");
        let (rc, _) = submit(ring, 1, 2000);
        println!("{} (ring still usable after clear)", hr("submit_and_wait", rc));
        println!("drained          : {}", drain(ring));
        println!("event3 signalled : {}  (expect false if NULL really cleared)", poll(event3));
        // Re-arm for the remaining cases.
        println!("{}", hr("set(event3) again after clear", set(ring, event3)));
    }

    // ---- Q5: auto-reset semantics, wait then drain-to-empty --------------
    println!("\n== Q5: is `wait; drain-to-empty` sufficient? ==");
    let _ = poll(event3); // start from a known-unsignalled state
    for (slot, user_data) in (0xD0..0xD8).enumerate() {
        assert_eq!(S_OK, push(ring, slot, user_data), "push failed");
    }
    let (rc, n) = submit(ring, 0, 0);
    println!("{}, submitted={n}", hr("submit 8 reads", rc));
    let mut total = 0;
    let mut waits = 0;
    while total < 8 {
        // SAFETY: live event handle.
        let woke = unsafe { WaitForSingleObject(event3, 5000) };
        assert_eq!(woke, WAIT_OBJECT_0, "timed out waiting for completions");
        waits += 1;
        let got = drain(ring);
        total += got;
        println!("  wait #{waits} -> drained {got} (total {total}/8)");
    }
    println!("waits needed for 8 completions: {waits}");
    println!("event still signalled after final drain: {}", poll(event3));

    // ---- Q6: does a duplicate stay signalled? ----------------------------
    println!("\n== Q6: duplicate the event, close the original ==");
    let original = make_event();
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: both handles are live; same-process duplication.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            original,
            GetCurrentProcess(),
            &raw mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    };
    assert!(ok != 0, "DuplicateHandle failed");
    println!("{}", hr("set(original)", set(ring, original)));
    // SAFETY: `original` is live and the kernel keeps its own reference to the
    // underlying event object; `duplicate` still names that object.
    let closed = unsafe { CloseHandle(original) };
    println!("CloseHandle(original) -> {}", closed != 0);
    assert_eq!(S_OK, push(ring, 0, 0xE1), "push failed");
    let (rc, _) = submit(ring, 0, 0);
    println!("{}", hr("submit", rc));
    // SAFETY: live duplicate handle.
    let woke = unsafe { WaitForSingleObject(duplicate, 2000) } == WAIT_OBJECT_0;
    println!("duplicate signalled after original closed: {woke}");
    println!("drained          : {}", drain(ring));

    // ---- Q7: does attaching AFTER completions land signal the backlog? ---
    // Q3 hinted at this: 4 completions were drainable but the event never
    // fired. Isolate it -- detach, let completions land with no event
    // attached, then attach and see whether the backlog signals.
    println!("
== Q7: attach an event after completions have already landed ==");
    println!("{}", hr("set(NULL) to detach", set(ring, ptr::null_mut())));
    let event7 = make_event();
    for (slot, user_data) in (0xF0..0xF2).enumerate() {
        assert_eq!(S_OK, push(ring, slot, user_data), "push failed");
    }
    let (rc, n) = submit(ring, 0, 0);
    println!("{}, submitted={n}", hr("submit 2 reads (no event attached)", rc));
    std::thread::sleep(std::time::Duration::from_millis(300));
    println!("(slept 300ms -- completions have certainly landed in the CQ)");
    println!("{}", hr("set(event7) now", set(ring, event7)));
    // SAFETY: live event handle.
    let backlog_woke = unsafe { WaitForSingleObject(event7, 1000) } == WAIT_OBJECT_0;
    println!("event7 signalled by the backlog: {backlog_woke}   <-- KEY RESULT");

    // Q7b: is the event functional for a *new* completion, or dead entirely?
    assert_eq!(S_OK, push(ring, 2, 0xF9), "push failed");
    let (rc, _) = submit(ring, 0, 0);
    println!("{}", hr("submit 1 more read", rc));
    // SAFETY: live event handle.
    let new_woke = unsafe { WaitForSingleObject(event7, 2000) } == WAIT_OBJECT_0;
    println!("event7 signalled by the new completion: {new_woke}");
    println!("drained (backlog + new): {}", drain(ring));

    // ---- Q8: is one signal per completion, or one per batch? -------------
    // Q5 drained all 8 after a single wait, which cannot distinguish
    // "one signal for the batch" from "8 signals coalesced into an
    // auto-reset event". Submit, drain fully, then check whether the event
    // is left spuriously signalled by the extra completions.
    println!("
== Q8: leftover signal after a full drain? ==");
    let _ = poll(event7);
    for (slot, user_data) in (0x50..0x54).enumerate() {
        assert_eq!(S_OK, push(ring, slot, user_data), "push failed");
    }
    let (_, _) = submit(ring, 0, 0);
    // SAFETY: live event handle.
    assert_eq!(unsafe { WaitForSingleObject(event7, 5000) }, WAIT_OBJECT_0);
    let drained = drain(ring);
    let leftover_signal = poll(event7);
    println!("drained after one wait      : {drained}");
    println!("event signalled after drain : {leftover_signal}");
    println!("  (true => a spurious extra wakeup is possible; a caller's");
    println!("   drain-to-empty loop must tolerate waking with nothing to pop)");

    // ---- Q9: is the signal edge-triggered on empty -> non-empty? ---------
    // Q7b showed a NEW completion failing to signal while a backlog sat in
    // the CQ, but Q8 then signalled fine after a full drain. That is exactly
    // the signature of an edge trigger on the queue becoming non-empty.
    // Test it directly.
    println!("
== Q9: edge-triggered on empty -> non-empty? ==");
    assert_eq!(drain(ring), 0, "CQ must start empty");
    let _ = poll(event7);

    assert_eq!(S_OK, push(ring, 0, 0x91), "push failed");
    let (_, _) = submit(ring, 0, 0);
    // SAFETY: live event handle.
    let first = unsafe { WaitForSingleObject(event7, 5000) } == WAIT_OBJECT_0;
    println!("completion #1 into an EMPTY queue signalled : {first}");

    // Deliberately do NOT drain -- the CQ stays non-empty.
    let _ = poll(event7); // consume any residual signal
    assert_eq!(S_OK, push(ring, 1, 0x92), "push failed");
    let (_, _) = submit(ring, 0, 0);
    // SAFETY: live event handle.
    let second = unsafe { WaitForSingleObject(event7, 1500) } == WAIT_OBJECT_0;
    println!("completion #2 into a NON-EMPTY queue signalled: {second}   <-- KEY RESULT");

    let flushed = drain(ring);
    println!("(drained {flushed}; queue is empty again)");
    let _ = poll(event7);
    assert_eq!(S_OK, push(ring, 2, 0x93), "push failed");
    let (_, _) = submit(ring, 0, 0);
    // SAFETY: live event handle.
    let third = unsafe { WaitForSingleObject(event7, 5000) } == WAIT_OBJECT_0;
    println!("completion #3 into an EMPTY queue signalled : {third}");
    println!("drained          : {}", drain(ring));

    println!("
-> verdict: {}", if first && !second && third {
        "EDGE-TRIGGERED on empty -> non-empty. A waiter MUST drain to empty 
   before waiting again, or it will sleep with work still queued."
    } else {
        "does NOT match the simple edge-trigger model -- re-examine."
    });

    // ---- teardown --------------------------------------------------------
    // Quiesce before closing: nothing may be outstanding when CloseIoRing runs.
    let (_, _) = submit(ring, 0, 0);
    let leftover = drain(ring);
    if leftover > 0 {
        println!("\n(drained {leftover} stragglers at teardown)");
    }
    // SAFETY: all operations drained above; handles live.
    unsafe {
        let rc = CloseIoRing(ring);
        assert_eq!(rc, S_OK, "CloseIoRing failed: 0x{:08X}", rc as u32);
        CloseHandle(file);
        CloseHandle(event1);
        CloseHandle(event2);
        CloseHandle(event3);
        CloseHandle(event7);
        CloseHandle(duplicate);
    }
    let _ = INFINITE;
    println!("\ndone.");
}
