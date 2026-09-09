// Copyright (c) Mike Grier.

//! Prints the machine's processor topology, and how many execution domains each
//! candidate partitioning policy would produce on it.
//!
//! **An experiment, not a component.** These probes measure platform behaviour
//! and are not for production use. Do not call them from production code, and
//! do not lift a technique out of here. See this crate's DESIGN-NOTES.md.
//!
//! Running this on every CI build is deliberate: hosted runners are a
//! heterogeneous fleet, so the accumulated output is a slow survey of what real
//! machines look like. The line tagged `x-probe-topology` is emitted as a single
//! JSON object so those results can be mined out of build logs mechanically
//! rather than read by eye.

use windows_placement_probe::fingerprint::Fingerprint;
use windows_platform_probes::report::emit_report;
use windows_platform_probes::topology::measure;
use windows_platform_probes::topology_report::{attribution, report, report_unmeasured};

fn main() {
    // `emit_report`, like every other probe. This used to call `emit` with a
    // fully composed `String`, which gave up the one thing `emit_report` is for:
    // printing what was already established when a later step panics. The three
    // host reads below are exactly the steps that can, so the bypass removed the
    // protection at the only place it would have been used.
    emit_report(render);
}

/// The probe's whole report, as text.
fn render(out: &mut String) {
    // The only place that reads the host. The text is composed in the library so
    // every branch of it can be driven from a test -- see `topology_report`.
    //
    // The banner is read FIRST and passed in. It runs a topology discovery of
    // its own, so leaving it to the renderer made the library half depend on a
    // host it claimed not to need.
    //
    // It is read AGAIN afterwards, and the pair bracketed, for the same reason
    // `measure` brackets its counters: the fingerprint is a topology rendering,
    // not a name, so a machine that changed across the run would otherwise print
    // one shape above a body describing another. `attribution` decides what that
    // pair means; both readings sit outside `measure`'s own bracket, which is
    // what makes them a wider window than it and worth closing separately.
    //
    // `Fingerprint::discover` rather than `banner_line`, so a failed read stays
    // distinguishable from a host that moved. Rendering goes through
    // `banner_line_for`, which keeps this probe's banner the same shape as every
    // other probe's.
    let before = Fingerprint::discover();
    let measured = measure();
    let after = Fingerprint::discover();
    let banner = attribution(&before, &after);
    let text = match measured {
        Ok(observation) => report(&banner, &observation),
        Err(error) => report_unmeasured(&banner, &error),
    };
    out.push_str(&text);
}
