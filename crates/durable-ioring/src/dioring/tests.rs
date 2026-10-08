// Copyright (c) 2026 Mike Grier

use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::OwnedHandle;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use win_shared_os_owned_handle::SharedHandle;

use super::{Dioring, FileSetup, FileSlot, Setup, SetupError, SetupRefusal};
use crate::contract::DurableRing;
use crate::ids::DioringIds;
use crate::provider::{DurabilityProvider, FlushRequest};
use crate::types::{FileKey, FileOptions, FlushDomain};

// Pushes, completions and the readiness signal: DI-3.2.2.1.
mod pushes;

type Ring = Dioring<Vec<u8>>;
type V = DioringIds<u64>;

/// Queue sizes for every instance these tests build. Small: nothing here pushes I/O.
const QUEUE: u32 = 16;

fn setup(
    files: Vec<FileSetup>,
    buffers: Vec<Vec<u8>>,
    provider: Option<Box<dyn DurabilityProvider<V>>>,
) -> Setup<u64, Vec<u8>> {
    Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files,
        buffers,
        provider,
    }
}

/// An instance given nothing.
pub(crate) fn empty() -> Ring {
    Ring::new(setup(Vec::new(), Vec::new(), None)).expect("build an empty instance")
}

/// A temporary file, removed when dropped.
pub(crate) struct TempFile(pub(crate) PathBuf);

impl TempFile {
    pub(crate) fn new(tag: &str) -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "durable-ioring-setup-{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, b"x").expect("create the temporary file");
        Self(path)
    }

    pub(crate) fn open(&self) -> SharedHandle {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.0)
            .expect("open the temporary file");
        SharedHandle::new(OwnedHandle::from(file))
    }

    /// Whether any handle to the file is open: an exclusive open fails exactly then.
    fn is_held(&self) -> bool {
        held(&self.0)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn held(path: &Path) -> bool {
    OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .is_err()
}

fn domain(name: &str) -> FlushDomain {
    FlushDomain::new(name.as_bytes().to_vec())
}

fn file(key: u64, temp: &TempFile, domains: &[&str]) -> FileSetup {
    FileSetup {
        key: FileKey(key),
        file: temp.open(),
        options: FileOptions::new().domains(domains.iter().map(|d| domain(d))),
    }
}

/// A provider that serves fixed domains and is never asked for anything here.
#[derive(Debug)]
struct Serves(Vec<FlushDomain>);

impl DurabilityProvider<V> for Serves {
    fn domains(&self) -> Vec<FlushDomain> {
        self.0.clone()
    }

    fn make_durable(&mut self, _: FlushRequest<V>) {
        unreachable!("construction asks a provider for its domains and nothing else")
    }
}

fn serves(domains: &[&str]) -> Option<Box<dyn DurabilityProvider<V>>> {
    Some(Box::new(Serves(
        domains.iter().map(|d| domain(d)).collect(),
    )))
}

fn refused(result: Result<Ring, SetupError<u64, Vec<u8>>>) -> SetupError<u64, Vec<u8>> {
    match result {
        Ok(_) => panic!("construction should have been refused"),
        Err(error) => error,
    }
}

#[test]
fn an_instance_given_nothing_builds_empty() {
    let ring = empty();
    assert!(ring.files.is_empty());
    assert!(ring.domains.is_empty());
    assert!(ring.registered.is_none());
    assert!(ring.provider.is_none());
}

#[test]
fn an_instance_has_only_its_default_lineage_to_begin_with() {
    let ring = empty();
    let lineages = ring.lineages();
    assert_eq!(lineages.len(), 1);
    let default = &lineages[0];
    assert!(default.is_default);
    assert_eq!(default.lineage, ring.default_lineage());
    assert!(default.description.is_none());
    assert!(default.durable_through.is_none());
    assert!(default.sealed_through.is_none());
}

#[test]
fn each_instance_has_its_own_default_lineage() {
    let (a, b) = (empty(), empty());
    assert_ne!(a.default_lineage(), b.default_lineage());
    assert_eq!(
        a.default_lineage(),
        a.default_lineage(),
        "stable for one instance"
    );
}

#[test]
fn files_given_at_construction_are_registered_in_order() {
    let temps: Vec<TempFile> = (0..3)
        .map(|i| TempFile::new(&format!("order{i}")))
        .collect();
    let files = temps
        .iter()
        .enumerate()
        .map(|(i, t)| file(10 + i as u64, t, &[]))
        .collect();
    let ring = Ring::new(setup(files, Vec::new(), None)).expect("build");
    for i in 0..3_u32 {
        match &ring.files[&FileKey(10 + u64::from(i))].target {
            FileSlot::Registered { index, .. } => assert_eq!(index.index(), i),
            FileSlot::Shared(_) => panic!("a construction file is registered"),
        }
    }
}

#[test]
fn the_ring_holds_construction_files_until_the_instance_is_dropped() {
    let temp = TempFile::new("held");
    let ring = Ring::new(setup(vec![file(1, &temp, &[])], Vec::new(), None)).expect("build");
    assert!(temp.is_held(), "the instance holds the file it was given");
    drop(ring);
    assert!(!temp.is_held(), "dropping the instance releases it");
}

#[test]
fn a_duplicate_key_is_refused_before_anything_is_created_and_everything_comes_back() {
    let (a, b) = (TempFile::new("dup-a"), TempFile::new("dup-b"));
    let given = vec![file(5, &a, &[]), file(5, &b, &[])];
    let originals: Vec<SharedHandle> = given.iter().map(|f| f.file.clone()).collect();
    let error = refused(Ring::new(setup(given, vec![vec![1, 2, 3]], serves(&["P"]))));
    assert!(matches!(
        error.reason,
        SetupRefusal::DuplicateKey(FileKey(5))
    ));
    assert_eq!(error.files.len(), 2);
    for (back, original) in error.files.iter().zip(&originals) {
        assert!(
            back.file.same_handle(original),
            "the same handle comes back, in order"
        );
    }
    assert_eq!(error.buffers, Some(vec![vec![1, 2, 3]]));
    assert_eq!(
        error
            .provider
            .expect("the provider comes back")
            .domains()
            .len(),
        1
    );
    drop((error.files, originals));
    assert!(!a.is_held() && !b.is_held(), "no ring took a handle");
}

#[test]
fn a_provider_naming_a_domain_twice_is_refused_and_everything_comes_back() {
    let temp = TempFile::new("pdup");
    let error = refused(Ring::new(setup(
        vec![file(1, &temp, &["A"])],
        vec![vec![0; 8]],
        serves(&["A", "B", "A"]),
    )));
    match error.reason {
        SetupRefusal::DuplicateProviderDomain(domain) => assert_eq!(domain.bytes(), b"A"),
        other => panic!("refused for the wrong reason: {other:?}"),
    }
    assert_eq!(error.files.len(), 1);
    assert_eq!(error.buffers, Some(vec![vec![0; 8]]));
    assert!(error.provider.is_some());
}

#[test]
fn a_ring_that_cannot_be_created_refuses_construction_and_everything_comes_back() {
    let temp = TempFile::new("noring");
    let mut given = setup(vec![file(1, &temp, &[])], vec![vec![7; 4]], serves(&["P"]));
    // Zero is not refused -- the kernel rounds it up -- but a queue no system can allocate is.
    given.submission_queue_size = u32::MAX;
    given.completion_queue_size = u32::MAX;
    let error = refused(Ring::new(given));
    assert!(
        matches!(error.reason, SetupRefusal::Ring(_)),
        "{:?}",
        error.reason
    );
    assert_eq!(error.files.len(), 1);
    assert_eq!(error.buffers, Some(vec![vec![7; 4]]));
    assert!(error.provider.is_some());
}

#[test]
fn domains_are_interned_once_per_distinct_bytes() {
    let temps: Vec<TempFile> = (0..3)
        .map(|i| TempFile::new(&format!("intern{i}")))
        .collect();
    let files = vec![
        file(1, &temps[0], &["A", "B"]),
        file(2, &temps[1], &["B", "C", "B"]),
        file(3, &temps[2], &[]),
    ];
    let ring = Ring::new(setup(files, Vec::new(), serves(&["C", "D"]))).expect("build");
    assert_eq!(ring.domains.len(), 4, "A, B, C and D, once each");
    let id = |name: &str| ring.domains[&domain(name)];
    let of = |key: u64| ring.files[&FileKey(key)].domains.clone();

    let mut one = vec![id("A"), id("B")];
    one.sort_unstable();
    assert_eq!(&*of(1), one.as_slice());
    let mut two = vec![id("B"), id("C")];
    two.sort_unstable();
    assert_eq!(&*of(2), two.as_slice(), "a repeated domain counts once");
    assert!(of(3).is_empty(), "a file declared with none stays unknown");

    let mut provider = vec![id("C"), id("D")];
    provider.sort_unstable();
    assert_eq!(
        &*ring
            .provider
            .as_ref()
            .expect("the provider is kept")
            .domains,
        provider.as_slice(),
        "a provider's domain shares its id with a file's"
    );
}

#[test]
fn buffers_given_at_construction_are_registered() {
    let ring = Ring::new(setup(Vec::new(), vec![vec![0; 64], vec![0; 128]], None)).expect("build");
    assert_eq!(ring.registered.as_ref().expect("registered").len(), 2);
}

#[test]
fn files_and_buffers_are_registered_together() {
    let temp = TempFile::new("both");
    let ring =
        Ring::new(setup(vec![file(1, &temp, &["A"])], vec![vec![0; 32]], None)).expect("build");
    assert_eq!(ring.files.len(), 1);
    assert_eq!(ring.registered.as_ref().expect("registered").len(), 1);
    assert!(temp.is_held());
}

#[test]
fn an_instance_is_built_with_any_epoch_id_and_context_type() {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    struct Lsn(u64);
    let ring = Dioring::<Vec<u8>, Lsn, u32>::new(Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files: Vec::new(),
        buffers: Vec::new(),
        provider: None,
    })
    .expect("build");
    assert_eq!(ring.lineages().len(), 1);
}

/// The refusals hand back their error rather than panicking or hiding it.
#[test]
fn a_setup_error_prints_without_a_debug_buffer_type() {
    struct Opaque;
    // `Opaque` is not `Debug`; the error still is.
    let error: SetupError<u64, Opaque> = SetupError {
        reason: SetupRefusal::Ring(io::Error::other("x")),
        files: Vec::new(),
        buffers: Some(vec![Opaque, Opaque]),
        provider: None,
    };
    let text = format!("{error:?}");
    assert!(text.contains("Some(2)"), "{text}");
}
