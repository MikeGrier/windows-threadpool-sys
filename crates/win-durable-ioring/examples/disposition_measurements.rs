// Copyright (c) 2026 Mike Grier

//! Measures which access masks the kernel accepts with each creation disposition that overwrites,
//! through the request crate's `OpenFile`, which is `CreateFileW` with nothing added. `DI-13.7` has to
//! offer the five dispositions, and `TRUNCATE_EXISTING` is documented as needing `GENERIC_WRITE`, which
//! the typed write access does not ask for; this finds out what it does need. It prints a table and
//! asserts nothing.
//!
//! Run: `cargo build --example disposition_measurements`, then the executable under
//! `target/debug/examples`.

use std::fs;
use std::path::Path;

use windows_namespace_request_sys::open::OpenFile;
use windows_sys::Win32::Foundation::GENERIC_WRITE;
use windows_sys::Win32::Storage::FileSystem::{
    CREATE_ALWAYS, CREATE_NEW, DELETE, FILE_APPEND_DATA, FILE_FLAG_NO_BUFFERING, FILE_READ_DATA,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA,
    FILE_WRITE_EA, OPEN_ALWAYS, OPEN_EXISTING, TRUNCATE_EXISTING,
};
use wtf_string::Wtf16String;

const CONTENT: &[u8] = b"not empty";

/// The access masks tried, as the typed accesses and the documented generic right would ask.
fn masks() -> Vec<(&'static str, u32)> {
    vec![
        ("FILE_WRITE_DATA (WriteOnly)", FILE_WRITE_DATA),
        (
            "FILE_READ_DATA | FILE_WRITE_DATA (ReadWrite)",
            FILE_READ_DATA | FILE_WRITE_DATA,
        ),
        (
            "FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES",
            FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES,
        ),
        (
            "FILE_WRITE_DATA | FILE_WRITE_EA",
            FILE_WRITE_DATA | FILE_WRITE_EA,
        ),
        (
            "FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA",
            FILE_WRITE_DATA | FILE_WRITE_ATTRIBUTES | FILE_WRITE_EA,
        ),
        ("FILE_WRITE_DATA | DELETE", FILE_WRITE_DATA | DELETE),
        (
            "FILE_WRITE_DATA | FILE_APPEND_DATA",
            FILE_WRITE_DATA | FILE_APPEND_DATA,
        ),
        ("GENERIC_WRITE", GENERIC_WRITE),
        ("FILE_READ_DATA (ReadOnly)", FILE_READ_DATA),
        ("0 (NoData)", 0),
    ]
}

const DISPOSITIONS: [(&str, u32); 5] = [
    ("CREATE_NEW", CREATE_NEW),
    ("CREATE_ALWAYS", CREATE_ALWAYS),
    ("OPEN_EXISTING", OPEN_EXISTING),
    ("OPEN_ALWAYS", OPEN_ALWAYS),
    ("TRUNCATE_EXISTING", TRUNCATE_EXISTING),
];

/// What one open does: the Win32 code it fails with, or whether it left the file empty.
fn attempt(path: &Path, access: u32, disposition: u32, flags: u32) -> String {
    let request = OpenFile::new(Wtf16String::from_os_str(path.as_os_str()))
        .expect("a temporary path")
        .with_desired_access(access)
        .with_share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .with_creation_disposition(disposition)
        .with_flags_and_attributes(flags);
    match request.perform() {
        Ok(handle) => {
            drop(handle);
            let length = fs::metadata(path).map_or(u64::MAX, |metadata| metadata.len());
            if length == 0 {
                "ok, emptied".to_owned()
            } else {
                "ok".to_owned()
            }
        }
        Err(error) => format!("error {}", error.code()),
    }
}

fn run(title: &str, flags: u32) {
    println!("\n{title}\n");
    print!("{:<58}", "access");
    for (name, _) in DISPOSITIONS {
        print!(" | {name:<17}");
    }
    println!();
    for (label, access) in masks() {
        print!("{label:<58}");
        for (_, disposition) in DISPOSITIONS {
            let path = std::env::temp_dir().join(format!("disposition-{}.tmp", std::process::id()));
            // CREATE_NEW needs the file to be absent; every other row needs it present.
            let _ = fs::remove_file(&path);
            if disposition != CREATE_NEW {
                fs::write(&path, CONTENT).expect("fill the scratch file");
            }
            let outcome = attempt(&path, access, disposition, flags);
            print!(" | {outcome:<17}");
            let _ = fs::remove_file(&path);
        }
        println!();
    }
}

fn main() {
    run("cached", 0);
    run(
        "unbuffered (FILE_FLAG_NO_BUFFERING)",
        FILE_FLAG_NO_BUFFERING,
    );
}
