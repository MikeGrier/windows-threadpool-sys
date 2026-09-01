// Copyright (c) 2026 Mike Grier
//! The human-readable report, rendered from the record.
//!
//! # Why this renders the record rather than the measurement
//!
//! The obvious implementation walks the [`Observation`](crate::core_affinity::Observation)
//! that the record was also built from. That gives two renderings of one run
//! which can disagree -- and this investigation has already been bitten three
//! times by exactly that: a probe that printed a fixed conclusion contradicting
//! its own table, a table that omitted the row its interpretation quoted, and a
//! classification that silently merged two placements.
//!
//! So the report is a function of the
//! [`SubmissionRecord`](crate::record::SubmissionRecord), full stop. If the
//! record is wrong the report is wrong in the same way, which is what makes the
//! printed text worth reading before deciding whether to send the file.

use std::fmt::Write as _;

use crate::record::SubmissionRecord;

/// Render the report a runner sees.
#[must_use]
pub fn render(record: &SubmissionRecord) -> String {
    let mut out = String::new();
    render_header(&mut out, record);
    render_machine(&mut out, record);
    render_placements(&mut out, record);
    render_by_class(&mut out, record);
    render_node_hops(&mut out, record);
    render_trust(&mut out, record);
    out
}

fn render_header(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(
        out,
        "== what does thread placement cost on this machine? =="
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "host:      {}", record.host);
    let _ = writeln!(out, "build:     {}", record.build);
    let _ = writeln!(out, "recorded:  {}", record.recorded_at);
    let _ = writeln!(out, "schema:    {}", record.schema_version);
}

fn render_machine(out: &mut String, record: &SubmissionRecord) {
    let machine = &record.machine;
    let _ = writeln!(out);
    let _ = writeln!(out, "-- the machine --");
    let _ = writeln!(
        out,
        "cpu:            {}",
        match (&machine.cpu_model, machine.model_suppressed) {
            (Some(model), _) => model.clone(),
            // Withheld and unreadable are different facts and are shown as
            // such: a reader of a submission must not have to guess which.
            (None, true) => "(withheld by the runner)".to_owned(),
            (None, false) => "(this host would not say)".to_owned(),
        }
    );
    let _ = writeln!(
        out,
        "os build:       {}",
        machine.os_build.as_deref().unwrap_or("(unknown)")
    );
    let _ = write!(out, "virtualisation: {}", machine.virtualisation);
    match &machine.virtualisation_name {
        Some(name) => {
            let _ = writeln!(out, " ({name})");
        }
        None => {
            let _ = writeln!(out);
        }
    }
}

fn render_placements(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(out);
    let _ = writeln!(out, "-- the handoff, by placement --");

    if record.placements.is_empty() {
        let _ = writeln!(
            out,
            "  Nothing was measured, which is a fault rather than a finding."
        );
        return;
    }

    // Says what the table covers, because the label alone does not. Each row is
    // one direction on one representative pair, with the ring left wherever the
    // allocator put it -- not an average over an edge. The hop table below is
    // where direction and ring placement are varied deliberately.
    let _ = writeln!(
        out,
        "  One direction per row (prod -> cons), ring left where it fell."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{:<26} {:<10} {:>12} {:>12}",
        "placement", "strategy", "ns/item", "batch depth"
    );
    for entry in &record.placements {
        let _ = writeln!(
            out,
            "{:<26} {:<10} {:>12.1} {:>12.1}",
            entry.placement, entry.strategy, entry.nanos_per_item, entry.consumer_batch
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "the slice each row was measured on:");
    // One entry per *placement*, not per row. Every strategy of a placement runs
    // on the same processors, so listing them per row repeats each slice as many
    // times as there are strategies and says nothing new.
    let mut shown: Vec<&str> = Vec::new();
    for entry in &record.placements {
        if shown.contains(&entry.placement.as_str()) {
            continue;
        }
        shown.push(&entry.placement);
        // The slice goes on its own line rather than beside the label: a real
        // slice names two processors with five fields each, and a single line
        // carrying both a label and that reaches about 120 characters, which
        // wraps on a normal terminal and risks being reflowed on paste.
        let _ = writeln!(out, "  {}", entry.placement);
        let _ = writeln!(out, "      {}", entry.slice);
    }
}

fn render_by_class(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(out);
    let _ = writeln!(out, "-- the handoff, by efficiency class --");

    if record.by_class.is_empty() {
        // Same reasoning as the empty node-hop table: on a homogeneous machine
        // there is no second class to compare against, and saying so is a fact
        // about the host rather than a measurement that failed.
        let _ = writeln!(
            out,
            "  Every core on this machine reports the same efficiency class, so"
        );
        let _ = writeln!(
            out,
            "  there is no fast-against-slow comparison to draw. On a machine"
        );
        let _ = writeln!(
            out,
            "  with performance and efficiency cores this table has a row each."
        );
        return;
    }

    // Class first, because it is what distinguishes these rows: the pair inside
    // a class is same-class and same-cache by construction, so `placement` and
    // `strategy` repeat down the table and only the class and the numbers move.
    let _ = writeln!(
        out,
        "{:<8} {:<26} {:<10} {:>12} {:>12}",
        "class", "placement", "strategy", "ns/item", "batch depth"
    );
    for entry in &record.by_class {
        let _ = writeln!(
            out,
            "{:<8} {:<26} {:<10} {:>12.1} {:>12.1}",
            entry.producer_efficiency_class,
            entry.placement,
            entry.strategy,
            entry.nanos_per_item,
            entry.consumer_batch
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Higher class is the faster core. The values are only comparable"
    );
    let _ = writeln!(out, "  against each other on this machine.");
}

fn render_node_hops(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(out);
    let _ = writeln!(out, "-- the handoff, by NUMA node pair --");

    if record.node_hops.is_empty() {
        // Not an apology. Every machine measured by the author so far reports
        // one node, which is exactly why a submission from a multi-node host is
        // worth asking for -- so the empty case says what it means.
        let _ = writeln!(
            out,
            "  This machine has one NUMA node, so there is no node crossing to"
        );
        let _ = writeln!(
            out,
            "  measure. That is a fact about the host, not a failed measurement."
        );
        return;
    }

    // Three positions, three columns. `->` and not `<->`: the row describes a
    // direction, because the producer writes and the consumer reads, and the
    // ring sits on one node or the other while they do. A row that named only
    // the two endpoints would leave the reader unable to tell a remote write
    // from a remote read -- the two costs this table exists to separate.
    let _ = writeln!(
        out,
        "{:<12} {:<8} {:<10} {:>12} {:>12}",
        "prod -> cons", "ring on", "strategy", "ns/item", "batch depth"
    );
    for entry in &record.node_hops {
        let ring_on = match entry.memory_node {
            Some(node) => format!("node {node}"),
            // Reported, not hidden. A hop whose ring landed somewhere unknown
            // is still a measurement, but not of the pair it names.
            None => "unknown".to_owned(),
        };
        let _ = writeln!(
            out,
            "{:<12} {:<8} {:<10} {:>12.1} {:>12.1}",
            format!(
                "{} -> {}",
                entry.producer_numa_node, entry.consumer_numa_node
            ),
            ring_on,
            entry.strategy,
            entry.nanos_per_item,
            entry.consumer_batch
        );
    }
}

fn render_trust(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(out);
    let _ = writeln!(out, "-- how far to trust this --");

    if record.is_fully_trusted() {
        let _ = writeln!(
            out,
            "  An official build, reading this machine's real topology."
        );
    } else {
        let _ = writeln!(out, "  This run is marked, and here is why:");
        if !record.build.is_official() {
            let _ = writeln!(
                out,
                "  - the binary is not an official CI build ({})",
                record.build
            );
        }
        if !record.topology_provenance.is_measured() {
            let _ = writeln!(
                out,
                "  - the topology is {}, not read from this machine",
                record.topology_provenance
            );
        }
        let _ = writeln!(
            out,
            "  The numbers are still real; they simply cannot be traced the way"
        );
        let _ = writeln!(out, "  an official run can, so say so when you send them.");
    }

    // Stated on every run, not only untrusted ones. A long clean run is exactly
    // when someone is most tempted to read more into it than it says.
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  What this does NOT establish: anything about memory ordering. These"
    );
    let _ = writeln!(
        out,
        "  are timing measurements, and timing cannot catch a weakened ordering."
    );
}

#[cfg(test)]
mod tests;
