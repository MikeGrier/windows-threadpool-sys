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
            // Both, and equal: this fixture stands for rows that got the
            // placement they asked for. The redirect case, where they differ,
            // has its own test below.
            row.memory_node = Some(memory_node);
            row.requested_memory_node = Some(memory_node);
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
    // still a measurement, but not of the pair it names, and the report has to
    // admit that rather than leave a column reading as though it succeeded.
    let mut record = fully_populated();
    let mut rows = one_numa_edge();
    rows[0].memory_node = None;
    record.node_hops = rows;

    let text = render(&record);

    assert!(
        text.contains("did not get the memory they asked for"),
        "a hop with no achieved placement did not admit it:
{text}"
    );
    assert!(
        text.contains("could not determine"),
        "an unachievable placement must read differently from a redirected one:
{text}"
    );
}

#[test]
fn two_hops_redirected_to_one_node_stay_distinguishable() {
    // **The defect this guards.** Windows may satisfy a NUMA allocation on a
    // node other than the one requested, and the probe tolerates that rather
    // than failing. Keyed on the achieved node, the producer-local and
    // consumer-local rows for a pair then serialise and print identically, and
    // a reader sees two duplicate rows instead of the two placements the table
    // exists to separate.
    let mut record = fully_populated();
    let mut rows = one_numa_edge();
    // Both rows of the 0 -> 1 edge asked for different nodes; both landed on 0.
    rows[0].requested_memory_node = Some(0);
    rows[1].requested_memory_node = Some(1);
    rows[0].memory_node = Some(0);
    rows[1].memory_node = Some(0);
    record.node_hops = rows;

    let text = render(&record);

    for requested in [0, 1] {
        // Anchored at the start of the line so this counts *table rows* only.
        // The caveat below the table names the same pair and also mentions the
        // node it landed on, so a substring search matches it too -- which is
        // how the first draft of this test reported two rows for node 0.
        let matched = text.lines().filter(|line| {
            line.starts_with("0 -> 1") && line.contains(&format!("node {requested}"))
        });
        assert_eq!(
            matched.count(),
            1,
            "the row that asked for node {requested} is not identifiable:
{text}"
        );
    }
    // And the one that did not get what it asked for is called out.
    assert!(
        text.contains("did not get the memory they asked for"),
        "a redirected allocation was reported as though it succeeded:
{text}"
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

#[test]
fn the_by_class_rows_are_rendered_and_labelled_by_class() {
    // The defect this guards: `by_class` was measured, recorded, and then
    // dropped from the report, so an entire dimension of the run existed only
    // in the raw JSON. Rendering it is not enough on its own -- the rows of
    // that list agree on placement and strategy by construction, so the class
    // is the only thing distinguishing them and must appear.
    let mut record = fully_populated();
    let mut fast = record.by_class[0].clone();
    fast.producer_efficiency_class = 1;
    fast.consumer_efficiency_class = 1;
    fast.nanos_per_item = 4.25;
    record.by_class.push(fast);

    let text = render(&record);

    assert!(
        text.contains("efficiency class"),
        "the by-class section is missing entirely: {text}"
    );
    for entry in &record.by_class {
        assert!(
            text.contains(&format!("{:.1}", entry.nanos_per_item)),
            "the measurement for class {} is absent from the report: {text}",
            entry.producer_efficiency_class
        );
    }
    // Both classes named in the *class column*, so the two rows can be told
    // apart. Deliberately not a bare `contains("0")`: every row carries times
    // like "10.5", so a substring search would pass on a table that never
    // printed a class at all.
    // Bounded at the next heading. Without that, the scan runs on into the node
    // hop table, whose rows begin "0 -> 0" and so also start with a parsable
    // number -- which is how the first draft of this test read a third class
    // that the by-class table never printed.
    let section = text
        .split("-- the handoff, by efficiency class --")
        .nth(1)
        .expect("the by-class section must exist")
        .split(
            "
--",
        )
        .next()
        .expect("split always yields at least one part");
    let classes: Vec<&str> = section
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|first| first.parse::<u8>().is_ok())
        .collect();
    assert_eq!(
        classes,
        vec!["0", "1"],
        "the class column must name each row's class: {section}"
    );
}

#[test]
fn a_homogeneous_machine_says_why_there_is_no_class_comparison() {
    // Same contract as the single-node hop table: an empty section must read as
    // a fact about the host, not as a measurement that failed.
    let mut record = fully_populated();
    record.by_class.clear();

    let text = render(&record);

    assert!(text.contains("same efficiency class"), "got {text}");
    assert!(
        !text.to_lowercase().contains("error"),
        "a homogeneous machine was reported as an error: {text}"
    );
}
