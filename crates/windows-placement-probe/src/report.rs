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
//! So the report is a function of the [`SubmissionRecord`], full stop. If the
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
    for entry in &record.placements {
        let _ = writeln!(out, "  {:<26} {}", entry.placement, entry.slice);
    }
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

    let _ = writeln!(
        out,
        "{:<14} {:<10} {:>12} {:>12}",
        "node pair", "strategy", "ns/item", "batch depth"
    );
    for entry in &record.node_hops {
        let _ = writeln!(
            out,
            "{:<14} {:<10} {:>12.1} {:>12.1}",
            format!(
                "{} <-> {}",
                entry.producer_numa_node, entry.consumer_numa_node
            ),
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
