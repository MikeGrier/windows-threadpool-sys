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

/// Run `script` through `diskpart`, and give what it printed, or fail with it if it reported an error.
fn run_diskpart(script: &str, tag: &str) -> io::Result<String> {
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
        Ok(printed.into_owned())
    } else {
        Err(io::Error::other(format!(
            "diskpart ({}): {printed}",
            output.status
        )))
    }
}

/// Whether anything is mounted at `letter`. A raw volume has no file system, so `X:\` does not
/// exist for it, and only opening the volume itself shows it is there.
fn letter_is_taken(letter: char) -> bool {
    Path::new(&format!("{letter}:\\")).exists()
        || OpenOptions::new()
            .access_mode(0)
            .open(format!(r"\\.\{letter}:"))
            .is_ok()
}

/// A drive letter nothing is mounted at, from the end of the alphabet.
fn free_letter() -> io::Result<char> {
    ('D'..='Z')
        .rev()
        .find(|letter| !letter_is_taken(*letter))
        .ok_or_else(|| io::Error::other("no free drive letter"))
}

/// The temporary directory in the long form the system names it by, which is the form `diskpart`
/// prints back for a file made in it. `TEMP` can hold a short (8.3) form, such as `RUNNER~1`, and
/// `diskpart` prints a name as it was given.
pub(crate) fn scratch_directory() -> PathBuf {
    let given = std::env::temp_dir();
    match std::fs::canonicalize(&given) {
        Ok(canonical) => {
            let text = canonical.display().to_string();
            match text.strip_prefix(r"\\?\") {
                // Only a drive path loses its prefix; `\\?\UNC\...` is not one.
                Some(plain) if plain.as_bytes().get(1) == Some(&b':') => PathBuf::from(plain),
                _ => canonical,
            }
        }
        Err(_) => given,
    }
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
        let vhd = scratch_directory().join(format!(
            "dioring-scratch-{}-{}.vhdx",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&vhd);
        if let Err(error) = run_diskpart(&create_script(&vhd, megabytes, letter), "create") {
            // Whatever was attached before the failure is detached, and the file removed.
            let _ = run_diskpart(&detach_script(&vhd), "detach");
            let _ = std::fs::remove_file(&vhd);
            return Err(error);
        }
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

/// The `diskpart` script that creates a virtual disk of `megabytes` at `vhd`, attaches it with no
/// partition on it, and prints what it is, so the number of the disk it became can be read.
pub(crate) fn create_disk_script(vhd: &Path, megabytes: u32) -> String {
    format!(
        "create vdisk file=\"{vhd}\" maximum={megabytes} type=expandable\n\
         select vdisk file=\"{vhd}\"\n\
         attach vdisk\n\
         detail vdisk\n",
        vhd = vhd.display()
    )
}

/// The `diskpart` script that brings disk `disk_number` online and writable, as a new virtual
/// disk may not be.
pub(crate) fn online_script(disk_number: u32) -> String {
    format!(
        "select disk {disk_number}\n\
         online disk noerr\n\
         attributes disk clear readonly noerr\n"
    )
}

/// What `detail vdisk` printed for `vhd`, read for the number of the disk it is attached as.
///
/// This is the only way a number reaches a test that writes to a disk, so it is strict: the output
/// must name the very file the test created, once, and exactly one associated disk, which cannot be
/// disk 0, the one a machine starts from. Anything else is an error and nothing is written.
pub(crate) fn associated_disk(printed: &str, vhd: &Path) -> Result<u32, String> {
    let value = |label: &str| -> Vec<&str> {
        printed
            .lines()
            .filter_map(|line| line.trim().strip_prefix(label))
            .map(str::trim)
            .collect()
    };
    let names = value("Filename:");
    let [name] = names.as_slice() else {
        return Err(format!("expected one Filename, found {names:?}"));
    };
    if !name.eq_ignore_ascii_case(&vhd.display().to_string()) {
        return Err(format!(
            "diskpart describes {name:?}, not the disk made at {}",
            vhd.display()
        ));
    }
    let disks = value("Associated disk#:");
    let [disk] = disks.as_slice() else {
        return Err(format!("expected one associated disk, found {disks:?}"));
    };
    match disk.parse::<u32>() {
        Ok(0) => Err("the associated disk is disk 0, which is never a scratch disk".to_owned()),
        Ok(number) => Ok(number),
        Err(error) => Err(format!(
            "the associated disk {disk:?} is not a number: {error}"
        )),
    }
}

/// A virtual disk attached with no partition and no volume, and the number it was given. Writing
/// to it needs an administrator, as for [`ScratchVolume`]; it is detached and removed when it
/// drops.
pub(crate) struct ScratchDisk {
    vhd: PathBuf,
    disk_number: u32,
    megabytes: u32,
}

impl ScratchDisk {
    /// A disk of `megabytes`, attached, online and writable.
    pub(crate) fn create(megabytes: u32) -> io::Result<Self> {
        let _creating = CREATING.lock().unwrap_or_else(PoisonError::into_inner);
        let vhd = scratch_directory().join(format!(
            "dioring-scratch-disk-{}-{}.vhdx",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_file(&vhd);
        // Made in the long form of the temporary directory, and then named in full by the system,
        // so the name compared below is the long one diskpart prints and not a short form.
        let printed = run_diskpart(&create_disk_script(&vhd, megabytes), "create-disk");
        let named = std::fs::canonicalize(&vhd).map(|path| {
            path.display()
                .to_string()
                .trim_start_matches(r"\\?\")
                .to_owned()
        });
        let disk = match (&printed, named) {
            (Ok(printed), Ok(named)) => associated_disk(printed, Path::new(&named))
                .map_err(|reason| format!("{reason}\n{printed}")),
            (Err(error), _) => Err(error.to_string()),
            (_, Err(error)) => Err(format!("the disk file was not made: {error}")),
        };
        let disk_number = match disk {
            Ok(number) => number,
            Err(reason) => {
                // Whatever was attached is detached, and the file removed, before giving up.
                let _ = run_diskpart(&detach_script(&vhd), "detach");
                let _ = std::fs::remove_file(&vhd);
                return Err(io::Error::other(reason));
            }
        };
        let scratch = Self {
            vhd,
            disk_number,
            megabytes,
        };
        run_diskpart(&online_script(disk_number), "online")?;
        Ok(scratch)
    }

    /// The disk's number.
    pub(crate) fn disk_number(&self) -> u32 {
        self.disk_number
    }

    /// The disk's size in bytes, as it was asked for.
    pub(crate) fn bytes(&self) -> u64 {
        u64::from(self.megabytes) * 1024 * 1024
    }
}

impl Drop for ScratchDisk {
    fn drop(&mut self) {
        let _ = run_diskpart(&detach_script(&self.vhd), "detach");
        let _ = std::fs::remove_file(&self.vhd);
    }
}

#[cfg(test)]
mod tests;
