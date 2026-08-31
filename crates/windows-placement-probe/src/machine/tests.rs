// Copyright (c) 2026 Mike Grier
//! Tests for [`MachineDescription`](super::MachineDescription).
//!
//! These run against the real machine, because the whole point of the module is
//! what a real machine will say. They therefore assert *shape and policy* --
//! that suppression is honoured, that absence is distinguishable from
//! suppression, that nothing forbidden is read -- rather than any particular
//! value, which would be an assertion about whatever host ran the suite.

use super::{MachineDescription, VirtualisationHint};

#[test]
fn the_default_hint_is_not_a_claim_of_bare_metal() {
    // `NotDetected` must be the default rather than anything stronger: failing
    // to detect a hypervisor is not the same as establishing there is none, and
    // a reader deciding whether a submission could show NUMA rows depends on
    // that distinction.
    assert_eq!(
        VirtualisationHint::default(),
        VirtualisationHint::NotDetected
    );
    assert_eq!(
        VirtualisationHint::NotDetected.to_string(),
        "not detected",
        "the rendered form must not read as a positive claim"
    );
}

#[test]
fn reading_this_machine_answers_something() {
    // A smoke test with teeth: if the registry reads were wrong -- bad key
    // path, wrong value type, mishandled buffer -- every field would come back
    // empty at once, and that is what this catches.
    let described = MachineDescription::read(false);

    assert!(
        described.cpu_model.is_some() || described.os_build.is_some(),
        "no field could be read at all, which suggests the reads are broken \
         rather than that this host is unusually quiet: {described:?}"
    );
}

#[test]
fn the_cpu_model_when_present_looks_like_a_processor_name() {
    let described = MachineDescription::read(false);

    if let Some(model) = &described.cpu_model {
        assert!(!model.is_empty(), "an empty model must be reported as None");
        assert_eq!(model.trim(), model, "the model must arrive trimmed");
        assert!(
            model.chars().any(|c| c.is_ascii_alphabetic()),
            "a processor name with no letters is not a name: {model:?}"
        );
    }
}

#[test]
fn the_os_build_reports_the_real_major_version_and_not_the_legacy_string() {
    // **This test exists because the shape-only test below passed a wrong
    // answer.** `CurrentMajorVersionNumber` is a `REG_DWORD`; reading it as a
    // string is rejected for type, falls back to the legacy `CurrentVersion`
    // string, and reports a Windows 11 host as build `6.3.0.26200` -- dotted
    // numbers, entirely plausible, and false.
    //
    // The expected value is read here rather than written down, so this checks
    // the assembly against the registry rather than against a constant that
    // would itself go stale.
    let Some(major) = super::read_registry_u32(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "CurrentMajorVersionNumber",
    ) else {
        // Pre-Windows-10, where the legacy string genuinely is the answer.
        return;
    };

    let build = MachineDescription::read(false)
        .os_build
        .expect("a host that reports a major version must yield a build");

    assert!(
        build.starts_with(&format!("{major}.")),
        "os_build {build:?} does not begin with the registry's major version {major}"
    );
}

#[test]
fn the_os_build_when_present_is_dotted_numbers() {
    // Guards the assembly in `read_os_build`, which stitches several registry
    // values together and could silently produce something like "..".
    let described = MachineDescription::read(false);

    if let Some(build) = &described.os_build {
        let parts: Vec<&str> = build.split('.').collect();
        assert!(
            parts.len() >= 3,
            "an OS build should have at least three components: {build:?}"
        );
        assert!(
            parts.iter().all(|part| !part.is_empty()),
            "an OS build with an empty component means a missing registry \
             value was stitched in silently: {build:?}"
        );
        assert!(
            parts
                .iter()
                .all(|part| part.chars().all(|c| c.is_ascii_digit())),
            "an OS build component that is not a number: {build:?}"
        );
    }
}

#[test]
fn suppressing_the_model_withholds_it_and_records_that_it_was_withheld() {
    // The distinction that a bare `Option` would have lost. A collector must be
    // able to tell "the runner withheld this" from "the host would not say".
    let suppressed = MachineDescription::read(true);

    assert!(suppressed.cpu_model.is_none());
    assert!(suppressed.model_suppressed);
}

#[test]
fn not_suppressing_records_that_nothing_was_withheld() {
    let described = MachineDescription::read(false);

    assert!(
        !described.model_suppressed,
        "an unsuppressed read must not claim the model was withheld"
    );
}

#[test]
fn suppression_withholds_only_the_model() {
    // Suppression is a privacy switch, not a mute button: the fields it does
    // not cover must still be collected, or a suppressed submission would be
    // far less useful than the runner intended.
    let open = MachineDescription::read(false);
    let suppressed = MachineDescription::read(true);

    assert_eq!(open.os_build, suppressed.os_build);
    assert_eq!(open.virtualisation, suppressed.virtualisation);
}

#[test]
fn a_detected_hypervisor_names_itself() {
    // A bare "detected" would ask the reader to trust the heuristic. Naming the
    // string that matched lets them judge it -- which matters, because the
    // markers include manufacturer names that also ship real hardware.
    let described = MachineDescription::read(false);

    match described.virtualisation {
        VirtualisationHint::Detected => assert!(
            described.virtualisation_name.is_some(),
            "a detection with nothing named cannot be judged by a reader"
        ),
        _ => assert!(
            described.virtualisation_name.is_none(),
            "a name was recorded without a detection: {described:?}"
        ),
    }
}
