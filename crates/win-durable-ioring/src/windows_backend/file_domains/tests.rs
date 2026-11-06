// Copyright (c) 2026 Mike Grier
//! Tests for a file's flush domains.
//!
//! The rule is tested against a storage stack built in memory, which can hold what this machine
//! cannot: a virtual disk, one nested in another, a loop, and a failure at each step. The Windows
//! stack is tested through files on this machine's own volumes, against what a raw volume derives.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use durable_ioring::{FlushDomain, IoTarget, NoData};
use win_shared_os_owned_handle::SharedHandle;

use super::{
    StorageStack, Windows, derive, found, host_volume, seeded, seeded_found, volume_prefix,
};
use crate::dioring::tests::{TempFile, wtf};
use crate::scratch_volume::{ScratchVolume, scratch_directory};
use crate::windows_backend::{
    Cached, FileOptions, OpenedFile, OpenedVolume, Unbuffered, VolumeOptions, VolumePath,
    Win32FileTarget, disk_flush_domain,
};

fn volume(n: u32) -> VolumePath {
    VolumePath::guid(&format!(r"\\?\Volume{{00000000-0000-0000-0000-{n:012x}}}")).expect("a GUID")
}

/// A storage stack held in memory. The file is on `file_volume`; anything not in a map fails.
#[derive(Default)]
struct Stack {
    file_volume: Option<VolumePath>,
    disks: BTreeMap<String, Vec<u32>>,
    backed: BTreeSet<u32>,
    hosts: BTreeMap<String, Vec<VolumePath>>,
    disk_hosts: BTreeMap<u32, Vec<VolumePath>>,
    disk_fails: BTreeSet<u32>,
}

impl Stack {
    fn on(volume: VolumePath) -> Self {
        Self {
            file_volume: Some(volume),
            ..Self::default()
        }
    }

    fn with_disks(mut self, volume: &VolumePath, disks: &[u32]) -> Self {
        self.disks
            .insert(volume.as_str().to_owned(), disks.to_vec());
        self
    }

    fn backed_by_file(mut self, disk: u32) -> Self {
        self.backed.insert(disk);
        self
    }

    fn hosted_on(mut self, volume: &VolumePath, hosts: &[VolumePath]) -> Self {
        self.hosts
            .insert(volume.as_str().to_owned(), hosts.to_vec());
        self
    }

    fn disk_hosted_on(mut self, disk: u32, hosts: &[VolumePath]) -> Self {
        self.disk_hosts.insert(disk, hosts.to_vec());
        self
    }
}

fn missing(what: &str) -> io::Error {
    io::Error::other(what.to_owned())
}

impl StorageStack for Stack {
    fn volume_of(&self, _file: &SharedHandle) -> io::Result<VolumePath> {
        self.file_volume.clone().ok_or_else(|| missing("no volume"))
    }

    fn disks_of(&self, volume: &VolumePath) -> io::Result<Vec<u32>> {
        self.disks
            .get(volume.as_str())
            .cloned()
            .ok_or_else(|| missing("no extents"))
    }

    fn is_file_backed(&self, disk: u32) -> io::Result<bool> {
        if self.disk_fails.contains(&disk) {
            return Err(missing("no descriptor"));
        }
        Ok(self.backed.contains(&disk))
    }

    fn hosts_of(&self, volume: &VolumePath) -> io::Result<Vec<VolumePath>> {
        self.hosts
            .get(volume.as_str())
            .cloned()
            .ok_or_else(|| missing("no dependency"))
    }
}

impl Stack {
    /// The dependency query asked of raw disk `disk`'s own handle.
    fn hosts_of_disk(&self, disk: u32) -> io::Result<Vec<VolumePath>> {
        self.disk_hosts
            .get(&disk)
            .cloned()
            .ok_or_else(|| missing("no disk dependency"))
    }
}

/// A raw volume opened at `volume`: its disks are the extents its handle reports, and its handle is
/// asked for its hosts, as `OpenedVolume::open` does.
fn volume_domains(stack: &Stack, volume: &VolumePath) -> io::Result<Vec<FlushDomain>> {
    let own = stack.disks_of(volume)?;
    seeded(stack, &own, || stack.hosts_of(volume))
}

fn volume_found(stack: &Stack, volume: &VolumePath) -> Vec<FlushDomain> {
    volume_domains(stack, volume).unwrap_or_default()
}

/// Raw disk `disk` opened, its handle asked for its hosts, as `OpenedDisk::open` does.
fn disk_domains(stack: &Stack, disk: u32) -> io::Result<Vec<FlushDomain>> {
    seeded(stack, &[disk], || stack.hosts_of_disk(disk))
}

fn disk_found(stack: &Stack, disk: u32) -> Vec<FlushDomain> {
    seeded_found(stack, &[disk], || stack.hosts_of_disk(disk))
}

/// `text` as UTF-16, the form the names arrive in.
fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// A lone high surrogate: valid in a Windows name, not in Unicode.
const LONE: u16 = 0xD800;

/// Any handle: the stack in memory does not look at it.
fn any_handle() -> (SharedHandle, TempFile) {
    let temp = TempFile::new("fd-any");
    let file = File::create(&temp.0).expect("create");
    (SharedHandle::from(file), temp)
}

fn domains(disks: &[u32]) -> Vec<FlushDomain> {
    disks.iter().copied().map(disk_flush_domain).collect()
}

// --- The rule, in memory ----------------------------------------------------------------------

#[test]
fn a_file_on_plain_disks_has_one_domain_per_disk_once_each_in_disk_order() {
    let (handle, _temp) = any_handle();
    for (disks, expected) in [
        (vec![0], vec![0]),
        (vec![2, 0, 2, 1], vec![0, 1, 2]),
        (vec![7, 3], vec![3, 7]),
    ] {
        let stack = Stack::on(volume(1)).with_disks(&volume(1), &disks);
        assert_eq!(
            derive(&stack, &handle).expect("plain disks"),
            domains(&expected),
            "{disks:?}"
        );
    }
}

#[test]
fn a_virtual_disk_adds_the_disks_of_the_volume_holding_its_backing_file_and_keeps_its_own() {
    let (handle, _temp) = any_handle();
    // The file is on volume 1, which sits on disk 5, a VHD; the VHD's file is on volume 2, on disk 0.
    let stack = Stack::on(volume(1))
        .with_disks(&volume(1), &[5])
        .backed_by_file(5)
        .hosted_on(&volume(1), &[volume(2)])
        .with_disks(&volume(2), &[0]);

    assert_eq!(derive(&stack, &handle).expect("followed"), domains(&[0, 5]));
}

#[test]
fn a_raw_volume_on_a_virtual_disk_shares_fate_with_the_volume_holding_its_backing_file() {
    // The raw volume is volume 1, on disk 5, a VHD whose file is on volume 2, on disk 0: a file on
    // volume 2 is on disk 0, so the raw volume must carry disk 0 too.
    let stack = Stack::default()
        .with_disks(&volume(1), &[5])
        .backed_by_file(5)
        .hosted_on(&volume(1), &[volume(2)])
        .with_disks(&volume(2), &[0]);

    assert_eq!(
        volume_domains(&stack, &volume(1)).expect("followed"),
        domains(&[0, 5])
    );
    assert_eq!(volume_found(&stack, &volume(1)), domains(&[0, 5]));
}

#[test]
fn a_raw_volume_on_plain_disks_has_its_disks_and_is_not_asked_for_dependencies() {
    let stack = Stack::default().with_disks(&volume(1), &[3, 1, 3]);
    assert!(stack.hosts.is_empty());
    assert_eq!(volume_found(&stack, &volume(1)), domains(&[1, 3]));
}

#[test]
fn a_raw_volume_whose_virtual_disk_cannot_be_followed_has_no_domains() {
    let no_dependency = Stack::default()
        .with_disks(&volume(1), &[5])
        .backed_by_file(5);
    let mut no_descriptor = Stack::default().with_disks(&volume(1), &[5]);
    no_descriptor.disk_fails.insert(5);
    for (stack, what) in [
        (no_dependency, "the dependency query"),
        (no_descriptor, "the descriptor"),
        (Stack::default(), "the extents"),
    ] {
        assert!(volume_domains(&stack, &volume(1)).is_err(), "{what}");
        assert!(volume_found(&stack, &volume(1)).is_empty(), "{what}");
    }
}

#[test]
fn a_raw_disk_that_is_virtual_is_followed_through_its_own_dependency() {
    // Disk 5 is a VHD whose file is on volume 2, on disk 0, whose own VHD file is on volume 3.
    let stack = Stack::default()
        .backed_by_file(5)
        .disk_hosted_on(5, &[volume(2)])
        .with_disks(&volume(2), &[4])
        .backed_by_file(4)
        .hosted_on(&volume(2), &[volume(3)])
        .with_disks(&volume(3), &[0]);

    assert_eq!(
        disk_domains(&stack, 5).expect("followed"),
        domains(&[0, 4, 5])
    );
    assert_eq!(disk_found(&stack, 5), domains(&[0, 4, 5]));
}

#[test]
fn a_raw_target_is_derived_from_its_handle_and_never_from_its_path_again() {
    // The handle reports disk 5, a VHD, and names volume 2 as its host. The stack's own idea of
    // volume 1 -- what its path would resolve to now -- is a different disk, and is never consulted.
    let stack = Stack::default()
        .with_disks(&volume(1), &[9])
        .hosted_on(&volume(1), &[volume(3)])
        .with_disks(&volume(3), &[8])
        .backed_by_file(5)
        .with_disks(&volume(2), &[0]);
    assert_eq!(
        seeded(&stack, &[5], || Ok(vec![volume(2)])).expect("from the handle"),
        domains(&[0, 5])
    );
    // A plain disk never asks the handle.
    assert_eq!(
        seeded(
            &Stack::default(),
            &[3, 1, 3],
            || -> io::Result<Vec<VolumePath>> {
                panic!("a plain disk's handle is not asked for its hosts")
            }
        )
        .expect("plain"),
        domains(&[1, 3])
    );
    // A handle reporting no disk, or no host for its virtual disk, has none.
    assert!(
        seeded(&stack, &[], || Ok(vec![volume(2)])).is_err(),
        "no disk"
    );
    assert!(seeded(&stack, &[5], || Ok(Vec::new())).is_err(), "no host");
    assert!(
        seeded_found(&stack, &[5], || Err(missing("refused"))).is_empty(),
        "refused"
    );
}

#[test]
fn a_raw_plain_disk_is_itself_and_is_not_asked_for_dependencies() {
    let stack = Stack::default();
    assert!(stack.disk_hosts.is_empty());
    assert_eq!(disk_found(&stack, 7), domains(&[7]));
}

#[test]
fn a_raw_virtual_disk_that_cannot_be_followed_has_no_domains() {
    let no_dependency = Stack::default().backed_by_file(5);
    let no_hosts = Stack::default().backed_by_file(5).disk_hosted_on(5, &[]);
    let host_no_extents = Stack::default()
        .backed_by_file(5)
        .disk_hosted_on(5, &[volume(2)]);
    let mut no_descriptor = Stack::default();
    no_descriptor.disk_fails.insert(5);
    for (stack, what) in [
        (no_dependency, "the dependency query"),
        (no_hosts, "a dependency naming no host"),
        (host_no_extents, "the host volume's extents"),
        (no_descriptor, "the descriptor"),
    ] {
        assert!(disk_domains(&stack, 5).is_err(), "{what}");
        assert!(disk_found(&stack, 5).is_empty(), "{what}");
    }
}

#[test]
fn a_virtual_disk_inside_a_virtual_disk_is_followed_to_the_bottom() {
    let (handle, _temp) = any_handle();
    let stack = Stack::on(volume(1))
        .with_disks(&volume(1), &[5])
        .backed_by_file(5)
        .hosted_on(&volume(1), &[volume(2)])
        .with_disks(&volume(2), &[4])
        .backed_by_file(4)
        .hosted_on(&volume(2), &[volume(3)])
        .with_disks(&volume(3), &[0, 1]);

    assert_eq!(
        derive(&stack, &handle).expect("followed twice"),
        domains(&[0, 1, 4, 5])
    );
}

#[test]
fn a_virtual_disk_with_several_hosts_takes_all_of_them_and_a_host_reached_twice_counts_once() {
    let (handle, _temp) = any_handle();
    let stack = Stack::on(volume(1))
        .with_disks(&volume(1), &[5])
        .backed_by_file(5)
        .hosted_on(&volume(1), &[volume(2), volume(3), volume(2)])
        .with_disks(&volume(2), &[0])
        .with_disks(&volume(3), &[1]);

    assert_eq!(derive(&stack, &handle).expect("both"), domains(&[0, 1, 5]));
}

#[test]
fn a_loop_in_the_dependencies_ends() {
    let (handle, _temp) = any_handle();
    let stack = Stack::on(volume(1))
        .with_disks(&volume(1), &[5])
        .backed_by_file(5)
        .hosted_on(&volume(1), &[volume(2)])
        .with_disks(&volume(2), &[6])
        .backed_by_file(6)
        .hosted_on(&volume(2), &[volume(1)]);

    assert_eq!(derive(&stack, &handle).expect("ends"), domains(&[5, 6]));
}

#[test]
fn a_chain_deeper_than_the_crate_follows_is_unknown() {
    let (handle, _temp) = any_handle();
    let mut stack = Stack::on(volume(0));
    for n in 0..=u32::try_from(super::MAX_VOLUMES).expect("small") {
        stack = stack
            .with_disks(&volume(n), &[100 + n])
            .backed_by_file(100 + n)
            .hosted_on(&volume(n), &[volume(n + 1)]);
    }

    assert!(derive(&stack, &handle).is_err());
    assert!(found(&stack, &handle).is_empty());

    // And one short of the limit is followed to the end.
    let mut shallow = Stack::on(volume(0));
    let last = u32::try_from(super::MAX_VOLUMES - 1).expect("small");
    for n in 0..last {
        shallow = shallow
            .with_disks(&volume(n), &[100 + n])
            .backed_by_file(100 + n)
            .hosted_on(&volume(n), &[volume(n + 1)]);
    }
    shallow = shallow.with_disks(&volume(last), &[0]);
    assert_eq!(
        found(&shallow, &handle).len(),
        usize::try_from(last).expect("small") + 1
    );
}

#[test]
fn a_failure_at_any_step_leaves_no_domains_and_never_a_partial_set() {
    let (handle, _temp) = any_handle();
    let whole = || {
        Stack::on(volume(1))
            .with_disks(&volume(1), &[5, 2])
            .backed_by_file(5)
            .hosted_on(&volume(1), &[volume(2)])
            .with_disks(&volume(2), &[0])
    };
    assert_eq!(found(&whole(), &handle), domains(&[0, 2, 5]), "the control");

    let mut no_volume = whole();
    no_volume.file_volume = None;
    let mut no_extents = whole();
    no_extents.disks.remove(volume(1).as_str());
    let mut host_no_extents = whole();
    host_no_extents.disks.remove(volume(2).as_str());
    let mut no_descriptor = whole();
    no_descriptor.disk_fails.insert(2);
    let mut no_dependency = whole();
    no_dependency.hosts.clear();
    let no_hosts = whole().hosted_on(&volume(1), &[]);
    let on_no_disk = whole().with_disks(&volume(2), &[]);

    for (stack, what) in [
        (no_volume, "the file's volume"),
        (no_extents, "the volume's extents"),
        (host_no_extents, "the host volume's extents"),
        (no_descriptor, "a disk's descriptor"),
        (no_dependency, "the dependency query"),
        (no_hosts, "a dependency naming no host"),
        (on_no_disk, "a volume on no disk"),
    ] {
        assert!(derive(&stack, &handle).is_err(), "{what}");
        assert!(found(&stack, &handle).is_empty(), "{what}");
    }
}

#[test]
fn a_volume_on_no_virtual_disk_is_never_asked_for_its_dependencies() {
    // The dependency query's answer for a volume that is not on a virtual disk is undocumented, so
    // it is never asked: a stack whose dependency query fails still derives plain disks.
    let (handle, _temp) = any_handle();
    let stack = Stack::on(volume(1)).with_disks(&volume(1), &[0, 1]);
    assert!(stack.hosts.is_empty());

    assert_eq!(derive(&stack, &handle).expect("plain"), domains(&[0, 1]));
}

// --- Reading the names --------------------------------------------------------------------------

#[test]
fn a_final_path_in_guid_form_names_its_volume_and_nothing_else_does() {
    let guid = r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}";
    for path in [
        guid.to_owned(),
        format!(r"{guid}\"),
        format!(r"{guid}\dir\file.txt"),
    ] {
        assert_eq!(
            volume_prefix(&utf16(&path)).expect("a volume").as_str(),
            guid,
            "{path}"
        );
    }
    // A path whose name holds a lone surrogate still names its volume: only the prefix is read.
    let mut odd = utf16(&format!(r"{guid}\dir\"));
    odd.extend([LONE, u16::from(b'x')]);
    assert_eq!(volume_prefix(&odd).expect("a volume").as_str(), guid);
    let mut inside = utf16(r"\\?\Volume{0123");
    inside.push(LONE);
    inside.extend(utf16(r"4567-89ab-cdef-0123-456789abcdef}\file"));
    assert!(volume_prefix(&inside).is_none(), "a surrogate in the GUID");
    for path in [
        r"\\?\UNC\server\share\file",
        r"C:\file",
        r"\\?\C:\file",
        r"\\?\Volume{not-a-guid}\file",
        "",
    ] {
        assert!(volume_prefix(&utf16(path)).is_none(), "{path}");
    }
}

#[test]
fn a_host_volume_is_a_guid_path_or_a_drive_root_and_nothing_else() {
    let guid = r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}";
    assert_eq!(
        host_volume(&utf16(&format!(r"{guid}\")))
            .expect("guid")
            .as_str(),
        guid
    );
    assert_eq!(
        host_volume(&utf16(r"C:\")).expect("drive").as_str(),
        r"\\.\C:"
    );
    assert_eq!(
        host_volume(&utf16("d:")).expect("drive").as_str(),
        r"\\.\D:"
    );
    for name in [r"C:\dir", "C", r"CC:\", r"\\server\share", r"1:\", ""] {
        assert!(host_volume(&utf16(name)).is_none(), "{name}");
    }
    assert!(host_volume(&[u16::from(b'C'), u16::from(b':'), LONE]).is_none());
}

// --- This machine -------------------------------------------------------------------------------

/// The volume `path` is on, by its drive letter.
fn drive_of(path: &Path) -> VolumePath {
    let text = path.to_str().expect("a UTF-8 temp path");
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    VolumePath::drive(plain.chars().next().expect("a drive")).expect("a drive letter")
}

fn expected_for(path: &Path) -> Vec<FlushDomain> {
    OpenedVolume::<Cached, NoData>::open(&drive_of(path), VolumeOptions::new())
        .expect("open the volume with no data access")
        .flush_domains()
}

#[test]
fn a_local_file_has_the_domains_its_volume_derives() {
    let temp = TempFile::new("fd-local");
    let file = File::create(&temp.0).expect("create");

    let derived = derive(&Windows, &SharedHandle::from(file)).expect("a local file");

    assert!(!derived.is_empty());
    assert_eq!(derived, expected_for(&temp.0));
}

#[test]
fn a_file_opened_by_this_crate_carries_its_domains_into_its_target_in_either_mode() {
    let temp = TempFile::new("fd-opened");
    File::create(&temp.0).expect("create");
    let expected = expected_for(&temp.0);

    let cached = OpenedFile::<Cached>::open(wtf(&temp.0), FileOptions::new().read().write())
        .expect("cached");
    assert_eq!(cached.flush_domains(), expected.as_slice());
    let target = Win32FileTarget::new(cached);
    assert_eq!(target.flush_domains(), expected.as_slice());

    let unbuffered =
        OpenedFile::<Unbuffered>::open(wtf(&temp.0), FileOptions::new().read().write())
            .expect("unbuffered");
    assert_eq!(unbuffered.flush_domains(), expected.as_slice());
}

#[test]
fn a_file_whose_name_is_not_unicode_still_has_its_volumes_domains() {
    // A lone surrogate is a valid Windows name and not Unicode; only the volume prefix of the final
    // path is read, so it changes nothing.
    let base = TempFile::new("fd-surrogate");
    let mut name = base.0.file_name().expect("a name").to_owned();
    name.push(OsString::from_wide(&[0xD800]));
    let temp = TempFile(base.0.with_file_name(name));
    File::create(&temp.0).expect("create a file named with a lone surrogate");

    let opened =
        OpenedFile::<Cached>::open(wtf(&temp.0), FileOptions::new().read().write()).expect("open");

    assert!(!opened.flush_domains().is_empty());
    assert_eq!(opened.flush_domains(), expected_for(&base.0).as_slice());
}

#[test]
fn a_declared_domain_adds_to_the_ones_found_and_removes_none() {
    let temp = TempFile::new("fd-declared");
    File::create(&temp.0).expect("create");
    let expected = expected_for(&temp.0);

    let opened =
        OpenedFile::<Cached>::open(wtf(&temp.0), FileOptions::new().read().write()).expect("open");
    let extra = FlushDomain::new(b"a consumer's own".to_vec());
    let target = Win32FileTarget::new(opened).with_domains([extra.clone(), expected[0].clone()]);

    let reported = target.flush_domains();
    for domain in &expected {
        assert!(reported.contains(domain), "{domain:?} found is kept");
    }
    assert!(reported.contains(&extra), "the declared one is added");
    assert_eq!(
        reported.len(),
        expected.len() + 1,
        "each once: {reported:?}"
    );
}

/// The administrative share of this volume, `\\localhost\X$\...`, naming `local`.
fn admin_share(local: &Path) -> PathBuf {
    let text = local.to_str().expect("a UTF-8 temp path");
    let plain = text.strip_prefix(r"\\?\").unwrap_or(text);
    let drive = plain.chars().next().expect("a drive letter");
    PathBuf::from(format!(r"\\localhost\{drive}${}", &plain[2..]))
}

#[test]
#[ignore = "needs the administrative share \\\\localhost\\C$, which a CI runner has"]
fn a_file_on_a_share_has_no_domains_found_even_when_the_share_is_this_machine() {
    let temp = TempFile::new("fd-remote");
    File::create(&temp.0).expect("create");

    let opened = OpenedFile::<Cached>::open(
        wtf(&admin_share(&temp.0)),
        FileOptions::new().read().write(),
    )
    .expect("a cached file on a share");

    assert!(
        derive(&Windows, opened.handle()).is_err(),
        "a remote handle's final path names no volume"
    );
    assert!(opened.flush_domains().is_empty());
    let target = Win32FileTarget::new(opened);
    assert!(target.flush_domains().is_empty(), "unknown");

    // A consumer that knows where the share's storage is declares it, and with none found that is
    // the file's whole set: here, honestly, this machine's own disks.
    let local = expected_for(&temp.0);
    let declared = target.with_domains(local.clone());
    assert_eq!(declared.flush_domains(), local.as_slice());
}

#[test]
#[ignore = "needs an administrator: creates and attaches a virtual disk"]
fn a_raw_volume_on_a_scratch_virtual_disk_is_file_backed_and_names_the_volume_its_file_is_on() {
    // The one hop no developer machine shows: a real virtual disk, whose file is in the temporary
    // directory. Its disk must describe itself as file-backed, and the dependency must name the
    // volume that holds the file, whose disks are the ones the derivation then adds.
    let scratch = ScratchVolume::create(64).expect("a scratch volume");
    let volume = scratch.path();

    let disks = Windows
        .disks_of(&volume)
        .expect("the scratch volume's extents");
    assert_eq!(disks.len(), 1, "one virtual disk: {disks:?}");
    assert!(
        Windows
            .is_file_backed(disks[0])
            .expect("the virtual disk describes itself"),
        "a VHD's disk is BusTypeFileBackedVirtual"
    );

    let hosts = Windows.hosts_of(&volume).expect("the dependency query");
    assert!(!hosts.is_empty(), "the dependency names a host");
    let expected_disks = Windows
        .disks_of(&drive_of(&scratch_directory()))
        .expect("the temporary directory's volume");
    let mut host_disks = Vec::new();
    for host in &hosts {
        host_disks.extend(Windows.disks_of(host).expect("a host volume's extents"));
    }
    host_disks.sort_unstable();
    host_disks.dedup();
    let mut expected = expected_disks;
    expected.sort_unstable();
    expected.dedup();
    assert_eq!(
        host_disks, expected,
        "the hosts are the volume the VHD file is on: {hosts:?}"
    );
}
