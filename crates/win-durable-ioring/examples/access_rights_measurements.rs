// Copyright (c) 2026 Mike Grier

//! Measures what the kernel allows on a handle opened with each set of rights, through the calls a
//! file target and the consumer that owns it make on the same handle: the ring's read, write and
//! commit (a commit is `FlushFileBuffers`, which the documentation says needs `GENERIC_WRITE`), the
//! size query, and the length, time and flush calls a database makes. It prints a table and asserts
//! nothing; the measurements are for whoever decides how access is typed (`DI-13.3`, `DI-13.7`).
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
    Cached, FileOptions, FileRights, OpenedFile, Unbuffered,
};

/// One sector of the size an unbuffered transfer needs, and the length the calls below extend to.
const SECTOR: usize = 4096;
const EXTENDED: u64 = 3 * SECTOR as u64;

type Opener = fn(&Path, FileOptions) -> io::Result<File>;

fn duplicate<T: AsHandle>(opened: &T) -> io::Result<File> {
    Ok(File::from(opened.as_handle().try_clone_to_owned()?))
}

fn open_cached(path: &Path, options: FileOptions) -> io::Result<File> {
    duplicate(&OpenedFile::<Cached>::open(path, options)?)
}

fn open_unbuffered(path: &Path, options: FileOptions) -> io::Result<File> {
    duplicate(&OpenedFile::<Unbuffered>::open(path, options)?)
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

fn sets() -> Vec<FileRights> {
    vec![
        FileRights::READ_DATA,
        FileRights::WRITE_DATA,
        FileRights::READ_DATA | FileRights::WRITE_DATA,
        FileRights::READ_DATA | FileRights::READ_ATTRIBUTES,
        FileRights::WRITE_DATA | FileRights::READ_ATTRIBUTES,
        FileRights::WRITE_DATA | FileRights::WRITE_ATTRIBUTES,
        FileRights::GENERIC_READ,
        FileRights::GENERIC_WRITE,
        FileRights::GENERIC_READ | FileRights::GENERIC_WRITE,
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

fn main() {
    let mut out = io::stdout().lock();
    let modes: [(&str, Opener); 2] = [("cached", open_cached), ("unbuffered", open_unbuffered)];
    let mut serial = 0;
    for (mode, open) in modes {
        emit(&mut out, &format!("== {mode}"));
        for rights in sets() {
            serial += 1;
            let path = scratch(mode, serial);
            fs::write(&path, vec![0u8; 2 * SECTOR]).expect("fill the scratch file");
            let line = match open(&path, FileOptions::new().access(rights)) {
                Ok(mut file) => calls(&mut file)
                    .into_iter()
                    .map(|(name, result)| format!("{name}={result}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                Err(error) => format!("open refused: {}", outcome(Err::<(), _>(error))),
            };
            emit(&mut out, &format!("{rights:?}: {line}"));
            fs::remove_file(&path).expect("remove the scratch file");
        }
    }
}
