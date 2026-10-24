// Copyright (c) 2026 Mike Grier
//! What an instance refuses past the capacity it was built for (DI-D-107), through the driver:
//! construction, a target given or added, a push, a mint and an import. The core's own rules are
//! `durable-ioring`'s tests; this is what the driver makes of them -- a refused construction hands
//! everything back and creates nothing, and a refused `add_target` attaches nothing.

use std::os::windows::io::AsRawHandle;

use super::{CAPACITY, Ring, SetupRefusal, TempFile, file, refused, serves, setup, target};
use crate::dioring::{Dioring, Setup};
use durable_ioring::{
    AddTargetCause, AtCapacity, Capacity, ConstructionError, DurableRing, ImportError, ImportScope,
    IoTargetKey, Limit, PushRefusal,
};

fn declared(f: impl FnOnce(&mut Capacity)) -> Capacity {
    let mut capacity = CAPACITY;
    f(&mut capacity);
    capacity
}

fn with(
    capacity: Capacity,
    setup: Setup<'static, 'static, u64, Vec<u8>>,
) -> Setup<'static, 'static, u64, Vec<u8>> {
    Setup { capacity, ..setup }
}

fn at(limit: Limit, declared: usize) -> AtCapacity {
    AtCapacity { limit, declared }
}

#[test]
fn a_capacity_declaring_no_lineage_is_refused_and_everything_comes_back() {
    let temp = TempFile::new("cap-none");
    let given = with(
        declared(|c| c.lineages = 0),
        setup(vec![file(1, &temp, &[])], vec![vec![1; 4]], serves(&["P"])),
    );
    let error = refused(Ring::new(given));
    assert!(matches!(
        error.reason,
        SetupRefusal::Construction(ConstructionError::NoLineages)
    ));
    assert_eq!(error.files.len(), 1);
    assert_eq!(error.buffers, Some(vec![vec![1; 4]]));
    assert!(error.provider.is_some());
    drop(error.files);
    assert!(!temp.is_held(), "no ring took a handle");
}

#[test]
fn a_provider_serving_more_domains_than_declared_is_refused_and_everything_comes_back() {
    let temp = TempFile::new("cap-domains");
    let given = with(
        declared(|c| c.domains = 1),
        setup(vec![file(1, &temp, &[])], Vec::new(), serves(&["A", "B"])),
    );
    let error = refused(Ring::new(given));
    assert!(matches!(
        error.reason,
        SetupRefusal::Construction(ConstructionError::TooManyDomains {
            served: 2,
            declared: 1
        })
    ));
    assert_eq!(error.files.len(), 1);
    assert!(error.provider.is_some());
}

#[test]
fn more_targets_than_declared_are_refused_and_every_file_comes_back() {
    let (a, b) = (TempFile::new("cap-t-a"), TempFile::new("cap-t-b"));
    let given = with(
        declared(|c| c.targets = 1),
        setup(vec![file(1, &a, &[]), file(2, &b, &[])], Vec::new(), None),
    );
    let error = refused(Ring::new(given));
    assert!(matches!(
        error.reason,
        SetupRefusal::AtCapacity(AtCapacity {
            limit: Limit::Targets,
            declared: 1
        })
    ));
    assert_eq!(error.files.len(), 2, "both come back, in order");
    drop(error.files);
    assert!(!a.is_held() && !b.is_held(), "no ring took a handle");
}

#[test]
fn targets_declaring_more_domains_than_declared_are_refused() {
    let (a, b) = (TempFile::new("cap-d-a"), TempFile::new("cap-d-b"));
    let given = with(
        declared(|c| c.domains = 1),
        setup(
            vec![file(1, &a, &["x"]), file(2, &b, &["y"])],
            Vec::new(),
            None,
        ),
    );
    let error = refused(Ring::new(given));
    assert!(matches!(
        error.reason,
        SetupRefusal::AtCapacity(AtCapacity {
            limit: Limit::Domains,
            declared: 1
        })
    ));
}

#[test]
fn an_instance_built_at_exactly_its_capacity_is_accepted() {
    let (a, b) = (TempFile::new("cap-ok-a"), TempFile::new("cap-ok-b"));
    let given = with(
        declared(|c| {
            c.targets = 2;
            c.domains = 2;
        }),
        setup(
            vec![file(1, &a, &["x"]), file(2, &b, &["y"])],
            Vec::new(),
            None,
        ),
    );
    let _built = Ring::new(given).expect("exactly the capacity");
}

#[test]
fn an_add_target_past_the_targets_declared_is_refused_hands_the_target_back_and_attaches_nothing() {
    let (a, b) = (TempFile::new("cap-add-a"), TempFile::new("cap-add-b"));
    let mut ring = Dioring::<Vec<u8>>::new(with(
        declared(|c| c.targets = 1),
        setup(vec![file(1, &a, &[])], Vec::new(), None),
    ))
    .expect("build")
    .0;
    let offered = b.open();
    let raw = offered.as_raw_handle();
    let error = ring
        .add_target(IoTargetKey(2), target(offered, &[]))
        .expect_err("full");
    assert_eq!(
        error.cause,
        AddTargetCause::AtCapacity(at(Limit::Targets, 1))
    );
    assert_eq!(
        error.target.handle().as_raw_handle(),
        raw,
        "the consumer's own handle comes back"
    );
    assert!(ring.engine().core.target(IoTargetKey(2)).is_none());
    assert!(
        ring.engine().backend.target(IoTargetKey(2)).is_none(),
        "nothing was attached"
    );
    ring.remove_target(IoTargetKey(1)).expect("idle");
    ring.add_target(IoTargetKey(2), error.target)
        .expect("a place was let go");
}

#[test]
fn an_add_target_past_the_domains_declared_is_refused_and_gives_the_instance_none_of_them() {
    let (a, b) = (TempFile::new("cap-ad-a"), TempFile::new("cap-ad-b"));
    let mut ring = Dioring::<Vec<u8>>::new(with(
        declared(|c| c.domains = 1),
        setup(vec![file(1, &a, &["x"])], Vec::new(), None),
    ))
    .expect("build")
    .0;
    let error = ring
        .add_target(IoTargetKey(2), target(b.open(), &["y"]))
        .expect_err("a second domain");
    assert_eq!(
        error.cause,
        AddTargetCause::AtCapacity(at(Limit::Domains, 1))
    );
    let imported = ring
        .import_failure(ImportScope::Domains(vec![super::domain("y")]))
        .expect("an import by domain");
    assert_eq!(imported.unmatched, [super::domain("y")]);
}

#[test]
fn a_mint_past_the_lineages_declared_is_refused_and_a_retire_lifts_it() {
    let mut ring = Dioring::<Vec<u8>>::new(with(
        declared(|c| c.lineages = 2),
        setup(Vec::new(), Vec::new(), None),
    ))
    .expect("build")
    .0;
    let minted = ring.mint_lineage(None).expect("the default and one");
    assert_eq!(ring.mint_lineage(None).err(), Some(at(Limit::Lineages, 2)));
    ring.retire_lineage(minted).expect("nothing holds it");
    let _again = ring.mint_lineage(None).expect("a place was let go");
}

#[test]
fn an_import_past_the_imports_declared_is_refused() {
    let mut ring = Dioring::<Vec<u8>>::new(with(
        declared(|c| c.imports = 1),
        setup(Vec::new(), Vec::new(), None),
    ))
    .expect("build")
    .0;
    ring.import_failure(ImportScope::All).expect("room");
    assert_eq!(
        ring.import_failure(ImportScope::All).err(),
        Some(ImportError::AtCapacity(at(Limit::Imports, 1)))
    );
    assert_eq!(ring.failures().len(), 1, "nothing was recorded");
}

#[test]
fn a_push_past_the_operations_declared_is_refused_with_its_buffer() {
    let temp = super::pushes::temp(&[0; 16]);
    let mut ring = super::pushes::Ring::new(with(
        declared(|c| c.operations = 1),
        setup(
            vec![super::pushes::given(super::pushes::GIVEN, &temp)],
            Vec::new(),
            None,
        ),
    ))
    .expect("build")
    .0;
    ring.read(super::pushes::GIVEN, 0, vec![0; 4], 1)
        .expect("room");
    let error = ring
        .read(super::pushes::GIVEN, 0, vec![9; 4], 2)
        .expect_err("full");
    assert!(matches!(
        error.reason,
        PushRefusal::AtCapacity(AtCapacity {
            limit: Limit::Operations,
            declared: 1
        })
    ));
    assert_eq!(error.buffer, Some(vec![9; 4]));
    assert_eq!(error.context, 2);
}
