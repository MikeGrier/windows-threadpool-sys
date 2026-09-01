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
//!   Q6 **what does a Storage Space report?** Run this against a file on a
//!      striped or parity space whose columns are NVMe devices on different
//!      PCIe roots, ideally attached to different nodes. Three outcomes, and
//!      they are not equally good:
//!        - no answer: honest, and the consumer degrades to "any domain";
//!        - one node that genuinely matches every column: useful;
//!        - **one node for a device set that spans several: a fiction, and
//!          worse than no answer**, because a consumer would act on it.
//!      The third is the case worth knowing about, and it cannot be
//!      distinguished from the second without independently knowing where the
//!      columns live -- so record the space's layout (`Get-StoragePool`,
//!      `Get-PhysicalDisk`) alongside whatever this prints, or the result
//!      cannot be interpreted.
//!   Q7 **does a thread created with `PROC_THREAD_ATTRIBUTE_GROUP_AFFINITY`
//!      get a node-local stack?** Binding a thread's affinity *after* it starts
//!      cannot move its stack, which was allocated at creation on the creating
//!      thread's node -- so a domain runtime must construct threads with the
//!      affinity already set, and this asks whether the kernel then honours it
//!      for the stack allocation. Measured with `QueryWorkingSetEx`, whose
//!      `PSAPI_WORKING_SET_EX_BLOCK` carries a `Node` field: take the address
//!      of a local in the new thread and ask which node its page is on.
//!      Compare a thread created with an affinity attribute for a *remote*
//!      node against one created with none. If the stacks report different
//!      nodes, creation-time affinity governs stack placement and the builder
//!      is justified; if they match, it does not and the design should stop
//!      claiming it does.
//!      **This question is not implemented below**, because it needs
//!      `CreateRemoteThreadEx` and an attribute list rather than a file handle.
//!      It now has its own instrument: `thread-stack-numa-spike.rs`.
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
//! # The storage topology is captured too, and it is useful even at one node
//!
//! Whether the node query *succeeds at all* is itself a finding, independent of
//! how many nodes exist. On the ARM64 development machine both calls succeed
//! and report `0` -- so "an ordinary NTFS file" is not the no-association case.
//! If the same calls **fail** on some other host, the association depends on
//! the storage stack rather than on node count, and the interesting question
//! becomes *which* stacks have it.
//!
//! So the spike also records what the volume is made of:
//!
//!   - the bus type and product identity (`IOCTL_STORAGE_QUERY_PROPERTY`),
//!     which distinguishes real NVMe from a virtual disk -- and hosted CI
//!     runners are virtual;
//!   - the physical disk number (`IOCTL_STORAGE_GET_DEVICE_NUMBER`);
//!   - **how many disks back the volume**
//!     (`IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`). More than one means the volume
//!     spans devices, which is exactly the Q6 hazard: a single reported node
//!     for a multi-device volume is a fiction, and worse than no answer.
//!
//! That last one needs no NUMA hardware, so a spanned volume anywhere in a CI
//! fleet is a result.
//!
//! # Machine-readable output
//!
//! The final `x-spike-file-handle-numa` line is a single JSON object, so
//! accumulated build logs can be mined mechanically instead of read.
//!
//! Run with:
//! ```toml
//! [dependencies]
//! windows-sys = { version = "0.61.2", default-features = false, features = [
//!     "Win32_Foundation", "Win32_Security", "Win32_Storage_FileSystem",
//!     "Win32_System_IO", "Win32_System_Ioctl",
//! ] }
//! ```

use std::ffi::{OsStr, c_void};
use std::fs;
use std::io::Write as _;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GetVolumePathNameW, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Ioctl::{
    FSCTL_QUERY_VOLUME_NUMA_INFO, IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY,
    PropertyStandardQuery, STORAGE_DEVICE_DESCRIPTOR, STORAGE_DEVICE_NUMBER,
    STORAGE_PROPERTY_QUERY, StorageDeviceProperty,
};

fn to_wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// What the volume under a path is physically made of.
///
/// Every field is optional because every query can fail, and a failure is a
/// result here rather than an error: it says this host does not expose that
/// fact, which is precisely what varies across a runner fleet.
#[derive(Default)]
struct Storage {
    volume_root: Option<String>,
    bus_type: Option<u8>,
    product: Option<String>,
    removable: Option<bool>,
    disk_number: Option<u32>,
    /// Disks backing the volume. Greater than one means it spans devices, and a
    /// single NUMA node reported for it cannot be true of all of them.
    disk_extents: Option<u32>,
}

/// `STORAGE_BUS_TYPE` values worth naming. A virtual bus is the tell for a
/// hosted runner; NVMe and SAS are the cases where device proximity data
/// plausibly exists.
fn bus_name(bus: u8) -> &'static str {
    match bus {
        0x01 => "SCSI",
        0x02 => "ATAPI",
        0x03 => "ATA",
        0x04 => "1394",
        0x05 => "SSA",
        0x06 => "Fibre",
        0x07 => "USB",
        0x08 => "RAID",
        0x09 => "iSCSI",
        0x0A => "SAS",
        0x0B => "SATA",
        0x0C => "SD",
        0x0D => "MMC",
        0x0E => "Virtual",
        0x0F => "FileBackedVirtual",
        0x10 => "Spaces",
        0x11 => "NVMe",
        0x12 => "SCM",
        0x13 => "UFS",
        _ => "unknown",
    }
}

fn open_volume(root: &str) -> HANDLE {
    // `\\.\C:` form: strip the trailing separator the volume-path API returns.
    let device = format!(r"\\.\{}", root.trim_end_matches('\\'));
    let wide: Vec<u16> = OsStr::new(&device).encode_wide().chain(Some(0)).collect();
    // Zero desired access is enough for query-only IOCTLs and, unlike
    // GENERIC_READ, does not require elevation -- which matters because this is
    // meant to run unprivileged in CI.
    unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    }
}

fn describe_storage(path: &Path) -> Storage {
    let mut out = Storage::default();

    // Which volume is this path on?
    let wide = to_wide(path);
    let mut root = vec![0_u16; 260];
    let ok = unsafe {
        GetVolumePathNameW(
            wide.as_ptr(),
            root.as_mut_ptr(),
            u32::try_from(root.len()).unwrap(),
        )
    };
    if ok == 0 {
        return out;
    }
    let len = root.iter().position(|&c| c == 0).unwrap_or(root.len());
    out.volume_root = Some(String::from_utf16_lossy(&root[..len]));

    let Some(volume_root) = out.volume_root.clone() else {
        return out;
    };
    let handle = open_volume(&volume_root);
    if handle == INVALID_HANDLE_VALUE {
        return out;
    }

    let mut returned: u32 = 0;

    // Bus type and product identity: distinguishes real NVMe from a virtual
    // disk, which is the correlation worth having when the node query fails.
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let mut buf = vec![0_u8; 1024];
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast::<c_void>(),
            u32::try_from(size_of::<STORAGE_PROPERTY_QUERY>()).unwrap(),
            buf.as_mut_ptr().cast::<c_void>(),
            u32::try_from(buf.len()).unwrap(),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 && (returned as usize) >= size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        // SAFETY: the driver filled at least a descriptor's worth of `buf`.
        //
        // Read out, not borrowed in place. `buf` is a `Vec<u8>`, which promises
        // only byte alignment, so forming a `&STORAGE_DEVICE_DESCRIPTOR` into it
        // is undefined behaviour no matter how many bytes are there. That the
        // system allocator happens to hand back suitably aligned blocks today is
        // exactly the kind of incidental behaviour not to build on.
        let desc = unsafe { buf.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>().read_unaligned() };
        out.bus_type = Some(desc.BusType as u8);
        out.removable = Some(desc.RemovableMedia);
        // The ID offsets are byte offsets into the same buffer, or 0 for absent.
        let text_at = |offset: u32| -> Option<String> {
            if offset == 0 || offset as usize >= buf.len() {
                return None;
            }
            let start = offset as usize;
            let end = buf[start..]
                .iter()
                .position(|&b| b == 0)
                .map_or(buf.len(), |n| start + n);
            let s = String::from_utf8_lossy(&buf[start..end]).trim().to_string();
            (!s.is_empty()).then_some(s)
        };
        let vendor = text_at(desc.VendorIdOffset);
        let product = text_at(desc.ProductIdOffset);
        out.product = match (vendor, product) {
            (Some(v), Some(p)) => Some(format!("{v} {p}")),
            (Some(v), None) => Some(v),
            (None, Some(p)) => Some(p),
            (None, None) => None,
        };
    }

    // Which physical disk.
    let mut number = STORAGE_DEVICE_NUMBER::default();
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            std::ptr::null(),
            0,
            (&raw mut number).cast::<c_void>(),
            u32::try_from(size_of::<STORAGE_DEVICE_NUMBER>()).unwrap(),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        out.disk_number = Some(number.DeviceNumber);
    }

    // How many disks back this volume. This is the Q6 question, and it needs no
    // NUMA hardware: more than one extent means a reported node cannot be true
    // of every device the volume sits on.
    let mut extents = vec![0_u8; 4096];
    let ok = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
            std::ptr::null(),
            0,
            extents.as_mut_ptr().cast::<c_void>(),
            u32::try_from(extents.len()).unwrap(),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 && (returned as usize) >= size_of::<u32>() {
        // SAFETY: the first field of VOLUME_DISK_EXTENTS is NumberOfDiskExtents.
        // Unaligned for the same reason as the descriptor above: `extents` is a
        // `Vec<u8>` and carries no alignment guarantee beyond one byte.
        let count = unsafe { extents.as_ptr().cast::<u32>().read_unaligned() };
        out.disk_extents = Some(count);
    }

    unsafe { CloseHandle(handle) };
    out
}

// Not present in windows-sys 0.61, so declared here.
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetNumaNodeNumberFromHandle(hFile: HANDLE, NodeNumber: *mut u16) -> i32;
    fn GetNumaHighestNodeNumber(HighestNodeNumber: *mut u32) -> i32;
}

/// How many NUMA nodes this machine reports.
///
/// One on failure, which is the conservative answer: it makes the spike call
/// itself vacuous rather than claim a result it cannot support.
fn numa_node_count() -> u32 {
    let mut highest = 0_u32;
    // SAFETY: `highest` is a live local for the duration of the call.
    if unsafe { GetNumaHighestNodeNumber(&raw mut highest) } != 0 {
        highest.saturating_add(1)
    } else {
        1
    }
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
    // **The token is a verdict, not a disclaimer, and must be earned.**
    // `tools/run-numa-spikes.ps1` classifies a spike by searching its output for
    // `VACUOUS`, so printing it unconditionally -- as this did, before any node
    // was queried -- marked every run vacuous and made the one signal the CI job
    // exists to raise unreachable for this spike. The other spikes gate theirs
    // on a measured node count; this one now does too.
    let nodes = numa_node_count();
    if nodes <= 1 {
        println!("NOTE: this machine reports one NUMA node, so this spike is VACUOUS.");
        println!("The apparatus below still runs, but its results say nothing.
");
    } else {
        println!("this machine reports {nodes} NUMA nodes, so the results below are real.
");
    }

    // Q6: pass a directory on a Storage Space as argv[1] to ask the harder
    // question. Default target is the temp directory, i.e. the boot volume.
    let dir = match std::env::args().nth(1) {
        Some(arg) => {
            println!("target directory overridden: {arg}");
            println!("for Q6, record the space's layout (Get-StoragePool,");
            println!("Get-PhysicalDisk) alongside this output, or the result");
            println!("cannot be interpreted.\n");
            std::path::PathBuf::from(arg)
        }
        None => std::env::temp_dir(),
    };
    // **Per-process, and created exclusively.** The old fixed name was written
    // with truncation and deleted at the end, so a second copy of this spike --
    // or any unrelated file that happened to sit at that path -- was destroyed
    // and then removed. `create_new` refuses to open anything that already
    // exists, so the probe can only ever delete a file it made itself.
    let path = dir.join(format!("numa-probe-target-{}.bin", std::process::id()));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&[0_u8; 4096])?;
    }

    // What the volume is physically made of. Printed before the node queries so
    // that a reader has the context to interpret a failure: a virtual disk
    // failing to report a node means something different from an NVMe failing.
    let storage = describe_storage(&path);
    println!("-- storage under {} --", path.display());
    println!(
        "  volume root  : {}",
        storage.volume_root.as_deref().unwrap_or("(unknown)")
    );
    match storage.bus_type {
        Some(bus) => println!("  bus type     : {bus} ({})", bus_name(bus)),
        None => println!("  bus type     : (query failed)"),
    }
    println!(
        "  product      : {}",
        storage.product.as_deref().unwrap_or("(unknown)")
    );
    match storage.disk_number {
        Some(n) => println!("  disk number  : {n}"),
        None => println!("  disk number  : (query failed)"),
    }
    match storage.disk_extents {
        Some(1) => println!("  disk extents : 1 (single device)"),
        Some(n) => println!(
            "  disk extents : {n} -- THIS VOLUME SPANS {n} DEVICES, so any single \
             node reported for it cannot be true of all of them"
        ),
        None => println!("  disk extents : (query failed)"),
    }

    // Q1-Q4: a garden-variety data file, opened the ordinary way. This is the
    // case for which no published measurement could be found.
    let file = fs::File::open(&path)?;
    let (file_volume_node, file_handle_node) = probe(
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
    let dir_wide = to_wide(&dir);
    let handle = unsafe {
        CreateFileW(
            dir_wide.as_ptr(),
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
        probe(&format!("directory handle: {}", dir.display()), handle);
        unsafe { CloseHandle(handle) };
    }

    // One machine-readable line, so accumulated CI logs can be mined without
    // parsing the prose above.
    let json_opt_u32 = |v: Option<u32>| v.map_or("null".to_string(), |n| n.to_string());
    let json_opt_str = |v: Option<&str>| {
        v.map_or("null".to_string(), |s| {
            format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
        })
    };
    println!(
        concat!(
            r#"{{"reason":"x-spike-file-handle-numa","arch":"{}","volume_root":{},"#,
            r#""bus_type":{},"bus_name":{},"product":{},"removable":{},"disk_number":{},"#,
            r#""disk_extents":{},"spans_devices":{},"fsctl_volume_node":{},"#,
            r#""handle_node":{},"both_succeeded":{}}}"#
        ),
        std::env::consts::ARCH,
        json_opt_str(storage.volume_root.as_deref()),
        json_opt_u32(storage.bus_type.map(u32::from)),
        json_opt_str(storage.bus_type.map(bus_name)),
        json_opt_str(storage.product.as_deref()),
        storage
            .removable
            .map_or("null".to_string(), |b| b.to_string()),
        json_opt_u32(storage.disk_number),
        json_opt_u32(storage.disk_extents),
        storage
            .disk_extents
            .map_or("null".to_string(), |n| (n > 1).to_string()),
        json_opt_u32(file_volume_node),
        json_opt_u32(file_handle_node.map(u32::from)),
        file_volume_node.is_some() && file_handle_node.is_some(),
    );

    drop(file);
    let _ = fs::remove_file(&path);
    Ok(())
}
