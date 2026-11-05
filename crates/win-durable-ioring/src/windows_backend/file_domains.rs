// Copyright (c) 2026 Mike Grier
//! The flush domains of a file this crate opened, found from the opened handle when it is opened
//! (DI-D-117).
//!
//! A local file's domains are the disks of its volume, each the same [`disk_flush_domain`] bytes a
//! raw volume derives, so a file and a volume on one disk share fate by construction. A disk whose
//! bus type is `BusTypeFileBackedVirtual` is followed through the volume's storage dependency to the
//! volume that holds its backing file, and on from there; its own disk is kept as well, since an
//! extra domain is safe.
//!
//! **All or nothing.** Any step that fails -- a handle with no volume GUID, as a remote file's has
//! none; a volume that does not answer the extents control; a disk that does not describe itself; a
//! virtual disk whose host cannot be named -- leaves the file with no domains found. None is unknown,
//! which shares fate with every target, the safe default; a partial set is the one way a declaration
//! makes the contract false, so one is never produced.
//!
//! **A snapshot.** The domains are those of the moment of the open; a volume later extended onto
//! another disk is not seen.
//!
//! The steps are a trait so the rule that walks them can be tested without the devices: no virtual
//! disk can be attached here without an elevated token, so that hop is tested against a stack built
//! in memory, and the real stack through files on this machine's own volumes.

use std::collections::BTreeSet;
use std::io;

use durable_ioring::{FlushDomain, NoData};
use win_shared_os_owned_handle::SharedHandle;
use win_volume_sys::{
    StorageBusType, StorageDependencyFlags, StorageDependencyVersion,
    storage_dependency_information, storage_device_descriptor,
};
use windows_namespace_request_sys::{FinalPathFlags, QueryFinalPath};

use super::opened_file::Cached;
use super::opened_volume::{OpenedVolume, VolumeOptions, VolumePath, disk_flush_domain};
use super::raw_open;
use super::share_mode::ShareMode;

#[cfg(test)]
mod tests;

/// How many volumes one derivation visits before it stops: a virtual disk nested this deep is not
/// one this crate has a reason to expect, and a loop in the dependencies must end.
const MAX_VOLUMES: usize = 16;

/// What the derivation asks of the storage stack.
pub(super) trait StorageStack {
    /// The volume the file `file` is on.
    fn volume_of(&self, file: &SharedHandle) -> io::Result<VolumePath>;
    /// The numbers of the disks the volume `volume` sits on.
    fn disks_of(&self, volume: &VolumePath) -> io::Result<Vec<u32>>;
    /// Whether disk `disk` is backed by a file: its bus type is `BusTypeFileBackedVirtual`.
    fn is_file_backed(&self, disk: u32) -> io::Result<bool>;
    /// The volumes that hold the files backing the virtual disks under `volume`.
    fn hosts_of(&self, volume: &VolumePath) -> io::Result<Vec<VolumePath>>;
}

/// The flush domains of `file`, or the reason none could be found; [`found`] is the all-or-nothing
/// form the open uses.
pub(super) fn derive(
    stack: &impl StorageStack,
    file: &SharedHandle,
) -> io::Result<Vec<FlushDomain>> {
    let mut disks = BTreeSet::new();
    let mut visited = BTreeSet::new();
    let mut pending = vec![stack.volume_of(file)?];
    // Counted as followed, and not as the size of `visited`, so the bound holds by itself.
    let mut followed = 0_usize;
    while let Some(volume) = pending.pop() {
        if !visited.insert(volume.as_str().to_owned()) {
            continue;
        }
        followed += 1;
        if followed > MAX_VOLUMES {
            return Err(io::Error::other(
                "the virtual disks under the file nest deeper than this crate follows",
            ));
        }
        let on = stack.disks_of(&volume)?;
        if on.is_empty() {
            return Err(io::Error::other(format!(
                "the volume {} sits on no disk",
                volume.as_str()
            )));
        }
        let mut backed = false;
        for disk in on {
            disks.insert(disk);
            backed |= stack.is_file_backed(disk)?;
        }
        if backed {
            let hosts = stack.hosts_of(&volume)?;
            if hosts.is_empty() {
                return Err(io::Error::other(format!(
                    "the volume {} is on a virtual disk whose backing file's volume is not named",
                    volume.as_str()
                )));
            }
            pending.extend(hosts);
        }
    }
    Ok(disks.into_iter().map(disk_flush_domain).collect())
}

/// The flush domains of `file`, all or nothing: those [`derive`] finds, or none.
pub(super) fn found(stack: &impl StorageStack, file: &SharedHandle) -> Vec<FlushDomain> {
    derive(stack, file).unwrap_or_default()
}

/// The storage stack of this machine.
pub(super) struct Windows;

impl StorageStack for Windows {
    fn volume_of(&self, file: &SharedHandle) -> io::Result<VolumePath> {
        let path = QueryFinalPath::new(file.clone())
            .with_flags(FinalPathFlags::VOLUME_NAME_GUID)
            .perform()
            .map_err(io::Error::other)?;
        let path = path.to_os_string();
        let path = path
            .to_str()
            .ok_or_else(|| io::Error::other("a final path that is not Unicode"))?;
        volume_prefix(path)
            .ok_or_else(|| io::Error::other(format!("{path} does not begin with a volume GUID")))
    }

    fn disks_of(&self, volume: &VolumePath) -> io::Result<Vec<u32>> {
        let opened = OpenedVolume::<Cached, NoData>::open(volume, VolumeOptions::new())?;
        Ok(opened
            .disk_extents()
            .iter()
            .map(|extent| extent.disk_number)
            .collect())
    }

    fn is_file_backed(&self, disk: u32) -> io::Result<bool> {
        let opened =
            raw_open::open::<Cached, NoData>(format!(r"\\.\PhysicalDrive{disk}"), ShareMode::ALL)?;
        Ok(storage_device_descriptor(&opened)?.bus_type == StorageBusType::FILE_BACKED_VIRTUAL)
    }

    fn hosts_of(&self, volume: &VolumePath) -> io::Result<Vec<VolumePath>> {
        let opened = raw_open::open::<Cached, NoData>(volume.as_str(), ShareMode::ALL)?;
        let info = storage_dependency_information(
            &opened,
            StorageDependencyFlags::HOST_VOLUMES,
            StorageDependencyVersion::V2,
        )?;
        info.entries()
            .iter()
            .map(|entry| {
                let name = entry
                    .host_volume_name()
                    .ok_or_else(|| io::Error::other("a dependency that names no host volume"))?
                    .to_os_string();
                let name = name
                    .to_str()
                    .ok_or_else(|| io::Error::other("a host volume name that is not Unicode"))?
                    .to_owned();
                host_volume(&name).ok_or_else(|| {
                    io::Error::other(format!("{name} is not a volume this crate can open"))
                })
            })
            .collect()
    }
}

/// The volume a path in `VOLUME_NAME_GUID` form begins with: `\\?\Volume{...}` and nothing after
/// it.
fn volume_prefix(path: &str) -> Option<VolumePath> {
    let end = path.find('}')?;
    VolumePath::guid(&path[..=end]).ok()
}

/// A host volume as the dependency names it: a volume GUID path, or a drive letter's root. No
/// entry has been observed (WV-D-10), so both documented-looking forms are taken and any other is
/// refused, which leaves the file unknown.
fn host_volume(name: &str) -> Option<VolumePath> {
    if let Some(volume) = volume_prefix(name) {
        return Some(volume);
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next(), chars.as_str()) {
        (Some(letter), Some(':'), "" | "\\") => VolumePath::drive(letter).ok(),
        _ => None,
    }
}
