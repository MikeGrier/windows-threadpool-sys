// Copyright (c) 2026 Mike Grier

use std::fs::{self, OpenOptions};
use std::io;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use win_shared_os_owned_handle::SharedHandle;
use win_time_sys::{InterruptTime, TimePoint};

use super::{DeliveryMode, Dioring, Setup, SetupError, SetupRefusal, TargetSetup};
use crate::windows_backend::{FileOptions, OpenedFile, Win32FileTarget};
use durable_ioring::DurableRing;
use durable_ioring::backend::IoBackend;
use durable_ioring::{DurabilityProvider, FlushRequest};
use durable_ioring::{FlushDomain, IoTargetKey};
use durable_ioring::{LineageHandle, StampClock};

// Pushes, completions and the readiness signal: DI-3.2.2.1.
mod pushes;
// Seals and durability in the default lineage: DI-3.2.3.
mod seals;
// Failures and their resolution, through the fault seam: DI-3.2.4.
mod failures;
// Failure stamps, from the instance's clock: WT-2.2.2.
mod stamps;
// Markings, and a failure's final record: DI-3.2.4.2.
mod markings;
// Lineage handles: minting, ending, retiring, and failures across lineages: DI-3.2.5.1.
mod lineages;
// Gated operations, and the cycles a push or a seal would close: DI-3.2.5.2.
mod gates;
// The consumer's durability provider: DI-3.2.6.
mod providers;
// Removing a file, and ending the instance: DI-3.2.7.
mod construction;
mod ending;
mod pumped;
mod signalled;
mod submits;
// A file opened unbuffered, through an instance: DI-D-60.
mod unbuffered;

type Ring = Dioring<Vec<u8>>;
type V = crate::WindowsIds<u64>;

/// Queue sizes for every instance these tests build. Small: nothing here pushes I/O.
const QUEUE: u32 = 16;

fn setup(
    files: Vec<TargetSetup>,
    buffers: Vec<Vec<u8>>,
    provider: Option<Box<dyn DurabilityProvider<V>>>,
) -> Setup<'static, 'static, u64, Vec<u8>> {
    Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files,
        buffers,
        provider,
        diagnostics: None,
        delivery: DeliveryMode::Pool(None),
    }
}

/// A mock interrupt-time clock a test moves by hand (DI-D-38). Clones share one reading, so a test
/// keeps a clone to move the clock it gave away. It keeps `Steady`'s promise by construction: it
/// only moves forwards.
#[derive(Clone, Debug, Default)]
pub(crate) struct MockClock(Arc<AtomicU64>);

impl MockClock {
    /// A clock reading `ticks`.
    pub(crate) fn at(ticks: u64) -> Self {
        Self(Arc::new(AtomicU64::new(ticks)))
    }

    /// Move the clock forwards by `ticks`.
    pub(crate) fn advance(&self, ticks: u64) {
        self.0.fetch_add(ticks, Ordering::SeqCst);
    }
}

impl StampClock for MockClock {
    type Instant = TimePoint<InterruptTime>;

    fn now(&self) -> TimePoint<InterruptTime> {
        TimePoint::from_ticks(self.0.load(Ordering::SeqCst))
    }
}

/// An instance given nothing.
pub(crate) fn empty() -> Ring {
    Ring::new(setup(Vec::new(), Vec::new(), None))
        .expect("build an empty instance")
        .0
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

    pub(crate) fn open(&self) -> OpenedFile {
        OpenedFile::open(&self.0, FileOptions::new().read(true).write(true))
            .expect("open the temporary file")
    }

    /// Whether any handle to the file is open: an exclusive open fails exactly then.
    pub(crate) fn is_held(&self) -> bool {
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

fn file(key: u64, temp: &TempFile, domains: &[&str]) -> TargetSetup {
    TargetSetup {
        key: IoTargetKey(key),
        target: target(temp.open(), domains),
    }
}

/// A target over `file`, declared with `domains`.
pub(crate) fn target(file: OpenedFile, domains: &[&str]) -> Win32FileTarget {
    Win32FileTarget::new(file).with_domains(domains.iter().map(|d| domain(d)))
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

/// A provider that serves fixed domains and keeps every request it is handed, unanswered, for the
/// test to answer through the shared list.
#[derive(Debug)]
pub(crate) struct Keeper {
    domains: Vec<FlushDomain>,
    kept: Kept,
}

/// The requests a `Keeper` has been handed and not yet given up.
pub(crate) type Kept = Arc<Mutex<Vec<FlushRequest<V>>>>;

impl DurabilityProvider<V> for Keeper {
    fn domains(&self) -> Vec<FlushDomain> {
        self.domains.clone()
    }

    fn make_durable(&mut self, request: FlushRequest<V>) {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
    }
}

/// A `Keeper` serving `domains`, and the list it keeps its requests in.
pub(crate) fn keeper(domains: &[&str]) -> (Option<Box<dyn DurabilityProvider<V>>>, Kept) {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let provider = Keeper {
        domains: domains.iter().map(|d| domain(d)).collect(),
        kept: Arc::clone(&kept),
    };
    (Some(Box::new(provider)), kept)
}

fn refused(
    result: Result<(Ring, LineageHandle), SetupError<u64, Vec<u8>>>,
) -> SetupError<u64, Vec<u8>> {
    match result {
        Ok(_) => panic!("construction should have been refused"),
        Err(error) => error,
    }
}

#[test]
fn an_instance_given_nothing_builds_empty() {
    let ring = empty();
    assert!(ring.provider.is_none());
    let engine = ring.engine();
    assert_eq!(engine.backend.registered_buffers(), 0);
    assert!(engine.core.target(IoTargetKey(0)).is_none());
    assert!(engine.backend.target(IoTargetKey(0)).is_none());
}

#[test]
fn an_instance_has_only_its_default_lineage_to_begin_with() {
    let ring = empty();
    let lineages = ring.lineages();
    assert_eq!(lineages.len(), 1);
    let default = &lineages[0];
    assert!(default.is_default);
    assert_eq!(default.lineage, ring.default_lineage().lineage());
    assert!(default.description.is_none());
    assert!(default.durable_through.is_none());
    assert!(default.sealed_through.is_none());
}

#[test]
fn each_instance_has_its_own_default_lineage() {
    let (a, b) = (empty(), empty());
    assert_ne!(a.default_lineage().lineage(), b.default_lineage().lineage());
    assert_eq!(
        a.default_lineage().lineage(),
        a.default_lineage().lineage(),
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
    let ring = Ring::new(setup(files, Vec::new(), None)).expect("build").0;
    let engine = ring.engine();
    for i in 0..3_u32 {
        let key = IoTargetKey(10 + u64::from(i));
        assert_eq!(engine.backend.registration_index(key), Some(i));
        assert!(engine.core.target(key).is_some(), "the core decides for it");
    }
}

#[test]
fn the_ring_holds_construction_files_until_the_instance_is_dropped() {
    let temp = TempFile::new("held");
    let ring = Ring::new(setup(vec![file(1, &temp, &[])], Vec::new(), None))
        .expect("build")
        .0;
    assert!(temp.is_held(), "the instance holds the file it was given");
    drop(ring);
    assert!(!temp.is_held(), "dropping the instance releases it");
}

#[test]
fn a_duplicate_key_is_refused_before_anything_is_created_and_everything_comes_back() {
    let (a, b) = (TempFile::new("dup-a"), TempFile::new("dup-b"));
    let given = vec![file(5, &a, &[]), file(5, &b, &[])];
    let originals: Vec<SharedHandle> = given.iter().map(|f| f.target.handle().clone()).collect();
    let error = refused(Ring::new(setup(given, vec![vec![1, 2, 3]], serves(&["P"]))));
    assert!(matches!(
        error.reason,
        SetupRefusal::DuplicateKey(IoTargetKey(5))
    ));
    assert_eq!(error.files.len(), 2);
    for (back, original) in error.files.iter().zip(&originals) {
        assert!(
            back.target.handle().same_handle(original),
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
fn buffers_given_at_construction_are_registered() {
    let ring = Ring::new(setup(Vec::new(), vec![vec![0; 64], vec![0; 128]], None))
        .expect("build")
        .0;
    assert_eq!(ring.engine().backend.registered_buffers(), 2);
}

#[test]
fn files_and_buffers_are_registered_together() {
    let temp = TempFile::new("both");
    let ring = Ring::new(setup(vec![file(1, &temp, &["A"])], vec![vec![0; 32]], None))
        .expect("build")
        .0;
    assert!(ring.engine().core.target(IoTargetKey(1)).is_some());
    assert_eq!(ring.engine().backend.registered_buffers(), 1);
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
        diagnostics: None,
        delivery: DeliveryMode::Pool(None),
    })
    .expect("build")
    .0;
    assert_eq!(ring.lineages().len(), 1);
}

#[test]
fn an_instance_delivers_on_the_pool_its_environment_names() {
    // This pins that an instance built with an environment delivers at all; that the pool named is
    // the one a drain runs on is pinned by holding its only thread, in pool delivery's own tests.
    use windows_threadpool_sys::callback_env::CallbackEnviron;
    use windows_threadpool_sys::pool::ThreadpoolPool;

    use durable_ioring::{DurabilityRequest, Entry};
    use pushes::{GIVEN, Harness, given, temp};

    let threads = ThreadpoolPool::new().expect("create a pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&threads);
    let file = temp(&[0; 16]);
    let ring = pushes::Ring::new(Setup {
        submission_queue_size: QUEUE,
        completion_queue_size: QUEUE,
        files: vec![given(GIVEN, &file)],
        buffers: Vec::new(),
        provider: None,
        diagnostics: None,
        delivery: DeliveryMode::Pool(Some(&mut env)),
    })
    .expect("build")
    .0;
    let mut harness = Harness::new(ring);
    harness.write(GIVEN, 0, b"pooled", 1, 1);
    harness.next();
    assert_eq!(harness.seal(1), DurabilityRequest::Submitted);
    assert!(matches!(harness.next_entry(), Entry::Durable { .. }));
    // The instance ends first, then `env`, then the pool it names -- the reverse of declaration.
    harness.finish();
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
