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

use windows_topology_sys::Coherence;

use crate::machine::VirtualisationHint;
use crate::record::SubmissionRecord;

/// Where a runner is pointed when something needs a conversation.
///
/// **The repository, not the results thread.** A disagreement between the
/// platform's own tables is not a result, and posting it into the collection
/// thread would bury it among measurements. Both a discussion and an issue are
/// one click from here, so the runner picks whichever they are comfortable
/// with rather than being told which one this is.
///
/// Deliberately its own constant rather than reusing
/// `submission::DISCUSSION_URL`, which is gated behind the `serde` feature that
/// this module is not; a test pins that the two agree about the repository so
/// the pair cannot drift apart silently.
///
/// **That name is inline code and not an intra-doc link, for the same reason
/// the constant exists.** A link resolves under `--all-features` and dangles
/// under `--no-default-features`, so linking it broke the configuration this
/// crate's manifest advertises -- while the sentence doing the linking was
/// itself explaining that the target is gated and this module is not.
pub const REPOSITORY_URL: &str = "https://github.com/MikeGrier/windows-threadpool-sys";

/// Render the report a runner sees.
#[must_use]
pub fn render(record: &SubmissionRecord) -> String {
    let mut out = String::new();
    render_header(&mut out, record);
    render_machine(&mut out, record);
    render_placements(&mut out, record);
    render_by_class(&mut out, record);
    render_node_hops(&mut out, record);
    render_origin(&mut out, record);
    render_topology_disagreement(&mut out, record);
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
    let _ = writeln!(
        out,
        "recorded:  {}",
        match (&record.recorded_at, record.recorded_at_suppressed) {
            (Some(stamp), _) => stamp.as_str(),
            (None, true) => "(withheld)",
            // Unreachable from `SubmissionRecord::new`, which only drops the
            // timestamp by withholding it. Rendered rather than unwrapped
            // because every field of the record is public, and a report that
            // panicked on a hand-assembled record would be worse than one that
            // says what it found.
            (None, false) => "(unknown)",
        }
    );
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
        match (&machine.os_build, machine.os_build_suppressed) {
            (Some(build), _) => build.as_str(),
            (None, true) => "(withheld by the runner)",
            (None, false) => "(this host would not say)",
        }
    );
    // Parenthesised when withheld, so this column reads the same way as the two
    // rows above it. The hint's own `Display` stays a plain word, because it is
    // the rendering of a value rather than of this table's cell.
    let _ = writeln!(
        out,
        "virtualisation: {}",
        match (machine.virtualisation, &machine.virtualisation_name) {
            (VirtualisationHint::Suppressed, _) => "(withheld by the runner)".to_owned(),
            (hint, Some(name)) => format!("{hint} ({name})"),
            (hint, None) => hint.to_string(),
        }
    );
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
        // **Empty does not mean homogeneous, and it must not claim to.** The
        // measurement needs a pair in one class on two different cores that
        // share a cache domain, and skips any class that cannot supply one --
        // a singleton class, or one whose cores sit in different cache
        // domains. A heterogeneous machine reaches here too, and telling its
        // owner that every core reports the same class is simply false.
        //
        // The record already knows which case this is, so it is asked rather
        // than guessed at.
        if record.host.efficiency_classes.len() <= 1 {
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
        } else {
            let _ = writeln!(
                out,
                "  This machine reports {} efficiency classes, but none of them could",
                record.host.efficiency_classes.len()
            );
            let _ = writeln!(
                out,
                "  supply a comparable pair: the measurement needs two cores of the"
            );
            let _ = writeln!(
                out,
                "  same class sharing a cache domain, and a class with a single core"
            );
            let _ = writeln!(
                out,
                "  -- or with its cores split across caches -- cannot provide one."
            );
            let _ = writeln!(
                out,
                "  That is a fact about this machine's layout, not a failed run."
            );
        }
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
        // **The requested node, because that is what identifies the row.** Two
        // rows for one directed pair differ only in where the ring was asked to
        // go; printing where it actually landed made them identical whenever
        // Windows redirected an allocation, which is exactly the case a reader
        // most needs to see. Any disagreement is called out under the table.
        let ring_on = match entry.requested_memory_node {
            Some(node) => format!("node {node}"),
            None => "unspecified".to_owned(),
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

    // A redirected allocation is a caveat on the row, not a footnote: the row
    // is labelled with the placement it asked for, and if the memory went
    // somewhere else then it did not measure that placement at all. Windows is
    // permitted to satisfy the request elsewhere, so this is an expected
    // outcome to disclose rather than an error to hide.
    let redirected = record
        .node_hops
        .iter()
        .filter(|entry| entry.memory_node != entry.requested_memory_node);
    let mut said_anything = false;
    for entry in redirected {
        if !said_anything {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "  Some rows did not get the memory they asked for, so they do not"
            );
            let _ = writeln!(out, "  measure the placement they name:");
            said_anything = true;
        }
        let landed = match entry.memory_node {
            Some(node) => format!("node {node}"),
            None => "somewhere this run could not determine".to_owned(),
        };
        let _ = writeln!(
            out,
            "  - {} -> {} ({}) asked for node {} and got {landed}",
            entry.producer_numa_node,
            entry.consumer_numa_node,
            entry.strategy,
            entry
                .requested_memory_node
                .map_or_else(|| "?".to_owned(), |node| node.to_string()),
        );
    }
}

/// Say what this result can be traced back to.
///
/// # Deliberately not "how far to trust this"
///
/// The question here is narrow and mechanical: was the binary built by CI from
/// a named commit, and was the topology read from the host rather than fed in.
/// Framing that as trust invites a much larger conversation -- about honesty,
/// about authentication -- that this section does not have and cannot settle,
/// since the build stamp is self-reported. Saying where a result came from is
/// the claim that is actually being made.
fn render_origin(out: &mut String, record: &SubmissionRecord) {
    let _ = writeln!(out);
    let _ = writeln!(out, "-- where this result came from --");
    if record.is_fully_traceable() {
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

    // Stated on every run, not only marked ones. A long clean run is exactly
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

/// Say that this machine described itself two ways, and offer a way to help.
///
/// # Nothing at all on the ordinary run
///
/// [`Coherence::Agreed`] and [`Coherence::NotCollected`] print nothing. A
/// section saying "the two sources agreed" would be noise on every run that
/// ever happens, and noise is what a reader learns to skip -- including on the
/// run where this matters.
///
/// # Informative, not coercive
///
/// This is a **report of something detected**, followed by an offer. It is
/// deliberately not a request, not a prompt, and not phrased so that declining
/// feels like a failure to help: the runner is already doing this project a
/// favour, and the result they have is a valid submission exactly as it stands.
/// The text says so in as many words, because a reader who feels chased is a
/// reader who closes the window.
///
/// What it does say is what would be *learned*. Neither side can be identified
/// from here -- the platform's description of this hardware may be
/// inconsistent, or this tool may read it wrongly -- and telling those apart
/// needs a second pair of eyes on a record from the machine that showed it.
/// That is a fact about the situation, so it can be stated plainly without
/// asking for anything.
fn render_topology_disagreement(out: &mut String, record: &SubmissionRecord) {
    /// `"s"` unless there is exactly one. A count printed as "1 processor(s)"
    /// is the sort of thing a reader notices instead of the sentence.
    const fn plural(count: usize) -> &'static str {
        if count == 1 { "" } else { "s" }
    }

    let Coherence::Disagreed {
        walk_only,
        cpu_sets_only,
        attempts,
    } = &record.topology_coherence
    else {
        return;
    };

    let _ = writeln!(out);
    let _ = writeln!(out, "-- this machine described itself two ways --");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Windows describes processors through two independent interfaces, and"
    );
    let _ = writeln!(
        out,
        "  on this machine they disagreed about which processors exist. The two"
    );
    let _ = writeln!(
        out,
        "  readings were repeated {attempts} times and never agreed, so this is a standing"
    );
    let _ = writeln!(
        out,
        "  difference between the platform's own tables rather than one reading"
    );
    let _ = writeln!(out, "  catching the machine mid-change.");
    let _ = writeln!(out);
    // Counts, not identities. Enough that "a mismatch" is a concrete thing a
    // reader can see rather than a word, and the record beside this carries the
    // processors themselves for anyone who goes looking.
    let _ = writeln!(
        out,
        "  The relationship walk reported {} processor{} the CPU-set enumeration",
        walk_only.len(),
        plural(walk_only.len())
    );
    let _ = writeln!(
        out,
        "  did not; the CPU-set enumeration reported {} the walk did not. The",
        cpu_sets_only.len()
    );
    let _ = writeln!(out, "  record below names them individually.");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Your measurements are unaffected: every row above was timed on"
    );
    let _ = writeln!(
        out,
        "  processors this run pinned and verified, and the numbers are real."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Two things could produce this, and they cannot be told apart from"
    );
    let _ = writeln!(
        out,
        "  here: the platform's description of this hardware may be inconsistent,"
    );
    let _ = writeln!(
        out,
        "  or this tool may be reading it wrongly. Either is worth knowing, and"
    );
    let _ = writeln!(
        out,
        "  the answer would apply to everyone with hardware like yours."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  If you would like to help work out which, you can start a discussion"
    );
    let _ = writeln!(out, "  or open an issue at");
    let _ = writeln!(out, "    {REPOSITORY_URL}");
    let _ = writeln!(
        out,
        "  and mention that you saw this message. What helps most is a record"
    );
    // The advice a runner cannot act on is the advice not to give. A record that
    // already names its OS build does not need to be produced again, and saying
    // otherwise would read as though the flag they passed had not worked.
    if record.machine.os_build_suppressed {
        let _ = writeln!(
            out,
            "  run with --include-metadata, because the OS build and hypervisor are"
        );
        let _ = writeln!(
            out,
            "  what tie a disagreement like this to a particular platform version."
        );
        let _ = writeln!(
            out,
            "  That is a separate run and a separate decision; the maintainers can"
        );
        let _ = writeln!(
            out,
            "  arrange a way to share it that does not post it publicly."
        );
    } else {
        let _ = writeln!(
            out,
            "  like this one, which already names the OS build and hypervisor -- the"
        );
        let _ = writeln!(
            out,
            "  things that tie a disagreement to a particular platform version. The"
        );
        let _ = writeln!(
            out,
            "  maintainers can arrange a way to share it that does not post it"
        );
        let _ = writeln!(out, "  publicly.");
    }
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  None of that is required. The result you already have is a valid"
    );
    let _ = writeln!(
        out,
        "  submission and is worth sending exactly as it stands."
    );
}

#[cfg(test)]
mod tests;
