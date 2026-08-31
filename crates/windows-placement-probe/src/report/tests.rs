// Copyright (c) 2026 Mike Grier
//! Tests for [`render`](super::render).
//!
//! These assert that the report *reflects the record*, not that it contains any
//! particular wording. Asserting exact prose would make the text unchangeable
//! and would not check the property that matters, which is that the two cannot
//! disagree.

use windows_topology_sys::Provenance;

use super::render;
use crate::build_identity::BuildSource;
use crate::machine::VirtualisationHint;
use crate::record::tests::fully_populated;

#[test]
fn the_report_shows_the_values_the_record_carries() {
    // The core property: a reader comparing the printed text against the file
    // must see the same numbers, because the text is a function of the file.
    let record = fully_populated();
    let text = render(&record);

    assert!(text.contains(&record.recorded_at), "timestamp missing");
    assert!(
        text.contains(&record.schema_version.to_string()),
        "schema version missing"
    );
    assert!(
        text.contains("Example CPU"),
        "the cpu model in the record is absent from the report"
    );
    assert!(
        text.contains("10.0.26200.9168"),
        "the os build in the record is absent from the report"
    );
    for entry in &record.placements {
        assert!(
            text.contains(&entry.placement),
            "placement {} is absent from the report",
            entry.placement
        );
        assert!(
            text.contains(&entry.slice),
            "the slice for {} is absent, so a number cannot be traced to its processors",
            entry.placement
        );
    }
}

#[test]
fn a_withheld_model_reads_differently_from_an_unreadable_one() {
    // The distinction the record keeps must survive into the text, or the
    // report would flatten two different facts into one blank.
    let mut withheld = fully_populated();
    withheld.machine.cpu_model = None;
    withheld.machine.model_suppressed = true;

    let mut unreadable = fully_populated();
    unreadable.machine.cpu_model = None;
    unreadable.machine.model_suppressed = false;

    let withheld = render(&withheld);
    let unreadable = render(&unreadable);

    assert!(withheld.contains("withheld"), "got {withheld}");
    assert!(!unreadable.contains("withheld"), "got {unreadable}");
}

#[test]
fn a_single_node_machine_says_why_there_are_no_hops() {
    // Every host measured so far is single-node, so this is the common case and
    // must not read as a failure -- a runner who thinks the tool broke will not
    // send the file.
    let mut record = fully_populated();
    record.node_hops.clear();

    let text = render(&record);

    assert!(text.contains("one NUMA node"), "got {text}");
    // Deliberately not a substring search for "failed measurement": the report
    // says "not a failed measurement", and an assertion that matched inside
    // that denial would fail on correct text. An earlier revision of this test
    // did exactly that.
    assert!(
        !text.to_lowercase().contains("error"),
        "an ordinary single-node machine was reported as an error: {text}"
    );
    assert!(
        text.contains("fact about the host"),
        "the empty case must explain itself rather than merely being empty: {text}"
    );
}

#[test]
fn a_fully_trusted_run_is_not_marked() {
    let text = render(&fully_populated());

    assert!(text.contains("official build"), "got {text}");
}

#[test]
fn an_untrusted_run_names_each_reason_separately() {
    // "Something is wrong" is not actionable. A reader triaging a surprising
    // submission needs to know whether the build or the topology was the
    // problem, and both can be true at once.
    let mut record = fully_populated();
    record.build.source = BuildSource::Local;
    record.topology_provenance = Provenance::Synthetic;
    record.host.provenance = Provenance::Synthetic;

    let text = render(&record);

    assert!(text.contains("not an official CI build"), "got {text}");
    assert!(text.contains("not read from this machine"), "got {text}");
}

#[test]
fn the_ordering_caveat_is_stated_even_on_a_clean_run() {
    // A long clean run is exactly when someone is most tempted to read it as
    // validation of something it never touched.
    let text = render(&fully_populated());

    assert!(
        text.contains("memory ordering"),
        "the ordering caveat must appear on every run, including trusted ones"
    );
}

#[test]
fn a_detected_hypervisor_is_named_in_the_report() {
    let text = render(&fully_populated());

    assert!(text.contains("Example Hypervisor"), "got {text}");
}

#[test]
fn a_report_with_no_measurements_calls_that_a_fault() {
    // An empty table with no comment would read as "measured, nothing to say".
    let mut record = fully_populated();
    record.placements.clear();

    let text = render(&record);

    assert!(text.contains("fault"), "got {text}");
}

#[test]
fn changing_a_record_changes_the_report() {
    // Guards against a report that renders constants. If this ever passes with
    // identical text, the report has stopped being a function of the record.
    let record = fully_populated();
    let mut changed = record.clone();
    changed.placements[0].nanos_per_item = 999.9;

    assert_ne!(render(&record), render(&changed));
    assert!(render(&changed).contains("999.9"));
}

#[test]
fn a_virtualisation_hint_is_not_rendered_as_a_certainty() {
    let mut record = fully_populated();
    record.machine.virtualisation = VirtualisationHint::NotDetected;
    record.machine.virtualisation_name = None;

    let text = render(&record);

    assert!(
        text.contains("not detected"),
        "a negative must read as 'not detected' rather than as 'bare metal': {text}"
    );
    assert!(!text.contains("bare metal"), "got {text}");
}

/// The four rows one NUMA edge produces: two directions, each at two ring
/// placements.
///
/// The shared fixture's "hop" has both endpoints on node 0, which is not a
/// crossing at all -- it is there to populate the array, not to describe one.
/// The hop table is the part of the report this workspace's hardware cannot
/// exercise, so the fixture has to be the thing that is realistic.
fn one_numa_edge() -> Vec<crate::record::MeasurementRecord> {
    let mut rows = Vec::new();
    for (producer_node, consumer_node) in [(0_u32, 1_u32), (1, 0)] {
        for memory_node in [producer_node, consumer_node] {
            let mut row = fully_populated().node_hops[0].clone();
            row.placement = "cross NUMA node".to_owned();
            row.producer_numa_node = producer_node;
            row.consumer_numa_node = consumer_node;
            row.memory_node = Some(memory_node);
            // Distinct per row, so a report that collapses two rows into one is
            // visible rather than merely suspected.
            row.nanos_per_item = 100.0 + f64::from(producer_node) * 10.0 + f64::from(memory_node);
            rows.push(row);
        }
    }
    rows
}

#[test]
fn every_row_of_a_numa_edge_is_separately_identifiable() {
    // The defect this guards. Four measurements of one edge differ only in
    // direction and ring placement, so a table that prints neither renders four
    // rows a reader cannot tell apart -- and the two quantities the table exists
    // to separate, remote write and remote read, are lost in the middle of it.
    let mut record = fully_populated();
    record.node_hops = one_numa_edge();

    let text = render(&record);

    for (from, to, memory) in [(0, 1, 0), (0, 1, 1), (1, 0, 0), (1, 0, 1)] {
        // Two tokens on one line rather than a formatted row: asserting the
        // exact spacing would make this a test of the column widths, which are
        // free to change, instead of a test that the row is identifiable.
        let matched = text.lines().filter(|line| {
            line.contains(&format!("{from} -> {to}")) && line.contains(&format!("node {memory}"))
        });
        assert_eq!(
            matched.count(),
            1,
            "the report does not distinguish {from} -> {to} with the ring on node {memory}:
{text}"
        );
    }
}

#[test]
fn a_hop_reads_as_a_direction_and_not_as_a_link() {
    // `<->` was the earlier rendering, and it was wrong in a way that read as
    // correct: it says the row describes a link, when the row describes one
    // side writing and the other reading across it.
    let mut record = fully_populated();
    record.node_hops = one_numa_edge();

    let text = render(&record);

    assert!(
        !text.contains("<->"),
        "a hop is still rendered as an undirected link:\n{text}"
    );
}

#[test]
fn a_hop_whose_ring_could_not_be_placed_says_so() {
    // Not a hidden caveat. A hop measured with the ring on an unknown node is
    // still a measurement, but not of the pair it names, and the row has to
    // admit that rather than leave a blank column reading as a zero.
    let mut record = fully_populated();
    let mut rows = one_numa_edge();
    rows[0].memory_node = None;
    record.node_hops = rows;

    let text = render(&record);

    assert!(
        text.contains("unknown"),
        "a hop with no achieved placement did not admit it:\n{text}"
    );
}

#[test]
fn the_placement_table_says_it_covers_one_direction() {
    // Without this line the placement rows read as though they summarised a
    // placement, when each is a single direction with the ring wherever the
    // allocator left it.
    let text = render(&fully_populated());

    assert!(
        text.contains("One direction per row"),
        "the placement table does not say what it covers:\n{text}"
    );
}
