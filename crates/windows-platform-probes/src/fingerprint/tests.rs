// Copyright (c) Mike Grier.

//! Tests for the fingerprint's rendering.
//!
//! The rendering is what makes two runs comparable, so it is what is tested:
//! a format that changes shape between hosts, or that renders two different
//! machines identically, would silently defeat the whole point.
//!
//! Discovery itself is not tested here -- it reads the real machine, so an
//! assertion about its output would be an assertion about whatever hardware
//! happens to run the suite.

use windows_topology_sys::Provenance;

use super::Fingerprint;

/// The development host: 12 cores, no SMT, two L2 domains, two efficiency
/// classes, one NUMA node.
fn arm64_dev_host() -> Fingerprint {
    Fingerprint {
        arch: "aarch64",
        processors: 12,
        cores: 12,
        smt: false,
        partitioning_cache_level: Some(2),
        cache_domain_sizes: vec![6, 6],
        efficiency_classes: vec![(0, 6), (1, 6)],
        numa_node_sizes: vec![12],
        // These fixtures stand in for real hosts, so they render as real hosts
        // do -- without a taint prefix. The exact-string assertions below are
        // assertions about what those machines actually print.
        provenance: Provenance::Measured,
    }
}

/// The shape an 8C/16T homogeneous x64 host is expected to take.
fn x64_smt_host() -> Fingerprint {
    Fingerprint {
        arch: "x86_64",
        processors: 16,
        cores: 8,
        smt: true,
        partitioning_cache_level: Some(3),
        cache_domain_sizes: vec![16],
        efficiency_classes: vec![(0, 16)],
        numa_node_sizes: vec![16],
        provenance: Provenance::Measured,
    }
}

#[test]
fn the_development_host_renders_as_expected() {
    assert_eq!(
        arm64_dev_host().to_string(),
        "aarch64 12p/12c smt- L2[6,6] ec[0:6,1:6] numa[12]"
    );
}

#[test]
fn an_smt_host_renders_as_expected() {
    assert_eq!(
        x64_smt_host().to_string(),
        "x86_64 16p/8c smt+ L3[16] ec[0:16] numa[16]"
    );
}

#[test]
fn smt_is_stated_rather_than_left_to_be_inferred() {
    // `16p/8c` already implies SMT arithmetically, but the marker is what a
    // reader scans for, and the arithmetic does not survive a machine with
    // offline processors.
    assert!(x64_smt_host().to_string().contains("smt+"));
    assert!(arm64_dev_host().to_string().contains("smt-"));
}

#[test]
fn the_two_hosts_do_not_render_identically() {
    assert_ne!(
        arm64_dev_host().to_string(),
        x64_smt_host().to_string(),
        "two machines that express different placements must be distinguishable"
    );
}

#[test]
fn a_machine_no_cache_partitions_says_so() {
    let flat = Fingerprint {
        arch: "x86_64",
        processors: 4,
        cores: 4,
        smt: false,
        partitioning_cache_level: None,
        cache_domain_sizes: vec![4],
        efficiency_classes: vec![(0, 4)],
        numa_node_sizes: vec![4],
        provenance: Provenance::Measured,
    };
    assert!(
        flat.to_string().contains("L-[4]"),
        "an undivided machine must render distinctly from a divided one, got {flat}"
    );
    assert!(!flat.partitioned());
}

#[test]
fn heterogeneity_is_reported_from_the_classes() {
    assert!(arm64_dev_host().heterogeneous());
    assert!(!x64_smt_host().heterogeneous());
}

#[test]
fn partitioning_is_reported_from_the_domain_count() {
    assert!(arm64_dev_host().partitioned());
    assert!(
        !x64_smt_host().partitioned(),
        "one cache domain covering everything partitions nothing"
    );
}

#[test]
fn rendering_is_stable_across_calls() {
    // Canonical output is what lets two runs be compared by string equality,
    // so nothing in the rendering may depend on iteration order or on time.
    let host = arm64_dev_host();
    assert_eq!(host.to_string(), host.to_string());
}

#[test]
fn every_field_that_changes_the_answer_appears_in_the_render() {
    // A fingerprint that omitted a field would render two genuinely different
    // machines identically, which is worse than having no fingerprint: it
    // would license a comparison that is not valid.
    let base = arm64_dev_host();

    let mut fewer_cores = base.clone();
    fewer_cores.cores = 6;
    fewer_cores.processors = 6;

    let mut with_smt = base.clone();
    with_smt.smt = true;

    let mut different_cache = base.clone();
    different_cache.cache_domain_sizes = vec![4, 8];

    let mut different_classes = base.clone();
    different_classes.efficiency_classes = vec![(0, 12)];

    let mut different_numa = base.clone();
    different_numa.numa_node_sizes = vec![6, 6];

    let mut different_level = base.clone();
    different_level.partitioning_cache_level = Some(3);

    for (name, variant) in [
        ("core count", fewer_cores),
        ("smt", with_smt),
        ("cache domains", different_cache),
        ("efficiency classes", different_classes),
        ("numa nodes", different_numa),
        ("cache level", different_level),
    ] {
        assert_ne!(
            base.to_string(),
            variant.to_string(),
            "changing {name} must change the fingerprint"
        );
    }
}

#[test]
fn a_measured_host_renders_without_any_marker() {
    // Every fingerprint recorded in a checklist or design note before the
    // marker existed was measured, so this is what keeps those strings valid
    // and comparable rather than silently reinterpreted.
    let rendered = arm64_dev_host().to_string();

    assert!(!rendered.contains("!!"), "got {rendered}");
    assert!(rendered.starts_with("aarch64"), "got {rendered}");
}

#[test]
fn a_synthetic_host_is_marked_at_the_front() {
    let mut fabricated = x64_smt_host();
    fabricated.provenance = Provenance::Synthetic;

    let rendered = fabricated.to_string();

    assert!(
        rendered.starts_with("!!SYNTHETIC!! "),
        "the marker must lead, so a reader scanning a column cannot skip it: {rendered}"
    );
}

#[test]
fn a_restored_host_is_marked_and_says_which_kind_of_untrusted_it_is() {
    // Restored and synthetic are different claims -- one describes some real
    // machine, the other describes none -- and a reader deciding how much to
    // believe a number needs to know which.
    let mut loaded = x64_smt_host();
    loaded.provenance = Provenance::Restored;

    let rendered = loaded.to_string();

    assert!(rendered.starts_with("!!RESTORED!! "), "got {rendered}");
    assert!(!rendered.contains("SYNTHETIC"), "got {rendered}");
}

#[test]
fn an_untrusted_host_never_compares_equal_to_the_real_one_it_imitates() {
    // The specific bug the marker exists to prevent, and the reason it lives
    // inside the string rather than beside it. The fingerprint is documented as
    // canonical, so equality of the rendered form is a supported comparison --
    // which means a fabricated machine claiming the exact shape of a real one
    // must not produce the same string.
    let real = x64_smt_host();
    for untrusted in [Provenance::Synthetic, Provenance::Restored] {
        let mut imitation = x64_smt_host();
        imitation.provenance = untrusted;

        assert_eq!(
            imitation.processors, real.processors,
            "the fixtures must otherwise be identical for this test to mean anything"
        );
        assert_ne!(
            imitation.to_string(),
            real.to_string(),
            "{untrusted:?} rendered identically to a measured host"
        );
    }
}

#[test]
fn the_marker_is_the_only_difference_an_untrusted_host_renders() {
    // The taint must not disturb the shape it prefixes, or a tainted
    // fingerprint could not be compared against a real one at all -- which is
    // exactly what someone validating synthetic selection logic needs to do.
    let real = x64_smt_host();
    let mut fabricated = x64_smt_host();
    fabricated.provenance = Provenance::Synthetic;

    let rendered = fabricated.to_string();
    let stripped = rendered
        .strip_prefix("!!SYNTHETIC!! ")
        .expect("the marker must be a removable prefix");

    assert_eq!(stripped, real.to_string());
}

#[test]
fn a_fingerprint_read_from_this_machine_reports_itself_as_measured() {
    // Ties the rendering to the real path: `discover` goes through
    // `Topology::discover`, which is the only thing entitled to claim the
    // machine. If provenance ever stopped flowing, every probe banner would
    // quietly start printing a taint marker -- or worse, stop printing one.
    let fingerprint = Fingerprint::discover().expect("this machine must be discoverable");

    assert!(fingerprint.provenance.is_measured());
    assert!(!fingerprint.to_string().contains("!!"));
}

#[test]
fn a_fingerprint_built_from_a_hand_made_topology_is_not_measured() {
    // The path a synthetic host takes. `Topology::default` is untrusted by
    // construction, and `from_topology` must carry that through rather than
    // inventing an answer.
    let fingerprint = Fingerprint::from_topology(&windows_topology_sys::Topology::default());

    assert!(!fingerprint.provenance.is_measured());
    assert!(
        fingerprint.to_string().starts_with("!!SYNTHETIC!! "),
        "got {fingerprint}"
    );
}

#[test]
fn the_banner_carries_whatever_the_fingerprint_says() {
    // On this machine the topology is real, so the banner must be clean. The
    // point is that the banner is not a second, independent rendering that
    // could drift from the fingerprint's own.
    let banner = super::banner_line();
    let fingerprint = Fingerprint::discover().expect("this machine must be discoverable");

    assert!(banner.starts_with("host:  "), "got {banner}");
    assert!(
        banner.contains(&fingerprint.to_string()),
        "the banner must embed the fingerprint verbatim: {banner}"
    );
    assert!(
        !banner.contains("!!"),
        "a real machine's banner must carry no taint marker: {banner}"
    );
}
