// Copyright (c) 2026 Mike Grier
//! Tests for [`MachineDescription`](super::MachineDescription).
//!
//! These run against the real machine, because the whole point of the module is
//! what a real machine will say. They therefore assert *shape and policy* --
//! that suppression is honoured, that absence is distinguishable from
//! suppression, that nothing forbidden is read -- rather than any particular
//! value, which would be an assertion about whatever host ran the suite.

use super::{MachineDescription, VirtualisationHint, classify_virtualisation};
use crate::redaction::MetadataPolicy;

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
    let described = MachineDescription::read(MetadataPolicy::included());

    assert!(
        described.cpu_model.is_some() || described.os_build.is_some(),
        "no field could be read at all, which suggests the reads are broken \
         rather than that this host is unusually quiet: {described:?}"
    );
}

#[test]
fn the_cpu_model_when_present_looks_like_a_processor_name() {
    let described = MachineDescription::read(MetadataPolicy::included());

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

    let build = MachineDescription::read(MetadataPolicy::included())
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
    let described = MachineDescription::read(MetadataPolicy::included());

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
    let suppressed = MachineDescription::read(MetadataPolicy::included().without_cpu_model());

    assert!(suppressed.cpu_model.is_none());
    assert!(suppressed.model_suppressed);
}

#[test]
fn not_suppressing_records_that_nothing_was_withheld() {
    let described = MachineDescription::read(MetadataPolicy::included());

    assert!(
        !described.model_suppressed,
        "an unsuppressed read must not claim the model was withheld"
    );
    assert!(
        !described.os_build_suppressed,
        "an unsuppressed read must not claim the os build was withheld"
    );
    assert_ne!(
        described.virtualisation,
        VirtualisationHint::Suppressed,
        "an unsuppressed read must not claim the hint was withheld"
    );
}

#[test]
fn suppressing_the_model_withholds_only_the_model() {
    // The subtraction is a scalpel, not a mute button: the fields it does not
    // cover must still be collected, or an opted-in submission that withheld a
    // name would be far less useful than the runner intended.
    let open = MachineDescription::read(MetadataPolicy::included());
    let suppressed = MachineDescription::read(MetadataPolicy::included().without_cpu_model());

    assert_eq!(open.os_build, suppressed.os_build);
    assert_eq!(open.virtualisation, suppressed.virtualisation);
    assert!(!suppressed.os_build_suppressed);
}

#[test]
fn the_default_policy_withholds_every_secondary_field() {
    // **The behaviour M36.2 exists for.** A run that asks for nothing must send
    // nothing but the measurement, and must say so in every field rather than
    // leaving a reader to infer it from a blank.
    let described = MachineDescription::read(MetadataPolicy::default());

    assert_eq!(described.cpu_model, None);
    assert!(described.model_suppressed);
    assert_eq!(described.os_build, None);
    assert!(described.os_build_suppressed);
    assert_eq!(described.virtualisation, VirtualisationHint::Suppressed);
    assert_eq!(described.virtualisation_name, None);
}

#[test]
fn a_withheld_hint_is_not_reported_as_a_negative_finding() {
    // The trap this variant exists to close. `NotDetected` is the enum's
    // default, so a withheld hint that fell back to it would tell a reader this
    // machine had been examined and found to be bare metal -- a claim nobody
    // made, on the field that decides whether a submission could ever have
    // shown NUMA rows.
    let described = MachineDescription::read(MetadataPolicy::redacted());

    assert_ne!(described.virtualisation, VirtualisationHint::NotDetected);
    assert_ne!(
        described.virtualisation,
        VirtualisationHint::Unknown,
        "a withheld hint must not blame the firmware either"
    );
    assert_eq!(VirtualisationHint::Suppressed.to_string(), "withheld");
}

#[test]
fn a_detected_hypervisor_names_itself() {
    // A bare "detected" would ask the reader to trust the heuristic. Naming the
    // string that matched lets them judge it -- which matters, because the
    // markers include manufacturer names that also ship real hardware.
    let described = MachineDescription::read(MetadataPolicy::included());

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

// ---------------------------------------------------------------------------
// Virtualisation detection.
//
// These use the strings real machines report, because the defect they guard was
// a rule that looked reasonable and was wrong about specific hardware: a vendor
// name that a hypervisor and a laptop both carry.
// ---------------------------------------------------------------------------

#[test]
fn a_physical_surface_is_not_reported_as_virtualised() {
    // **The false positive.** `Microsoft Corporation` is the manufacturer of a
    // Hyper-V guest and of every Surface, so matching the vendor marked real
    // hardware as a VM -- invisibly to the submitter, and unquestionable by
    // anyone reading the collected data later.
    let (hint, name) =
        classify_virtualisation(Some("Microsoft Corporation"), Some("Surface Pro 9"));

    assert_eq!(hint, VirtualisationHint::NotDetected, "got {name:?}");
    assert_eq!(name, None);
}

#[test]
fn a_hyper_v_guest_is_still_detected() {
    // The case that must keep working, and the reason the vendor marker was
    // there. This workspace's own host reports exactly these two strings.
    let (hint, name) =
        classify_virtualisation(Some("Microsoft Corporation"), Some("Virtual Machine"));

    assert_eq!(hint, VirtualisationHint::Detected);
    assert_eq!(
        name.as_deref(),
        Some("Microsoft Corporation Virtual Machine")
    );
}

#[test]
fn physical_hardware_from_a_cloud_vendor_is_not_virtualised() {
    // The same shape as the Surface case: `Google` names both a cloud and a
    // laptop, so only the product marker decides.
    let (physical, _) = classify_virtualisation(Some("Google"), Some("Pixelbook"));
    let (cloud, _) = classify_virtualisation(Some("Google"), Some("Google Compute Engine"));

    assert_eq!(physical, VirtualisationHint::NotDetected);
    assert_eq!(cloud, VirtualisationHint::Detected);
}

#[test]
fn the_common_hypervisors_are_detected_from_either_field() {
    for (manufacturer, product) in [
        ("VMware, Inc.", "VMware Virtual Platform"),
        ("innotek GmbH", "VirtualBox"),
        ("QEMU", "Standard PC (Q35 + ICH9, 2009)"),
        ("Xen", "HVM domU"),
        ("Parallels International", "Parallels Virtual Platform"),
        ("Amazon EC2", "t3.medium"),
    ] {
        let (hint, _) = classify_virtualisation(Some(manufacturer), Some(product));
        assert_eq!(
            hint,
            VirtualisationHint::Detected,
            "{manufacturer} / {product} was not detected"
        );
    }
}

#[test]
fn ordinary_hardware_is_left_alone() {
    for (manufacturer, product) in [
        ("Dell Inc.", "XPS 15 9520"),
        ("LENOVO", "20XW00"),
        ("ASUSTeK COMPUTER INC.", "ROG STRIX"),
        ("Apple Inc.", "MacBookPro18,3"),
    ] {
        let (hint, _) = classify_virtualisation(Some(manufacturer), Some(product));
        assert_eq!(
            hint,
            VirtualisationHint::NotDetected,
            "{manufacturer} / {product} was called a virtual machine"
        );
    }
}

#[test]
fn firmware_that_says_nothing_is_unknown_rather_than_physical() {
    // Unknown and NotDetected are different answers: one is "asked and told
    // no", the other is "could not ask", and a reader deciding how much to
    // trust a submission needs to tell them apart.
    let (hint, name) = classify_virtualisation(None, None);

    assert_eq!(hint, VirtualisationHint::Unknown);
    assert_eq!(name, None);
}

#[test]
fn one_readable_field_is_enough_to_answer() {
    let (hint, _) = classify_virtualisation(None, Some("VMware Virtual Platform"));
    assert_eq!(hint, VirtualisationHint::Detected);

    let (hint, _) = classify_virtualisation(Some("Dell Inc."), None);
    assert_eq!(hint, VirtualisationHint::NotDetected);
}
