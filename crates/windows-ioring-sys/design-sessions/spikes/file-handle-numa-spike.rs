// Copyright (c) Mike Grier
//! Spike: does a file handle yield a NUMA node, and which question does the
//! answer actually answer?
//!
//! **NOT YET RUN ON HARDWARE THAT CAN ANSWER IT.** This file is checked in as a
//! ready instrument, not as a result. It needs a machine with more than one
//! NUMA node and storage whose PDO advertises a proximity domain. On a
//! single-node machine both calls are uninformative: failure proves nothing,
//! and success can only ever say `0`.
//!
//! Questions:
//!   Q1 does `FSCTL_QUERY_VOLUME_NUMA_INFO` succeed on a garden-variety NTFS
//!      data file handle, and what node does it name?
//!   Q2 does `GetNumaNodeNumberFromHandle` succeed on the same handle?
//!   Q3 do they agree? If they do, the answer being seen is **volume**
//!      locality, not file locality -- which is the whole point of the spike.
//!   Q4 what exactly does the negative case look like (which error), since the
//!      degradation path has to handle it?
//!   Q5 does a directory handle behave the same as a file handle? The IFS docs
//!      say the FSCTL accepts either.
//!
//! Why it matters: `DESIGN-NOTES.md` asserts that mapping a file handle to the
//! NUMA node of its backing device "has no clean user-mode path" and "means
//! walking volume to disk to device instance". That is wrong on mechanism --
//! `FSCTL_QUERY_VOLUME_NUMA_INFO` is documented, takes a file or directory
//! handle directly, and returns `FSCTL_QUERY_VOLUME_NUMA_INFO_OUTPUT { ULONG
//! NumaNode }`. What is *right* is the conclusion, for a different reason: the
//! documented meaning is the node the **volume** resides on, not where the
//! file's extents live, and it is absent whenever the device advertised no
//! proximity domain.
//!
//! `GetNumaNodeNumberFromHandle` is the other path: a Win32 wrapper over
//! `NtQueryInformationFile` with `FileNumaNodeInformation` (class 53, Windows 7
//! and later), yielding `FILE_NUMA_NODE_INFORMATION { USHORT NodeNumber }`.
//! PHNT and the WDK mark that class **reserved for system use**, so this spike
//! measures it for comparison only. Do not build on it.
//!
//! Run with:
//! ```toml
//! [dependencies]
//! windows-sys = { version = "0.61.2", default-features = false, features = [
//!     "Win32_Foundation", "Win32_Security", "Win32_Storage_FileSystem",
//!     "Win32_System_IO", "Win32_System_Ioctl",
//! ] }
//! ```

use std::ffi::c_void;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::FSCTL_QUERY_VOLUME_NUMA_INFO;

fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

// Not present in windows-sys 0.61, so declared here.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetNumaNodeNumberFromHandle(hFile: HANDLE, NodeNumber: *mut u16) -> i32;
}

fn probe(label: &str, handle: HANDLE) -> (Option<u32>, Option<u16>) {
    println!("\n-- {label} --");

    // Q1/Q4: the documented path. Volume node, via a file or directory handle.
    let mut out: u32 = u32::MAX;
    let mut returned: u32 = 0;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_QUERY_VOLUME_NUMA_INFO,
            std::ptr::null(),
            0,
            (&raw mut out).cast::<c_void>(),
            u32::try_from(size_of::<u32>()).unwrap(),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    let volume = if ok != 0 {
        println!("  FSCTL_QUERY_VOLUME_NUMA_INFO : ok, NumaNode = {out} ({returned} bytes)");
        Some(out)
    } else {
        let e = std::io::Error::last_os_error();
        println!(
            "  FSCTL_QUERY_VOLUME_NUMA_INFO : FAILED, {} (raw {:?})",
            e,
            e.raw_os_error()
        );
        None
    };

    // Q2/Q4: the reserved-for-system-use path, for comparison only.
    let mut node: u16 = u16::MAX;
    let ok = unsafe { GetNumaNodeNumberFromHandle(handle, &raw mut node) };
    let file = if ok != 0 {
        println!("  GetNumaNodeNumberFromHandle  : ok, NodeNumber = {node}");
        Some(node)
    } else {
        let e = std::io::Error::last_os_error();
        println!(
            "  GetNumaNodeNumberFromHandle  : FAILED, {} (raw {:?})",
            e,
            e.raw_os_error()
        );
        None
    };

    // Q3: the discriminating comparison. Agreement means volume locality is
    // what is being observed, and that there is no per-file answer here.
    match (volume, file) {
        (Some(v), Some(f)) if u32::from(f) == v => {
            println!("  => AGREE on {v}: this is VOLUME locality, not file locality.");
        }
        (Some(v), Some(f)) => {
            println!("  => DISAGREE (volume {v}, handle {f}) -- interesting, investigate.");
        }
        (None, None) => println!("  => neither reports a node: no association for this volume."),
        _ => println!("  => only one path answered; record which."),
    }

    (volume, file)
}

fn main() -> std::io::Result<()> {
    println!("NOTE: on a single-NUMA-node machine this spike is VACUOUS.");
    println!("Check the node count first; if it is 1, these results say nothing.\n");

    let path = std::env::temp_dir().join("numa-probe-target.bin");
    fs::write(&path, vec![0_u8; 4096])?;

    // Q1-Q4: a garden-variety data file, opened the ordinary way. This is the
    // case for which no published measurement could be found.
    let file = fs::File::open(&path)?;
    probe(
        &format!("regular NTFS data file: {}", path.display()),
        file.as_raw_handle() as HANDLE,
    );

    // Q5: a directory handle on the same volume. The IFS docs say the FSCTL
    // accepts a file *or* directory, so this should match the file above.
    //
    // A directory needs `FILE_FLAG_BACKUP_SEMANTICS`; plain `File::open` fails
    // with ERROR_PATH_NOT_FOUND. The first version of this spike used
    // `File::open` and could never have answered Q5 -- caught by running it on
    // hardware where the rest of the spike is vacuous, which is a decent
    // argument for smoke-running an instrument even when its result cannot be.
    let dir = to_wide(&std::env::temp_dir());
    let handle = unsafe {
        CreateFileW(
            dir.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        println!(
            "\n-- directory handle -- CreateFileW failed: {}",
            std::io::Error::last_os_error()
        );
    } else {
        probe("directory handle (temp dir)", handle);
        unsafe { CloseHandle(handle) };
    }

    drop(file);
    let _ = fs::remove_file(&path);
    Ok(())
}
