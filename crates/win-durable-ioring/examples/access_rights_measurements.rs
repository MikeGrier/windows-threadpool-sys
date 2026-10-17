// Copyright (c) 2026 Mike Grier

//! Measures what the kernel allows on a handle opened with each data access and other rights,
//! through the calls a file target and the consumer that owns it make on the same handle: the ring's
//! read, write and commit (a commit is `FlushFileBuffers`, which the documentation says needs
//! `GENERIC_WRITE`), the size query, and the length, time and flush calls a database makes. It prints
//! a table and asserts nothing; the measurements are what `DI-D-65` rests on.
//!
//! Every call is a safe `std` call on a duplicate of the handle the open produced, so the handle is
//! judged as the kernel enforces it. Run: `cargo build --example access_rights_measurements`, then the
//! executable under `target/debug/examples`.

use std::fs::{self, File, FileTimes};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::windows::io::AsHandle;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use io_buffers::AlignedBuf;
use win_durable_ioring::windows_backend::{
    Buffering, Cached, DataAccess, FileOptions, FileRights, OpenedFile, Unbuffered,
};

/// One sector of the size an unbuffered transfer needs, and the length the calls below extend to.
const SECTOR: usize = 4096;
const EXTENDED: u64 = 3 * SECTOR as u64;

/// Opens the scratch file with one row's options and gives a duplicate of the handle.
type Opener = Box<dyn Fn(&Path) -> io::Result<File>>;

fn opener<B: Buffering, A: DataAccess>(options: FileOptions<A>) -> Opener {
    Box::new(move |path| {
        let opened = OpenedFile::<B, A>::open(path, options)?;
        Ok(File::from(opened.as_handle().try_clone_to_owned()?))
    })
}

/// The rows: a label and what opens the file for it, in buffering mode `B`.
fn rows<B: Buffering>() -> Vec<(&'static str, Opener)> {
    let attributes = FileRights::READ_ATTRIBUTES;
    let write_attributes = FileRights::WRITE_ATTRIBUTES;
    vec![
        ("NoData", opener::<B, _>(FileOptions::new())),
        ("ReadOnly", opener::<B, _>(FileOptions::new().read())),
        ("WriteOnly", opener::<B, _>(FileOptions::new().write())),
        (
            "ReadWrite",
            opener::<B, _>(FileOptions::new().read().write()),
        ),
        (
            "ReadOnly + READ_ATTRIBUTES",
            opener::<B, _>(FileOptions::new().read().rights(attributes)),
        ),
        (
            "WriteOnly + READ_ATTRIBUTES",
            opener::<B, _>(FileOptions::new().write().rights(attributes)),
        ),
        (
            "WriteOnly + WRITE_ATTRIBUTES",
            opener::<B, _>(FileOptions::new().write().rights(write_attributes)),
        ),
        (
            "ReadWrite + WRITE_ATTRIBUTES",
            opener::<B, _>(FileOptions::new().read().write().rights(write_attributes)),
        ),
    ]
}

fn outcome<T>(result: io::Result<T>) -> String {
    match result {
        Ok(_) => "ok".to_owned(),
        Err(error) => error
            .raw_os_error()
            .map_or_else(|| format!("{:?}", error.kind()), |code| format!("e{code}")),
    }
}

fn read_sector(file: &mut File) -> io::Result<()> {
    let mut buffer = AlignedBuf::zeroed(SECTOR, SECTOR).expect("an aligned buffer");
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(buffer.as_mut_slice())
}

fn write_sector(file: &mut File) -> io::Result<()> {
    let buffer = AlignedBuf::from_bytes(&[0x5a; SECTOR], SECTOR).expect("an aligned buffer");
    file.seek(SeekFrom::Start(0))?;
    file.write_all(buffer.as_slice())
}

/// The calls, in the order a row reports them. Each takes the handle as the open produced it.
fn calls(file: &mut File) -> Vec<(&'static str, String)> {
    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
    vec![
        ("read", outcome(read_sector(file))),
        ("write", outcome(write_sector(file))),
        (
            "size",
            outcome(file.metadata().map(|metadata| metadata.len())),
        ),
        ("extend", outcome(file.set_len(EXTENDED))),
        ("sync_all", outcome(file.sync_all())),
        ("sync_data", outcome(file.sync_data())),
        (
            "set_times",
            outcome(file.set_times(FileTimes::new().set_modified(modified))),
        ),
    ]
}

fn scratch(tag: &str, serial: u32) -> PathBuf {
    std::env::temp_dir().join(format!(
        "access-measure-{}-{tag}-{serial}",
        std::process::id()
    ))
}

/// The one place anything is printed.
fn emit(out: &mut impl Write, line: &str) {
    writeln!(out, "{line}").expect("write to the output");
}

fn measure<B: Buffering>(out: &mut impl Write, mode: &str, serial: &mut u32) {
    emit(out, &format!("== {mode}"));
    for (label, open) in rows::<B>() {
        *serial += 1;
        let path = scratch(mode, *serial);
        fs::write(&path, vec![0u8; 2 * SECTOR]).expect("fill the scratch file");
        let line = match open(&path) {
            Ok(mut file) => calls(&mut file)
                .into_iter()
                .map(|(name, result)| format!("{name}={result}"))
                .collect::<Vec<_>>()
                .join(" "),
            Err(error) => format!("open refused: {}", outcome(Err::<(), _>(error))),
        };
        emit(out, &format!("{label}: {line}"));
        fs::remove_file(&path).expect("remove the scratch file");
    }
}

fn main() {
    let mut out = io::stdout().lock();
    let mut serial = 0;
    measure::<Cached>(&mut out, "cached", &mut serial);
    measure::<Unbuffered>(&mut out, "unbuffered", &mut serial);
}
