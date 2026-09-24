// Copyright (c) 2026 Mike Grier
//! Does a ring write ever *pend*, and at what rate? -- the spike behind `M20.6`.
//!
//! # The question
//!
//! `examples/epoch_log`'s strategy harness is built on the premise that a log
//! "keeps appending while a commit is outstanding". Measuring it showed the
//! premise does not hold on the handle that sample opens: across 32 epochs,
//! `SubmitIoRing` for the commit took 289-555 us and returned with every
//! completion already queued, so the device flush was paid *inside submit* and
//! nothing was outstanding across the submit boundary.
//!
//! Before rebuilding that harness around a real pipeline, this asks the
//! narrower question the rebuild depends on: **which handle flags, if any,
//! make a ring write or flush pend, and how often?** "Pend" is defined
//! observationally, because it is the only form a consumer can act on:
//! `SubmitIoRing` returns and at least one completion is not yet in the queue.
//!
//! # Why this reports rates and not verdicts
//!
//! An earlier draft ran each condition **once** and printed "pended" or
//! "inline". That is the exact error [DESIGN-NOTES.md](../../DESIGN-NOTES.md)
//! records as `D-47`: the spike behind `D-24` ran its sequence a handful of
//! times, saw a barrier hold every time, and wrote down a guarantee. The real
//! violation rate was nearer one in a thousand -- which is precisely what a
//! handful of runs shows, because nothing in a passing run distinguishes
//! "guaranteed" from "usually".
//!
//! Two runs of this program, minutes apart and unchanged, reported the
//! NO_BUFFERING-extending condition as **5/500 and then 271/500**. Whatever
//! drives that -- filesystem allocation state, cache residency, something not
//! measured here -- it is outside this program's control. So even a rate is a
//! description of one run, and repeating a single condition more times would
//! not have fixed it: 1% and 54% are both 'what the platform did'.
//!
//! That draft also asserted, as a *rule*, that buffered writes complete in
//! submission order. They were **observed** to, in the runs behind the drain
//! spike. An observation over a handful of runs on one machine is not a rule
//! about the platform, and this crate has already paid once for treating one
//! as the other. So every row below is a measured frequency over [`TRIALS`]
//! trials, stated as what was seen rather than what must happen.
//!
//! # What may be built on the result: nothing
//!
//! This bounds a **frequency**. It does not establish a contract, and a row
//! reading `0/500` or `500/500` is not a licence to depend on either outcome.
//! Windows documents nothing about when a ring operation completes relative to
//! `SubmitIoRing`; whether one pends is *incidental current behaviour* of this
//! build, this device, this filesystem.
//!
//! That matters because the obvious use of this program is the wrong one. It
//! would be natural to read the rows, pick the flags that pended, and rebuild
//! `epoch_log`'s harness on them -- which would bind the sample's central
//! premise to an unspecified property, the failure the repository's PLATFORM
//! INTEGRITY rule 2 names and that this workspace has already paid for once.
//!
//! **A log, and a benchmark of one, must be correct whether an operation
//! completes inline or pends.** The specified surface is: push, submit, pop, and
//! a completion means the operation is done. Nothing in it says *when* the
//! completion becomes available. So the legitimate uses of this spike are to
//! explain why a measurement looks the way it does, and to know which
//! configurations are worth testing *across* -- never to choose one and assume
//! its behaviour holds.
//!
//! Note also what this cannot decide even descriptively: `AlternatingRings`'
//! blast-radius question is settled **structurally** and needs no run of this
//! program. `RegisteredBuffers::get_mut` refuses a slot with an operation
//! outstanding, so at most `SLOTS` appends are outstanding on a ring by
//! construction -- and each alternating lane registers its own arena of the
//! same size. The per-ring bound is identical either way, whatever the
//! platform does about pending.
//!
//! # Conditions
//!
//! Each is one handle, and [`TRIALS`] batches of [`WRITES`] writes of
//! [`WRITE_LEN`] bytes plus one flush:
//!
//! - **A** buffered, no `OVERLAPPED` -- what `epoch_log` opens today.
//! - **B** buffered + `OVERLAPPED` -- the cheapest possible fix.
//! - **C** `NO_BUFFERING` + `OVERLAPPED`, extending the file.
//! - **D** `NO_BUFFERING` + `OVERLAPPED`, over an extent written beforehand.
//!
//! C and D are separated because the drain spike observed extending writes
//! behaving like buffered ones, which it attributed to the filesystem
//! serializing writes past the valid-data length. D was the only condition
//! that spike found could discriminate at all.
//!
//! Sector alignment is why the C/D distinction matters beyond curiosity:
//! `NO_BUFFERING` requires sector-aligned buffers, offsets and lengths, and
//! `epoch_log` writes variable-length records at packed offsets. If D is the
//! only condition that pends, the harness fix is not a flag change -- it is a
//! change to the log's on-disk format.
//!
//! # What "discriminating" means here
//!
//! Not that some condition pends. That the conditions **differ**. If every row
//! reports the same rate, this program has not learned which flags matter; it
//! has learned that its own apparatus cannot tell them apart, which is the
//! failure the drain spike's control case exists to catch. It says so, rather
//! than letting a reader infer a platform conclusion from four identical rows.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, S_FALSE, S_OK};
use windows_sys::Win32::Storage::FileSystem::{
    BuildIoRingFlushFile, BuildIoRingWriteFile, CREATE_ALWAYS, CloseIoRing, CreateFileW,
    CreateIoRing, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED,
    FILE_FLUSH_DEFAULT, FILE_SHARE_READ, FILE_WRITE_FLAGS_NONE, IORING_BUFFER_REF,
    IORING_BUFFER_REF_0, IORING_CQE, IORING_CREATE_ADVISORY_FLAGS_NONE, IORING_CREATE_FLAGS,
    IORING_CREATE_REQUIRED_FLAGS_NONE, IORING_HANDLE_REF, IORING_HANDLE_REF_0, IORING_REF_RAW,
    IORING_VERSION_3, IOSQE_FLAGS_NONE, OPEN_EXISTING, PopIoRingCompletion, SubmitIoRing,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};

/// Sector size assumed for `NO_BUFFERING`. 4096 covers 512e and 4Kn alike, and
/// over-aligning is legal where under-aligning is not.
const SECTOR: usize = 4096;

/// Bytes per write. One sector, so the same size is legal in every condition.
const WRITE_LEN: usize = SECTOR;

/// Writes per batch. Matches `epoch_log`'s arena slot count, so the answer is
/// about the shape that sample actually submits.
const WRITES: usize = 8;

/// Batches per condition.
///
/// Sized against what `D-47` cost: that violation rate was around one in a
/// thousand, and a handful of trials reported it as never. This cannot resolve
/// one in a thousand either -- saying so is why the rate and the trial count
/// are printed together rather than a verdict. It is enough to separate
/// "always" from "usually" at the percent level, which is the resolution the
/// harness decision needs.
const TRIALS: usize = 500;

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn temp(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ioring-pend-spike-{tag}-{}.bin", std::process::id()))
}

/// A sector-aligned buffer. `VirtualAlloc` is page-granular, which is at least
/// sector-granular on every configuration this runs on.
fn aligned(len: usize, fill: u8) -> *mut u8 {
    // SAFETY: a null `lpAddress` lets the system choose; the result is checked.
    let p =
        unsafe { VirtualAlloc(std::ptr::null(), len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
    assert!(!p.is_null(), "VirtualAlloc failed");
    let p = p.cast::<u8>();
    // SAFETY: `len` bytes were just committed at `p`.
    unsafe { std::ptr::write_bytes(p, fill, len) };
    p
}

fn handle_ref(file: HANDLE) -> IORING_HANDLE_REF {
    IORING_HANDLE_REF {
        Kind: IORING_REF_RAW,
        Handle: IORING_HANDLE_REF_0 { Handle: file },
    }
}

fn buffer_ref(p: *mut u8) -> IORING_BUFFER_REF {
    IORING_BUFFER_REF {
        Kind: IORING_REF_RAW,
        Buffer: IORING_BUFFER_REF_0 {
            Address: p.cast::<c_void>(),
        },
    }
}

/// What one condition reported over [`TRIALS`] batches.
struct Outcome {
    label: &'static str,
    /// Trials where at least one completion was missing after submit returned.
    pended: usize,
    /// Fewest completions seen queued immediately after submit. `WRITES + 1`
    /// means no trial ever pended, even partially.
    min_queued: usize,
    submit_us: Vec<u128>,
}

impl Outcome {
    fn pct(&mut self, f: f64) -> u128 {
        if self.submit_us.is_empty() {
            return 0;
        }
        self.submit_us.sort_unstable();
        let r = (((self.submit_us.len() - 1) as f64) * f).round() as usize;
        self.submit_us[r.min(self.submit_us.len() - 1)]
    }
}

/// How a condition's file is prepared before the measured writes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Extent {
    /// Nothing: the measured writes extend the file.
    None,
    /// Zero-filled with a real write, so the valid data length covers every
    /// offset the measured writes will touch.
    Written,
    /// `set_len` only. Same end-of-file as `Written`, same bytes on read, and
    /// the whole question condition E exists to settle.
    SetLen,
    /// `set_len`, then a single write at the **end** of the extent, which
    /// obliges the filesystem to zero-fill everything in front of it.
    ///
    /// The question condition F exists to settle, raised in review: since a
    /// write past the valid data length forces the fill anyway, can that be
    /// used deliberately -- one small write instead of a buffer the size of
    /// the file -- to reach the same end state a zero-fill reaches?
    SetLenTouchEnd,
}

fn run(label: &'static str, flags: u32, extent: Extent) -> Outcome {
    let path = temp(label);
    let buffer = aligned(WRITE_LEN, 0xAB);

    // Give the writes an existing extent to land in, so they are not extending
    // writes -- which the drain spike observed behaving like buffered ones.
    match extent {
        Extent::None => {}
        Extent::Written => {
            std::fs::write(&path, vec![0u8; WRITE_LEN * (WRITES + 1)])
                .expect("zero-fill the extent");
        }
        Extent::SetLen => {
            // The question condition E exists to answer: `set_len` sets the
            // file's length without writing anything, so it costs nothing and
            // produces a file that reads back identically to the written one.
            // Whether it *behaves* identically on the write path is the thing
            // that cannot be settled by reading either file back, and was
            // being asserted from documentation until this row existed.
            let file = std::fs::File::create(&path).expect("create for set_len");
            file.set_len((WRITE_LEN * (WRITES + 1)) as u64)
                .expect("set_len the extent");
        }
        Extent::SetLenTouchEnd => {
            use std::io::{Seek, SeekFrom, Write};
            let total = WRITE_LEN * (WRITES + 1);
            let mut file = std::fs::File::create(&path).expect("create for set_len");
            file.set_len(total as u64).expect("set_len the extent");
            // One sector at the very end. Everything in front of it is now
            // between the valid data length and this write, so the filesystem
            // must zero it before this write can proceed -- which is the
            // forcing behaviour, used on purpose rather than tripped over.
            file.seek(SeekFrom::Start((total - WRITE_LEN) as u64))
                .expect("seek to the last sector");
            file.write_all(&vec![0_u8; WRITE_LEN])
                .expect("touch the end");
            file.flush().expect("flush");
        }
    }

    // A condition with an existing extent must open it: CREATE_ALWAYS would
    // truncate the extent that is the only thing distinguishing it from C.
    let disposition = if matches!(extent, Extent::None) {
        CREATE_ALWAYS
    } else {
        OPEN_EXISTING
    };
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let file = unsafe {
        CreateFileW(
            wide(&path).as_ptr(),
            GENERIC_WRITE,
            FILE_SHARE_READ,
            std::ptr::null(),
            disposition,
            FILE_ATTRIBUTE_NORMAL | flags,
            std::ptr::null_mut(),
        )
    };
    assert!(
        file as isize != -1,
        "CreateFileW({label}) failed: {}",
        std::io::Error::last_os_error()
    );

    let mut ring = std::ptr::null_mut();
    // SAFETY: out parameter is a live local for the call's duration.
    let hr = unsafe {
        CreateIoRing(
            IORING_VERSION_3,
            IORING_CREATE_FLAGS {
                Required: IORING_CREATE_REQUIRED_FLAGS_NONE,
                Advisory: IORING_CREATE_ADVISORY_FLAGS_NONE,
            },
            64,
            128,
            &raw mut ring,
        )
    };
    assert_eq!(hr, S_OK, "CreateIoRing failed: 0x{:08X}", hr as u32);

    let mut out = Outcome {
        label,
        pended: 0,
        min_queued: WRITES + 1,
        submit_us: Vec::with_capacity(TRIALS),
    };

    for _ in 0..TRIALS {
        for i in 0..WRITES {
            // SAFETY: `file` and `buffer` outlive the ring's use of them --
            // every completion is drained before this function returns.
            let hr = unsafe {
                BuildIoRingWriteFile(
                    ring,
                    handle_ref(file),
                    buffer_ref(buffer),
                    WRITE_LEN as u32,
                    (i * WRITE_LEN) as u64,
                    FILE_WRITE_FLAGS_NONE,
                    i,
                    IOSQE_FLAGS_NONE,
                )
            };
            assert_eq!(hr, S_OK, "BuildIoRingWriteFile failed: 0x{:08X}", hr as u32);
        }
        // SAFETY: as above.
        let hr = unsafe {
            BuildIoRingFlushFile(
                ring,
                handle_ref(file),
                FILE_FLUSH_DEFAULT,
                WRITES,
                IOSQE_FLAGS_NONE,
            )
        };
        assert_eq!(hr, S_OK, "BuildIoRingFlushFile failed: 0x{:08X}", hr as u32);

        // Submit with no wait, so any time spent is time the kernel spent
        // doing the work rather than time this program asked it to block.
        let mut submitted = 0u32;
        let started = Instant::now();
        // SAFETY: out parameter is a live local.
        let hr = unsafe { SubmitIoRing(ring, 0, 0, &raw mut submitted) };
        out.submit_us.push(started.elapsed().as_micros());
        assert_eq!(hr, S_OK, "SubmitIoRing failed: 0x{:08X}", hr as u32);

        // The observable: how many completions are already there. Fewer than
        // `WRITES + 1` means something had not finished when submit returned.
        let mut queued = 0usize;
        loop {
            let mut cqe: IORING_CQE = unsafe { std::mem::zeroed() };
            // SAFETY: out parameter is a live local.
            let hr = unsafe { PopIoRingCompletion(ring, &raw mut cqe) };
            if hr == S_FALSE {
                break;
            }
            assert_eq!(hr, S_OK, "PopIoRingCompletion failed: 0x{:08X}", hr as u32);
            queued += 1;
        }
        if queued < WRITES + 1 {
            out.pended += 1;
            out.min_queued = out.min_queued.min(queued);
        }

        // Drain whatever pended, so the next trial starts from an empty queue
        // and its count means what it says.
        let mut total = queued;
        let deadline = Instant::now();
        while total < WRITES + 1 {
            let mut cqe: IORING_CQE = unsafe { std::mem::zeroed() };
            // SAFETY: out parameter is a live local.
            let hr = unsafe { PopIoRingCompletion(ring, &raw mut cqe) };
            if hr == S_OK {
                total += 1;
            } else if hr != S_FALSE {
                panic!("PopIoRingCompletion failed: 0x{:08X}", hr as u32);
            }
            assert!(
                deadline.elapsed().as_secs() < 10,
                "{label}: {total} of {} completions after 10s",
                WRITES + 1
            );
        }
    }

    // SAFETY: the ring is drained, so nothing references `file` or the buffer.
    unsafe {
        CloseIoRing(ring);
        CloseHandle(file);
    }
    let _ = std::fs::remove_file(&path);
    out
}

fn main() {
    println!("Does a ring write ever pend, and at what rate? -- M20.6\n");
    println!(
        "{TRIALS} trials per condition; each trial is {WRITES} writes of {WRITE_LEN} bytes plus \
         one flush,\nsubmitted as one batch. A trial 'pended' if fewer than {} completions were \
         queued when\nSubmitIoRing returned.\n",
        WRITES + 1
    );

    let mut conditions = vec![
        run("A-buffered-sync", 0, Extent::None),
        run("B-buffered-overlapped", FILE_FLAG_OVERLAPPED, Extent::None),
        run(
            "C-nobuffer-extending",
            FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            Extent::None,
        ),
        run(
            "D-nobuffer-prewritten",
            FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            Extent::Written,
        ),
        run(
            "E-nobuffer-set_len",
            FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            Extent::SetLen,
        ),
        run(
            "F-nobuffer-touch-end",
            FILE_FLAG_OVERLAPPED | FILE_FLAG_NO_BUFFERING,
            Extent::SetLenTouchEnd,
        ),
    ];

    println!(
        "{:<24} {:>14} {:>12} {:>14} {:>14}",
        "condition", "pended/trials", "min queued", "submit p50 us", "submit p99 us"
    );
    for o in conditions.iter_mut() {
        let (p50, p99) = (o.pct(0.50), o.pct(0.99));
        println!(
            "{:<24} {:>14} {:>12} {:>14} {:>14}",
            o.label,
            format!("{}/{}", o.pended, TRIALS),
            o.min_queued,
            p50,
            p99
        );
    }
    println!();

    // Discrimination check. Four identical rows would mean this program cannot
    // tell the conditions apart -- a fact about the apparatus, not about the
    // platform, and it must not be read as "the flags do nothing".
    let rates: Vec<usize> = conditions.iter().map(|o| o.pended).collect();
    if rates.iter().all(|r| *r == rates[0]) {
        println!(
            "NOT DISCRIMINATING: every condition reported {}/{TRIALS}. That is a result about \
             this apparatus, not about the platform -- it has not shown which flags matter, only \
             that it cannot tell them apart. Do not read it as 'the flags do nothing'.",
            rates[0]
        );
    } else {
        println!("Conditions differ, so the apparatus discriminates. Reading the rows:");
        for o in &conditions {
            let verdict = match o.pended {
                0 => "never pended in this run".to_string(),
                n if n == TRIALS => "pended in every trial".to_string(),
                n => format!(
                    "pended in {n} of {TRIALS} trials ({:.1}%)",
                    100.0 * n as f64 / TRIALS as f64
                ),
            };
            println!("  {:<24} {verdict}", o.label);
        }
    }

    println!(
        "\nEvery line above is a frequency observed on this machine, this build, this device.\n\
         None of it is a platform guarantee. A rate of zero over {TRIALS} trials bounds the\n\
         frequency; it does not establish that the thing cannot happen -- which is exactly the\n\
         distinction D-47 was written to record. A rate of {TRIALS}/{TRIALS} is the same\n\
         statement pointing the other way: it may never have been false here, and it is still\n\
         not contractually true.\n\n\
         So do not pick the flags that pended and build on them. Windows specifies nothing about\n\
         when a ring operation completes relative to SubmitIoRing, and a log -- or a benchmark of\n\
         one -- has to be correct either way."
    );
}
