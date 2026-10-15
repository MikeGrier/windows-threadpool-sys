// Copyright (c) 2026 Mike Grier
//! A scratch volume for the tests that need a real raw volume: a small virtual disk, created and
//! attached with `diskpart`, holding one partition with no file system and a drive letter.
//!
//! Opening a raw volume for writing, and creating a virtual disk, need an administrator, so the
//! tests that use this are `#[ignore]`d and run with the account elevated. Nothing here is real
//! storage: the disk is a file in the temporary directory, detached and removed when the
//! [`ScratchVolume`] drops. A process killed before that leaves the disk attached; `diskpart`'s
//! `list vdisk` shows it and `detach vdisk` removes it.

use std::fs::OpenOptions;
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::windows_backend::VolumePath;

/// How long a new volume has to appear at its drive letter before the setup is given up on.
const MOUNT_PATIENCE: Duration = Duration::from_secs(30);

/// Numbers the files one process makes, so tests running together do not share one.
static NEXT: AtomicU32 = AtomicU32::new(0);

/// Held from choosing a drive letter until the volume has mounted at it, so two tests cannot choose
/// the same one.
static CREATING: Mutex<()> = Mutex::new(());

/// The text `diskpart` prints when a command in a script failed, which it does not say in its exit
/// code for every kind of failure.
const DISKPART_FAILURE: &str = "DiskPart has encountered an error";

/// The `diskpart` script that creates a virtual disk of `megabytes` at `vhd`, attaches it, and
/// gives it one primary partition at drive letter `letter`.
pub(crate) fn create_script(vhd: &Path, megabytes: u32, letter: char) -> String {
    format!(
        "create vdisk file=\"{vhd}\" maximum={megabytes} type=expandable\n\
         select vdisk file=\"{vhd}\"\n\
         attach vdisk\n\
         create partition primary\n\
         assign letter={letter}\n",
        vhd = vhd.display()
    )
}

/// The `diskpart` script that detaches the virtual disk at `vhd`.
pub(crate) fn detach_script(vhd: &Path) -> String {
    format!(
        "select vdisk file=\"{vhd}\"\ndetach vdisk\n",
        vhd = vhd.display()
    )
}

/// Run `script` through `diskpart`, and fail with what it printed if it reported an error.
fn run_diskpart(script: &str, tag: &str) -> io::Result<()> {
    let path = std::env::temp_dir().join(format!(
        "dioring-diskpart-{tag}-{}-{}.txt",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, script)?;
    let output = Command::new("diskpart").arg("/s").arg(&path).output();
    let _ = std::fs::remove_file(&path);
    let output = output?;
    let printed = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && !printed.contains(DISKPART_FAILURE) {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "diskpart ({}): {printed}",
            output.status
        )))
    }
}

/// A drive letter nothing is mounted at, from the end of the alphabet.
fn free_letter() -> io::Result<char> {
    ('D'..='Z')
        .rev()
        .find(|letter| !Path::new(&format!("{letter}:\\")).exists())
        .ok_or_else(|| io::Error::other("no free drive letter"))
}

/// A virtual disk attached with one raw partition, and the volume path that names it.
pub(crate) struct ScratchVolume {
    vhd: PathBuf,
    letter: char,
}

impl ScratchVolume {
    /// A volume of `megabytes`, mounted and ready to open.
    pub(crate) fn create(megabytes: u32) -> io::Result<Self> {
        let _creating = CREATING.lock().unwrap_or_else(PoisonError::into_inner);
        let letter = free_letter()?;
        let vhd = std::env::temp_dir().join(format!(
            "dioring-scratch-{}-{}.vhdx",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&vhd);
        run_diskpart(&create_script(&vhd, megabytes, letter), "create")?;
        let volume = Self { vhd, letter };
        volume.wait_until_mounted()?;
        Ok(volume)
    }

    /// The path to open the volume at.
    pub(crate) fn path(&self) -> VolumePath {
        VolumePath::drive(self.letter).expect("a drive letter")
    }

    /// The path as a plain string, for a test that opens it some other way.
    pub(crate) fn device_path(&self) -> String {
        self.path().as_str().to_owned()
    }

    /// A volume that has just been assigned a letter takes a moment to answer.
    fn wait_until_mounted(&self) -> io::Result<()> {
        let started = Instant::now();
        loop {
            // Opened with no access, which needs no privilege and is enough to show it is there.
            match OpenOptions::new().access_mode(0).open(self.device_path()) {
                Ok(_) => return Ok(()),
                Err(error) if started.elapsed() >= MOUNT_PATIENCE => {
                    return Err(io::Error::other(format!(
                        "the scratch volume did not mount at {}: {error}",
                        self.device_path()
                    )));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(250)),
            }
        }
    }
}

impl Drop for ScratchVolume {
    fn drop(&mut self) {
        let _ = run_diskpart(&detach_script(&self.vhd), "detach");
        let _ = std::fs::remove_file(&self.vhd);
    }
}

#[cfg(test)]
mod tests;
