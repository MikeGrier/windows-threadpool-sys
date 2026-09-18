// Copyright (c) Mike Grier.

//! The topology probe's report, as text.
//!
//! A separate module from the binary because it was untestable there: `render`
//! called [`crate::topology::measure`] itself, so every branch needed a live
//! host and none could be driven from a test. A mutation sweep found 13 of 13
//! mutants surviving -- `render` could return `"xyzzy"` and the suite stayed
//! green -- and the survivors were exactly the claims that cost the most review
//! rounds: the "no L3 at all" note, the caveat gate on an absent partitioning
//! answer, and the heterogeneous-core note. Each had been checked by running
//! the binary and reading the output, which nothing repeats on a later change.
//!
//! The same division as [`crate::topology::observe`] out of
//! [`crate::topology::measure`], for the same reason.

use std::fmt::Write as _;
use std::io;

use windows_placement_probe::fingerprint::{Fingerprint, banner_line_for};

use crate::row::{Row, Shape, Value};

/// What every entry in one of the four diagnostic lists must carry.
///
/// `code` is the stable discriminant a survey groups by, so an entry without one
/// is unmineable even though it is valid JSON. The payload beside it is
/// per-variant and so is not a member of this schema -- it is checked against
/// the typed publisher by
/// `the_row_carries_each_diagnostic_entry_whole_and_not_only_its_code`.
const CODED: &[(&str, Shape)] = &[("code", Shape::Text)];

/// Declares a row schema once, as names WITH shapes, and derives the key list.
///
/// One list, so a key cannot gain a shape without gaining a name or the reverse.
/// The alternative -- a `_KEYS` const beside a `_SHAPES` const -- is two
/// statements of one schema, which is the restatement this crate keeps paying
/// for.
macro_rules! row_schema {
    (
        $(#[$keys_doc:meta])* $keys:ident,
        $(#[$shapes_doc:meta])* $shapes:ident { $($name:literal => $shape:expr,)+ }
    ) => {
        $(#[$keys_doc])*
        pub const $keys: &[&str] = &[$($name,)+];

        $(#[$shapes_doc])*
        pub const $shapes: &[(&str, Shape)] = &[$(($name, $shape),)+];
    };
}

row_schema!(
    /// Every key a MEASURED topology row carries, in order.
    ///
    /// **This is the contract, not a census of the code.** The anti-census rule
    /// this crate keeps relearning is about restating facts that can be DERIVED
    /// -- a count of placeholders, a tally of variants. A schema is not
    /// derivable from anything: it IS the agreement with the survey that reads
    /// these rows, so writing it down is what makes it checkable at all.
    ///
    /// It was missing, and the gap was measured: with `packages` deleted from
    /// the builder entirely, the whole suite stayed green. `Row::keys` reports
    /// what the builder happened to supply, so a test comparing the two only
    /// ever showed the reader and the writer agreeing with each other -- never
    /// that a field the survey depends on is still there. Found by a review.
    ///
    /// Changing this list is a breaking change to the row, and
    /// `the_measured_row_carries_exactly_the_contracts_keys` is what makes that
    /// visible in a diff rather than in a mining pass six months later.
    MEASURED_ROW_KEYS,
    /// The same schema with each key's value SHAPE, which is the half the key
    /// list alone could not state.
    ///
    /// Names and order say WHICH fields a row carries; they say nothing about
    /// what those fields hold. Measured before this existed: publishing
    /// `processors` through `.to_string()` -- a number becoming a string in the
    /// mined artifact -- left all 230 library tests and all 10 real-host
    /// integration tests green. Found by a review.
    ///
    /// Changing a shape here is a breaking change to the row exactly as
    /// changing a name is.
    MEASURED_ROW_SHAPES
{
    "reason" => Shape::Text,
    "arch" => Shape::Text,
    "processors" => Shape::Number,
    "groups" => Shape::Number,
    "packages" => Shape::Number,
    "numa_domains" => Shape::Number,
    "numa_domains_without_processors" => Shape::Number,
    "cores" => Shape::Number,
    "efficiency_classes" => Shape::ListOfNumbers,
    "caches" => Shape::ListOfObjectsWith(&[("level", Shape::Number), ("domains", Shape::Number)]),
    "outermost_partitioning_cache_level" => Shape::NumberOrNull,
    "outermost_partitioning_cache" => Shape::Text,
    "policies" => Shape::ObjectOfNumbers,
    "cross_check" => Shape::Text,
    "disagreements" => Shape::ListOfObjectsWith(CODED),
    "not_compared" => Shape::ListOfObjectsWith(CODED),
    "parse_incomplete" => Shape::ListOfObjectsWith(CODED),
    "enumeration_anomalies" => Shape::ListOfObjectsWith(CODED),
    "numa_domains_only_in_cpu_sets" => Shape::Number,
});

row_schema!(
    /// Every key an UNMEASURED topology row carries, in order.
    ///
    /// Deliberately short, and deliberately its own schema rather than a subset
    /// of the one above: a row from a host whose discovery FAILED is a
    /// different shape, and a survey must be able to tell it from a measured
    /// row that happens to be missing fields.
    UNMEASURED_ROW_KEYS,
    /// The same schema with each key's value shape.
    UNMEASURED_ROW_SHAPES
{
    "reason" => Shape::Text,
    "arch" => Shape::Text,
    "cross_check" => Shape::Text,
    "discovery_error" => Shape::Text,
});
use crate::topology::diagnostic::{described, published_anomaly};
use crate::topology::{
    Disagreement, NotCompared, Observation, ParseIncomplete, PartitioningCache, Verdict,
};

/// The banner and title both reports open with.
///
/// The banner is part of the returned text rather than written out separately:
/// a captured report must carry the line naming the machine that produced it,
/// and the taint marker with it. Without it a number can be pasted anywhere and
/// compared against anything.
///
/// **Taken as an argument rather than read here.** This called
/// `fingerprint::banner_line()`, which runs a topology discovery of its own --
/// so this module's claim to be testable without a host was false of its very
/// first line, and the probe made a second, unbracketed platform read whose
/// result could describe a different instant from the body's. The caller reads
/// it once and passes it in.
/// The disclaimer `attribution` renders when the two bracket readings differ.
///
/// A constant because two places need it: the renderer that writes it, and
/// [`preamble`], which has to recognise an attribution-shaped banner to pass it
/// through. A second copy of this sentence in the recogniser would be a
/// restatement that could drift out of step with the one that writes it.
///
/// **It does not mention the body, and that is a correction.** It used to say
/// which reading "names the machine the body below describes was not
/// established", which was wrong in both directions once `measured` existed.
/// When a measured read is passed, the body's machine IS known -- the measured
/// reading names it by construction, and what the differing brackets leave
/// unestablished is whether the machine held still, not which reading the body
/// belongs to. And `report_unmeasured` renders this banner over a report with
/// no topology body at all, so "the body below" referred to nothing there.
///
/// [`NOT_ESTABLISHED`] already had the right shape and this now matches it:
/// state what the readings did, and what that leaves unconfirmed, without
/// reaching forward to a body that may not exist.
const READINGS_DISAGREE: &str = "HOST READINGS DISAGREE: the two readings that bracket the measurement\n\
     differ, so nothing confirmed the machine held still under it.";

/// The disclaimer `attribution` renders when at least one bracket read failed.
const NOT_ESTABLISHED: &str = "HOST NOT ESTABLISHED: at least one of the two readings that bracket the measurement\n\
     failed, so nothing confirmed the machine held still under it.";

/// The disclaimer `attribution` renders when the brackets agree with each other
/// but not with the read the body describes.
///
/// **Agreeing endpoints are not agreement.** `measured` was added so the banner
/// would name the read the body came from rather than an endpoint -- but naming
/// it silently is not the same as establishing it. Two endpoints can agree with
/// each other and both differ from the middle read, and that is exactly the
/// flaky-enumeration case `attribution`'s own rustdoc calls this probe's survey
/// population: `Fingerprint::discover` returns `Ok` on a parse that dropped a
/// record, so three reads of one unchanged machine can disagree. With no
/// disclaimer for it, that host printed an unqualified banner while the run
/// held the evidence against it.
///
/// **This one keeps its reference to the body, and the reason is reachability.**
/// [`READINGS_DISAGREE`] lost its equivalent phrase because it renders on the
/// `report_unmeasured` path, where there is no body to point at. This arm needs
/// a measured read, and the unmeasured path has none by definition -- so
/// wherever it is emitted, a body exists and the measured reading describes it.
/// That holds by what the arm requires, not by a caller remembering.
const MEASURED_UNCONFIRMED: &str = "HOST READING UNCONFIRMED: the two readings that bracket the measurement agree\n\
     with each other but not with the read the body below describes, so one of\n\
     the three discoveries did not see what the others saw.";

/// Whether `banner` is a shape [`attribution`] can produce.
///
/// The count of `host:` lines is not free: an undisclaimed banner carries
/// exactly ONE reading, and a disclaimed one carries the readings its own
/// disclaimer is about. So the cardinality is part of the shape, and this checks
/// it against the disclaimer that was found rather than against the mere fact
/// that one was.
///
/// **The count is per disclaimer, not per disclaimed-or-not.** The two bracket
/// disclaimers carry every reading the run holds, so two without a measured read
/// and three with one; `MEASURED_UNCONFIRMED` is reached only when the brackets
/// are equal, so it carries exactly two. An earlier form accepted only two for
/// all of them, which forced a disclaimed banner to drop one of its three
/// readings -- it dropped the measured one, so a report that disclaimed its own
/// attribution no longer showed the reading its body came from. Widening that to
/// a flat two-or-three for every disclaimer then admitted a three-line
/// `MEASURED_UNCONFIRMED` body, which this renderer cannot produce. Both are the
/// same mistake: a rule about cardinality that does not name which shape it is
/// the cardinality OF.
///
/// **A looser recogniser let the banner state a disagreement while denying
/// there was one.** It accepted any number of `host:` lines with the disclaimer
/// merely optional, so `host: aarch64 ...` above `host: x86_64 ...` and nothing
/// else passed through verbatim -- two readings that name different machines,
/// with none of the sentences `attribution` writes precisely to say that which
/// one describes the body was not established. Measured before this fix: that
/// banner rendered beside an `"arch":"x86_64"` row and the oracle returned no
/// violations at all, because the architecture rule's exemption for an
/// unestablished host swallowed it.
///
/// That is the exemption doing the opposite of its job, and the reason the check
/// belongs HERE rather than in the reader: the oracle can only decide what to do
/// about a shape the renderer emits, and a shape `attribution` cannot produce
/// should never reach it. Anything else did not come from `attribution`,
/// whatever it looks like, and is contained rather than trusted.
fn is_attribution_shaped(banner: &str) -> bool {
    // **The disclaimer must begin a line of its own, because that is how
    // `attribution` writes it.** Matching the suffix alone accepted it GLUED to
    // the reading above, and `trim_end_matches` hid the difference by eating
    // however many newlines it found -- including none. Measured: a banner of
    // `host: X\nhost: XHOST READINGS DISAGREE...` was recognised and written
    // through verbatim, so the report carried a line this renderer cannot
    // produce, with the disclaimer welded onto a reading.
    //
    // Exactly one newline rather than `trim_end_matches`: `attribution` emits
    // one, and accepting several would admit another shape it cannot produce.
    // **The admissible count depends on WHICH disclaimer, not merely on whether
    // there was one.** Widening this to a flat `2..=3` for every disclaimer was
    // half a rule: it admitted a three-reading body under `MEASURED_UNCONFIRMED`,
    // and `attribution` cannot emit that -- that arm is reached only when the
    // brackets are equal, so it prints the measured read and ONE bracket, the
    // second being a copy of the first. A shape this renderer cannot produce
    // passing the recogniser is the precise hole the rustdoc above argues was
    // closed, reopened by the fix that widened it.
    let (body, admissible) = [
        (READINGS_DISAGREE, 2..=3),
        (NOT_ESTABLISHED, 2..=3),
        (MEASURED_UNCONFIRMED, 2..=2),
    ]
    .into_iter()
    .find_map(|(disclaimer, admissible)| {
        banner
            .strip_suffix(disclaimer)
            .and_then(|head| head.strip_suffix('\n'))
            .map(|head| (head, admissible))
    })
    .unwrap_or((banner, 1..=1));

    let lines = body.lines().collect::<Vec<_>>();

    admissible.contains(&lines.len()) && lines.iter().all(|line| line.starts_with("host:"))
}

/// Caller-supplied text, reduced to something that cannot create a line.
///
/// **The report is line-oriented, and every line in it belongs to this
/// renderer.** Two values come from outside -- the banner, and the `io::Error`
/// of a failed discovery -- and both were interpolated verbatim, so either could
/// introduce lines this renderer never wrote. A reader that anchors to line
/// starts, as the oracle does, then reads those lines as the probe speaking.
///
/// Measured before this: an error of `x\nBUG IN THIS PROBE\n=> agree` was read
/// as an alarm beside an agreeing verdict, and an error containing a line
/// starting with `{` was selected as the report's machine-readable row -- so the
/// oracle checked the caller's text instead of the probe's. The second is the
/// worse of the two: not a false alarm but a check of the wrong artifact
/// entirely.
///
/// Fixed HERE rather than in the reader, because the renderer is what owns the
/// format. A reader cannot tell an injected line from a real one after the fact;
/// this guarantees there are none to tell apart. Found by a review, which was
/// right that documenting the hole was not the same as closing it.
fn renderer_owns_every_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

fn preamble(banner: &str) -> String {
    let mut out = String::new();

    // **An attribution-shaped banner passes through; anything else is contained
    // in the banner position.** Two defects meet here, and one fix has to answer
    // both.
    //
    // The banner LEGITIMATELY spans several lines: `attribution` renders two
    // `host:` readings and a disclaimer when they differ. Flattening it
    // unconditionally -- which an earlier fix did -- collapsed that into one
    // line, so the disclaimer stopped being a line of its own and the oracle's
    // exemption for it stopped firing. Measured: a two-reading banner rendered
    // as a single run-on line with no `HOST READINGS DISAGREE:` line at all.
    // Nothing caught it because this host's two readings agree.
    //
    // But the banner is also a `&str` any caller can supply, and it occupies the
    // first line, so an arbitrary string can impersonate a line the renderer
    // reserves. Measured: a banner of `{"reason":...,"cross_check":"agree"}` gave
    // the report TWO machine-readable rows, and the oracle read the caller's
    // instead of the renderer's; a banner of `=> agree` was read as a verdict.
    //
    // So: recognise what `attribution` can produce and write it verbatim, and
    // put everything else behind `host:  `, where it is a banner that names an
    // odd machine rather than a line pretending to be something else.
    if is_attribution_shaped(banner) {
        let _ = writeln!(out, "{banner}");
    } else {
        // Prefixed only when it is not already a banner line, because doubling
        // it produces `host:  host:  ...` and the architecture reader then takes
        // `host:` for an architecture -- a false violation invented by the
        // containment. The invariant wanted is only that this line BEGINS with
        // `host:`; once it does, it cannot be a verdict, an alarm, or a row.
        let contained = renderer_owns_every_line(banner);
        if contained.starts_with("host:") {
            let _ = writeln!(out, "{contained}");
        } else {
            let _ = writeln!(out, "host:  {contained}");
        }
    }
    let _ = writeln!(
        out,
        "== processor topology, and what each partitioning policy would yield ==\n"
    );
    out
}

/// The banner to print, given the host fingerprint read before and after the
/// measurement.
///
/// The fingerprint is itself a topology rendering -- architecture, processor and
/// core counts, cache domain sizes, NUMA nodes -- read through a *second*
/// discovery that `measure`'s bracket does not enclose. Reading it once and
/// calling that "attribution" was the weaker claim it sounded: a machine that
/// changed across the run would print one shape in the banner and a different
/// one in the body, with nothing saying which described the measurement.
///
/// So it is bracketed like everything else here. Equal readings print as one
/// line, exactly as before. Readings that differ print both and say so, because
/// which of them describes the body is what the run did not establish.
///
/// **It says WHICH reading is unknown, not that both are wrong.** The
/// measurement happened between them, so one of the two may well name the
/// machine the body describes -- what a disagreement at the endpoints
/// establishes is that this run cannot say which. Calling both wrong would be
/// its own over-claim, in the sentence added to stop one.
///
/// **It says the readings DIFFER, not that the host changed.**
/// `Fingerprint::discover` returns `Ok` on a parse that dropped a record or
/// whose two sources disagreed, so a fingerprint can differ from the one before
/// it because the enumeration was flaky rather than because any hardware moved.
/// Naming a cause this run cannot distinguish would be the same over-claim the
/// body below is built to avoid; what is established is that the two disagree,
/// and that is all this says.
///
/// **Takes the discoveries rather than two rendered lines.** Compared as
/// strings, a failed read is a line like any other, so one failure beside one
/// success -- or two failures whose `io::Error` text differs -- read as a host
/// that changed. That is a claim about the machine drawn from a gap in the
/// measurement: a failed read establishes neither that the host moved nor that
/// it held still.
/// **`measured` names the read the body describes, and that is the point.**
/// The banner used to be built from `before` -- an *endpoint* read -- so equal
/// endpoints printed an unqualified banner without anything having established
/// that the middle read agreed with them. `measure_observed` returns the
/// fingerprint of the topology it actually parsed, so passing it here makes the
/// banner describe the body by construction.
///
/// **Naming it is not establishing it, and the two were conflated.** Making the
/// banner name the measured read fixes *which* machine the body is attributed
/// to; it says nothing about whether the brackets confirmed that read. Equal
/// endpoints that both differ from it are a real and documented case -- see
/// `MEASURED_UNCONFIRMED` -- and it now has a disclaimer of its own rather than
/// printing unqualified. That is a comparison, and deliberately so: the
/// alternative is not "no comparison", it is an unstated one.
///
/// `None` for a run whose discovery failed: there is no measured read to name,
/// so the banner falls back to the first endpoint as before. That is the
/// `report_unmeasured` path, where the body describes no topology either.
///
/// **Every disclaimed banner shows every reading the run holds**, so a reader
/// who is told the attribution is in doubt can see what the readings were. With
/// a measured read that is three lines, not two.
///
/// The endpoints keep their job. They bracket a **wider** window than
/// `measure`'s counter bracket -- which covers only its own discovery -- so they
/// still catch structural change the counters cannot see.
#[must_use]
pub fn attribution(
    measured: Option<&Fingerprint>,
    before: &io::Result<Fingerprint>,
    after: &io::Result<Fingerprint>,
) -> String {
    // **Each reading is flattened HERE, because a banner line is a line.**
    // `banner_line_for` interpolates a failed read's `io::Error` verbatim, and
    // an OS error is free to contain a newline -- so a reading could arrive as
    // two lines and the banner this composes would have more lines than
    // readings. `is_attribution_shaped` then stops recognising its own output,
    // `preamble` contains the whole thing, and the flattening takes the
    // RENDERER-OWNED disclaimer down with it. Measured: a two-line error gave a
    // six-line attribution and a report with no `HOST NOT ESTABLISHED:` line at
    // all, so the oracle's exemption for an unestablished host stopped firing on
    // a report that had legitimately earned it.
    //
    // Containing each line as it is built keeps the composition's shape a
    // function of the number of READINGS rather than of what the OS wrote, which
    // is what every reader below assumes.
    // **Both changes survive the merge, and they are independent.** The
    // containment above is main's; `measured` is this branch's -- one decides
    // HOW a reading is contained, the other WHICH reading names the body. The
    // merge that brought them together took main's body and re-applied the
    // parameter rather than picking a side, because picking either would have
    // silently dropped a fix.
    //
    // Routed through `banner_line_for` rather than formatted here, even though
    // wrapping in `Ok` to do it looks roundabout: a probe's banner is comparable
    // with every other probe's only while exactly one place produces that line.
    //
    // **Which lines each arm prints is decided by what its disclaimer says, and
    // getting that backwards printed two identical lines under a sentence
    // asserting they differ.** `measured` names the read the body describes, so
    // it is the right banner for the arm that emits ONE unqualified line. Every
    // disclaimed arm then prints every reading the run holds -- the two
    // brackets, and the measured read when there is one -- because dropping one
    // is how both earlier defects happened: substituting the measured line for
    // `before` hid the reading that actually disagreed and, on a failed read,
    // hid the `io::Error` with it; dropping the measured line instead left a
    // report that disclaimed its own attribution without showing the reading
    // its body came from.
    //
    // **Agreeing brackets are not agreement, which is what the third arm is
    // for.** Two endpoints can agree with each other and both differ from the
    // middle read -- the flaky-enumeration case this rustdoc names as the survey
    // population -- and with no arm for it that host printed an unqualified
    // banner while the run held the evidence against it.
    let measured_line = || {
        renderer_owns_every_line(&measured.map_or_else(
            || banner_line_for(before),
            |fingerprint| banner_line_for(&Ok(fingerprint.clone())),
        ))
    };
    let bracket = |reading| renderer_owns_every_line(&banner_line_for(reading));
    // Every reading, measured first: the banner's first line names the machine
    // the body describes whenever the run knows it.
    let every_reading = || match measured {
        Some(_) => format!(
            "{}\n{}\n{}",
            measured_line(),
            bracket(before),
            bracket(after)
        ),
        None => format!("{}\n{}", bracket(before), bracket(after)),
    };
    match (before, after) {
        (Ok(one), Ok(two)) if one == two => match measured {
            // The brackets agree and the body's own read agrees with them, so
            // there is one reading to report and nothing to qualify.
            Some(fingerprint) if fingerprint == one => measured_line(),
            // They agree with each other and not with it. One bracket is shown
            // rather than both: they are equal here, so the second would be a
            // copy of the first, and the disclaimer says which relation failed.
            Some(_) => format!(
                "{}\n{}\n{MEASURED_UNCONFIRMED}",
                measured_line(),
                bracket(before)
            ),
            None => measured_line(),
        },
        (Ok(_), Ok(_)) => format!("{}\n{READINGS_DISAGREE}", every_reading()),
        // Covers (Err, Ok), (Ok, Err) AND (Err, Err), so the text says "at
        // least one". "One of the two readings failed" understates the case
        // where both did -- a small thing, but the same shape as every other
        // sentence corrected here: claiming a more specific state than the run
        // established.
        _ => format!("{}\n{NOT_ESTABLISHED}", every_reading()),
    }
}

/// The report for a run whose discovery failed.
///
/// Carries an `x-probe-topology` row of its own, so the fleet survey can tell a
/// host where discovery FAILED from a job that never ran the probe. Returning
/// only prose made those indistinguishable, which silently excluded exactly the
/// hosts most worth counting.
#[must_use]
pub fn report_unmeasured(banner: &str, error: &io::Error) -> String {
    let mut out = preamble(banner);
    let _ = writeln!(
        out,
        "MachineMemoryTopology::discover failed: {}",
        renderer_owns_every_line(&error.to_string())
    );
    let _ = writeln!(
        out,
        "(Reported rather than measured: a probe that cannot read its"
    );
    let _ = writeln!(
        out,
        "subject must say so instead of printing a misleading shape.)"
    );
    // **Through the same writer, and this is the shape that most needs it.**
    // The only caller text in any report reaches THIS renderer -- a failed
    // discovery's `io::Error`, whose message is whatever the OS said. Publishing
    // it as a `Value::Text` is what makes a brace in that message inert rather
    // than the start of a second row, which is the contamination measured on
    // PR #88.
    //
    // The error is published as a field rather than only printed, because a
    // survey counting failures wants to group them by cause, and the prose
    // sentence above is not something a mining pass should be parsing.
    let _ = writeln!(
        out,
        "{}",
        Row::new("x-probe-topology")
            .with("arch", std::env::consts::ARCH)
            .with("cross_check", "not_measured")
            .with("discovery_error", error.to_string())
            .render()
    );

    // Bound here too, for the reason given on `report` below: an unmeasured
    // report is still a report, and a survey still has to parse its row.
    //
    // This used to say the renderer "makes fewer claims, so fewer
    // correspondences apply". There are no correspondences left to apply -- M3
    // retired the prose/row relation, and what is checked is that the row is
    // well formed, which is not a thing a renderer can make less of.
    //
    // The SHAPES are bound here too, for the reason given on `report` below:
    // names alone leave a consumer's field types unconstrained.
    #[cfg(any(test, feature = "oracle-in-renderer"))]
    {
        crate::report_oracle::assert_row_is_well_formed(&out);
        crate::report_oracle::assert_row_has_the_schemas_shapes(&out, UNMEASURED_ROW_SHAPES);
    }
    out
}

/// The report for a run whose discovery succeeded.
#[must_use]
pub fn report(banner: &str, observation: &Observation) -> String {
    let mut out = preamble(banner);

    // Computed HERE rather than beside the cache conclusions it was first
    // written for. The heterogeneity note below is a hardware claim too -- "an
    // unconstrained thread can land on an efficiency core" -- and it was printed
    // before this line existed, so it was structurally ungated while the design
    // note said `parse_in_doubt` gates every hardware conclusion the renderer
    // draws. A rule with one exception is not a rule, and the exception was the
    // conclusion drawn from the one field the two sources are known to
    // contradict each other about.
    // **Bound here as well as in `observe`, and the two cover different
    // things.** `observe` covers every observation this crate MEASURES; this
    // covers every observation anyone RENDERS, which on the test side is most
    // of them -- the suite builds observations by hand rather than measuring a
    // machine, so binding only at `observe` would leave the invariants
    // exercised on one host shape.
    #[cfg(any(test, feature = "oracle-in-renderer"))]
    crate::topology::invariant::assert_holds(observation);

    let check = observation.cross_check();
    let parse_in_doubt = check.parse_in_doubt();

    let _ = writeln!(
        out,
        "processors (online) : {}",
        observation.online_processors
    );
    let _ = writeln!(out, "processor groups    : {}", observation.groups);
    let _ = writeln!(out, "packages            : {}", observation.packages);
    let _ = writeln!(
        out,
        "NUMA domains        : {} ({} with no processors)",
        observation.numa_domains, observation.numa_domains_without_processors
    );
    if observation.numa_domains_only_in_cpu_sets > 0 {
        let _ = writeln!(
            out,
            "  ({} reported only by CPU Sets, never by the relationship walk:",
            observation.numa_domains_only_in_cpu_sets
        );
        let _ = writeln!(out, "   the two sources group nodes differently)");
    }
    let _ = writeln!(out, "physical cores      : {}", observation.cores.len());

    let smt = observation
        .cores
        .iter()
        .filter(|c| c.simultaneous_multithreading)
        .count();
    let mut classes: Vec<u8> = observation
        .cores
        .iter()
        .map(|c| c.efficiency_class)
        .collect();
    classes.sort_unstable();
    classes.dedup();
    let _ = writeln!(out, "  cores with SMT    : {smt}");
    let _ = writeln!(out, "  efficiency classes: {classes:?}");
    if classes.len() > 1 {
        let _ = writeln!(
            out,
            "  (heterogeneous: an I/O thread left unconstrained can land on an"
        );
        let _ = writeln!(
            out,
            "   efficiency core, which is why even a single domain wants a mask)"
        );
        if parse_in_doubt {
            let _ = writeln!(
                out,
                "   (This run did not establish that the parse is whole, and the classes"
            );
            let _ = writeln!(
                out,
                "    above are what decoded -- see the cross-check below for why.)"
            );
        }
    }

    let _ = writeln!(out, "\ncaches:");
    if observation.caches.is_empty() {
        let _ = writeln!(out, "  none reported");
    }
    for cache in &observation.caches {
        let _ = writeln!(
            out,
            "  L{:<2} {:>3} domain(s), processors per domain: {:?}",
            cache.level,
            cache.domains(),
            cache.processors_per_domain
        );
    }

    // Every conclusion below is drawn from the cache summaries above, and those
    // describe what DECODED rather than what the machine has. An undersized
    // `CACHE_RELATIONSHIP` decodes to nothing and is recorded as an anomaly,
    // while one whose trailing affinity array is truncated is KEPT with the
    // entries that fit -- and `discover` returns `Ok` either way. So the
    // summaries can be short a level or carry one whose processor set is
    // smaller than the truth, and these lines say what the parse contains
    // rather than what the hardware is.
    //
    // The truncated case is worth stating precisely, because an earlier version
    // of this comment said it "decodes to nothing": a kept record with a
    // partial mask presents a set that is distinct from the full one, so it can
    // ADD a partition rather than remove one. A maintainer who believed the old
    // wording would have ruled out the only way that inflation happens.
    //
    // Named for the CONDITION, not for one of its causes. It was `dropped`, and
    // the caveats it gated said "a record was dropped" -- which became false the
    // moment the condition widened to every `parse_incomplete` entry: forcing a
    // coherence disagreement with zero anomalies printed a caveat blaming a
    // dropped record. Each caveat now states that the run did not establish the
    // parse is whole and points at the cross-check, which is where the actual
    // reason is already printed in full.
    //
    // ASKED, not re-derived, because every time this condition has been
    // restated here it has been restated wrongly. First as
    // `!enumeration_anomalies.is_empty()`, so a host reporting
    // `Coherence::Disagreed` with no anomalies printed "this machine reports no
    // L3 at all" a few lines above "=> INCOMPLETE ... its two enumerations never
    // agreed". Then as `!parse_incomplete.is_empty()`, which missed
    // `disagreements` -- so a host whose group count Windows contradicts printed
    // the same hardware claim directly above "=> DISAGREE", in the one case
    // where the evidence that the parse does not describe this machine was
    // already in hand. Both times the accompanying comment asserted the
    // condition was complete.
    //
    // `CrossCheck::parse_in_doubt` is now the single definition, and it is the
    // place that argues which lists belong. It is computed at the top of this
    // function so that every hardware conclusion below reads the same one.

    // Matched exhaustively, so an absent level cannot be printed without having
    // decided WHICH absent case it is. This arm used to recite the ambiguity --
    // "either no level partitions this machine, or two partition it
    // incomparably" -- because the probe genuinely could not tell. It can tell
    // the first apart from the rest, so it now says so.
    match observation.partitioning_cache() {
        PartitioningCache::Level(cache) => {
            // "of the processors it covers", not "of this machine". The owning
            // crate's filter is `blocks.len() > 1 && are_pairwise_disjoint`,
            // with no coverage check, so two disjoint domains covering
            // processors 0 and 1 of a four-processor host qualify -- and
            // nothing here verifies otherwise. The mirror sentence in the
            // `NoLevelPartitions` arm below was narrowed for this exact reason;
            // this one kept the claim, which is the half of a pair being swept
            // and the other half left behind.
            let _ = writeln!(
                out,
                "\noutermost cache that partitions the processors it covers: L{} ({} domains)",
                cache.level,
                cache.domains()
            );
            if parse_in_doubt {
                let _ = writeln!(
                    out,
                    "  (of the levels that decoded. This run did not establish that the parse"
                );
                let _ = writeln!(
                    out,
                    "   is whole -- see the cross-check below for why -- so a coarser level may"
                );
                let _ = writeln!(out, "   exist on this machine and be missing above.)");
            }
        }
        PartitioningCache::NoLevelPartitions => {
            let _ = writeln!(
                out,
                "\nno cache level reported more than one domain, so nothing here divides"
            );
            // Not "every level covers the whole machine", which was the reading
            // this offered and is false for a level that decoded to NO
            // partitions -- that covers nothing, not everything, and lands here
            // too. `cross_check` now caveats that case, but the sentence should
            // not have needed the caveat to stop being wrong.
            //
            // Nor "no cache boundary divides the work", which was the next
            // wording and is a claim about the HARDWARE. A single domain
            // covering half the online processors also lands here, and nothing
            // in this probe checks that the domains at a level cover the
            // machine -- so the honest statement is about what this report
            // found, not about what the silicon does.
            let _ = writeln!(out, "the work by cache.");
        }
        PartitioningCache::NoLevelsReported => {
            let _ = writeln!(
                out,
                "\nno cache levels were reported at all, so nothing here says whether a"
            );
            let _ = writeln!(out, "cache boundary divides this machine.");
        }
        PartitioningCache::NoUniqueOutermost => {
            // "More than one DISTINCT domain", not "partitions this machine".
            // `domains()` counts distinct processor sets and the topology crate
            // is explicit that distinct is not disjoint, so a level whose blocks
            // overlap lands here while partitioning nothing -- which the third
            // line below then names, contradicting an opening that claimed the
            // machine was partitioned.
            let _ = writeln!(
                out,
                "\nat least one cache level reported more than one distinct domain, but"
            );
            let _ = writeln!(
                out,
                "no unique outermost one was established: either two partition this"
            );
            let _ = writeln!(
                out,
                "machine incomparably, or the candidates were rejected as overlapping"
            );
            let _ = writeln!(out, "-- in which case none of them partitions it at all.");
        }
        PartitioningCache::SummaryMissing(level) => {
            let _ = writeln!(
                out,
                "\nBUG IN THIS PROBE: the topology crate named L{level} as the outermost"
            );
            let _ = writeln!(
                out,
                "partitioning cache and this survey carries no summary for it. Nothing"
            );
            let _ = writeln!(out, "below about cache partitioning can be trusted.");
        }
    }
    if parse_in_doubt
        && !matches!(
            observation.partitioning_cache(),
            PartitioningCache::Level(_) | PartitioningCache::SummaryMissing(_)
        )
    {
        // A parse that is short or disputed is exactly how the level that would
        // have partitioned this machine goes missing, so the absent answers
        // above are not safe to read as hardware either.
        //
        // Both variants that NAME a level are excluded, not just `Level`. The
        // exclusion list read `Level(_)` alone while `SummaryMissing` could not
        // reach here -- it did not put the parse in doubt by itself -- and the
        // check that made it do so turned this into "the topology crate named
        // L3 ... " printed directly above "the level that would have
        // partitioned this machine is missing". A level was named; its SUMMARY
        // is what is absent, and the arm above says exactly that.
        let _ = writeln!(
            out,
            "Or the parse is not whole and the level that would have partitioned this"
        );
        let _ = writeln!(
            out,
            "machine is missing -- see the cross-check below for why."
        );
    }
    if !observation.caches.iter().any(|c| c.level == 3) {
        // "no L3" is a claim about the machine, and the list it is read off is
        // only what decoded. Asserted as hardware when the parse is whole, and
        // as a fact about the parse when it is not -- otherwise a host whose L3
        // record alone failed to decode is filed as an ARM64-style no-L3
        // machine, and its "outermost partitioning cache: L2" is read as a real
        // cluster boundary.
        if parse_in_doubt {
            let _ = writeln!(
                out,
                "NOTE: no L3 decoded on this run, and this run did not establish that the"
            );
            let _ = writeln!(
                out,
                "parse is whole -- so that is a fact about the parse, not evidence the"
            );
            let _ = writeln!(out, "machine has no L3. See the cross-check below.");
        } else {
            let _ = writeln!(
                out,
                "NOTE: this machine reports no L3 at all, so a policy keyed literally"
            );
            let _ = writeln!(
                out,
                "on \"L3\" would find nothing here. That is the measured case behind"
            );
            let _ = writeln!(
                out,
                "phrasing the rule as \"the outermost level that partitions\"."
            );
        }
    }

    let _ = writeln!(out, "\ndomains each policy would produce:");
    for (name, count) in observation.domain_counts() {
        let _ = writeln!(out, "  {name:<34} {count}");
    }

    let _ = writeln!(
        out,
        "\ncross-check against independently read Win32 counters:"
    );
    let _ = writeln!(
        out,
        "  GetActiveProcessorCount     : {}",
        observation.raw_active_processors
    );
    let _ = writeln!(
        out,
        "  GetActiveProcessorGroupCount: {}",
        observation.raw_group_count
    );
    match observation.raw_highest_numa_node {
        Some(highest) => {
            // The identifier, and deliberately no count derived from it.
            // `GetNumaHighestNodeNumber` reports the largest node NUMBER, and
            // node numbers may be sparse -- a machine with nodes 0 and 2 has two
            // nodes and a highest of 2. `highest + 1` would print three, which
            // is the same mistake `Observation::cross_check` was corrected to
            // stop making; re-deriving it here would put it back in the output
            // the cross-check is printed beside.
            let _ = writeln!(out, "  GetNumaHighestNodeNumber    : {highest}");
            let _ = writeln!(
                out,
                "    (the largest node NUMBER, not a count: node numbers can be sparse)"
            );
        }
        None => {
            let _ = writeln!(out, "  GetNumaHighestNodeNumber    : failed");
        }
    }
    // Matched exhaustively on purpose. The verdict used to be `complaints
    // .is_empty()`, which printed "agree" when a counter had merely failed to
    // read -- on the line directly below "GetNumaHighestNodeNumber : failed".
    // A three-state verdict makes that arm impossible to omit.
    match check.verdict() {
        Verdict::Agree => {
            let _ = writeln!(
                out,
                "  => agree. Every check this probe could make was made and matched."
            );
        }
        Verdict::Disagree => {
            let _ = writeln!(out, "  => DISAGREE. This is a finding, not a nuisance:");
            for complaint in &check.disagreements {
                let _ = writeln!(out, "     - {}", described(complaint));
            }
            // Listed even here, so a reader knows the disagreement above is not
            // the whole picture.
            for skipped in &check.not_compared {
                let _ = writeln!(out, "     (not compared) {}", described(skipped));
            }
            for caveat in &check.parse_incomplete {
                let _ = writeln!(out, "     (parse incomplete) {}", described(caveat));
            }
        }
        Verdict::Incomplete => {
            let _ = writeln!(
                out,
                "  => INCOMPLETE. Nothing this probe compared disagreed, but this run"
            );
            let _ = writeln!(out, "     did not establish that the parse is consistent:");
            for skipped in &check.not_compared {
                let _ = writeln!(out, "     - {}", described(skipped));
            }
            for caveat in &check.parse_incomplete {
                let _ = writeln!(out, "     - {}", described(caveat));
            }
        }
    }

    // One machine-readable line, so accumulated CI logs can be mined without
    // parsing the prose above. Kept to a single line on purpose.
    //
    // `cross_check` is the one field a mining pass must read before trusting
    // any other. Every count here is taken from what decoded, so a dropped
    // record makes `caches`, `packages`, `cores` and
    // `outermost_partitioning_cache_level` short by an amount no field states
    // -- and a query grouping by cache level has no reason to join against
    // `enumeration_anomalies` on its own. `cross_check` closes that: anomalies
    // populate `parse_incomplete`, and a non-empty `parse_incomplete` forces
    // the verdict away from "agree", so `cross_check == "agree"` IMPLIES no
    // record failed to decode. One way only: a run whose counter failed to
    // read has a complete parse and still reports "incomplete". That is a rule about
    // the code above, so the test
    // `a_dropped_enumeration_record_blocks_agreement_even_when_every_counter_matches`
    // pins it rather than leaving it as a promise in a comment.
    //
    // That is narrower than "the counts are complete", and the gap is not
    // closed anywhere: nothing independent measures packages, cores or caches,
    // so a record that decoded cleanly while describing less of the machine
    // than exists raises no anomaly and reaches "agree". `agree` says every
    // check this probe could make was made and matched -- not that a check
    // exists for every field on this line.
    //
    // "Not short" is all it says, and the distinction is load-bearing. This
    // comment used to promise that `agree` made the rest of the line "describe
    // the machine rather than the parse", which was false for
    // `outermost_partitioning_cache_level`: a machine whose levels partition it
    // incomparably has a complete parse, agrees with every counter, and still
    // cannot be said to have an outermost partitioning cache. It emitted `null`
    // exactly like a machine no level partitions -- opposite conclusions for
    // anything sizing itself by cache boundary, on a row already certified.
    // Hence the `outermost_partitioning_cache` field beside it, which a
    // consumer must read rather than inferring from the level being absent.
    // **One writer, one value, no positions.** The row was built by
    // interpolating every value positionally into a `concat!` template, where a
    // field's name and its value were related only by counting -- so a reordered
    // argument or a miscounted placeholder yielded mislabelled data that still
    // parses. Here a name and its value are one pair. See `crate::row`.
    //
    // The diagnostics publish their DATA now, not only their code: a survey
    // learns `contradictory_cores` AND that three cores contradicted themselves.
    // What stopped M3.1 doing that was this template.
    let row = Row::new("x-probe-topology")
        .with("arch", std::env::consts::ARCH)
        .with("processors", observation.online_processors)
        .with("groups", observation.groups)
        .with("packages", observation.packages)
        .with("numa_domains", observation.numa_domains)
        .with(
            "numa_domains_without_processors",
            observation.numa_domains_without_processors,
        )
        .with("cores", observation.cores.len())
        // The CLASSES, not how many there are. A plural name over a count is
        // ambiguous in the one way that matters: on a single-class host this
        // emitted `"efficiency_classes":1`, which reads exactly like a machine
        // whose one class is class *1* -- while the prose printed
        // `efficiency classes: [0]`. Same fact, two renderings a consumer cannot
        // reconcile. A survey can still get the count from the length.
        .with(
            "efficiency_classes",
            classes.iter().copied().collect::<Value>(),
        )
        .with(
            "caches",
            observation
                .caches
                .iter()
                .map(|cache| {
                    Value::Object(vec![
                        ("level", Value::from(cache.level)),
                        ("domains", Value::from(cache.domains())),
                    ])
                })
                .collect::<Value>(),
        )
        // The level alone said `null` for every absent case alike, on a line the
        // verdict had already certified as `agree` -- an incomparable
        // partitioning touches nothing `cross_check` consults. A query counting
        // nulls as "machines no cache level partitions" then folded in machines
        // where a level DOES partition, the opposite conclusion for anything
        // sizing itself by cache boundary. Hence the discriminator beside it,
        // which a consumer must read rather than inferring from the absence.
        // **The level a SummaryMissing arm names is published too**, not
        // `null`: the two renderings of WHICH level went unchecked are what a
        // reader needs when the report is telling them the probe has a bug.
        .with(
            "outermost_partitioning_cache_level",
            match observation.partitioning_cache() {
                PartitioningCache::Level(cache) => Value::from(cache.level),
                PartitioningCache::SummaryMissing(level) => Value::from(level),
                PartitioningCache::NoLevelsReported
                | PartitioningCache::NoLevelPartitions
                | PartitioningCache::NoUniqueOutermost => Value::Null,
            },
        )
        .with(
            "outermost_partitioning_cache",
            match observation.partitioning_cache() {
                PartitioningCache::Level(_) => "level",
                PartitioningCache::NoLevelsReported => "no_levels_reported",
                PartitioningCache::NoLevelPartitions => "none",
                PartitioningCache::NoUniqueOutermost => "not_unique",
                PartitioningCache::SummaryMissing(_) => "summary_missing",
            },
        )
        .with(
            "policies",
            Value::Object(
                observation
                    .domain_counts()
                    .into_iter()
                    .map(|(name, count)| (name, Value::from(count)))
                    .collect(),
            ),
        )
        // A tri-state rather than a boolean: a mining pass must be able to tell
        // "all three counters agreed" from "two agreed and the third was never
        // compared". `cross_check_ok:true` said the same thing for both.
        .with(
            "cross_check",
            match check.verdict() {
                Verdict::Agree => "agree",
                Verdict::Disagree => "disagree",
                Verdict::Incomplete => "incomplete",
            },
        )
        .with(
            "disagreements",
            check
                .disagreements
                .iter()
                .map(Disagreement::published)
                .collect::<Value>(),
        )
        .with(
            "not_compared",
            check
                .not_compared
                .iter()
                .map(NotCompared::published)
                .collect::<Value>(),
        )
        .with(
            "parse_incomplete",
            check
                .parse_incomplete
                .iter()
                .map(ParseIncomplete::published)
                .collect::<Value>(),
        )
        .with(
            "enumeration_anomalies",
            observation
                .enumeration_anomalies
                .iter()
                .map(published_anomaly)
                .collect::<Value>(),
        )
        .with(
            "numa_domains_only_in_cpu_sets",
            observation.numa_domains_only_in_cpu_sets,
        );

    let _ = writeln!(out, "{}", row.render());

    // **Bound here rather than called from each test, which is the difference
    // between an oracle and three more tests.** A test added beside the others
    // checks one case; binding the renderer checks every case anyone writes
    // later, including the ones nobody thought to add.
    //
    // **What it catches: a renderer that emits a malformed row.** Nothing here
    // compares the prose against the row. M3 retired that relation -- the row is
    // the machine contract and the prose is reviewed, not parsed -- so the only
    // claim this line supports is that every report this renderer produces
    // carries exactly one well-formed JSON row.
    //
    // **Measured, not assumed** (2026-09-13, by sabotaging `Row::render` to drop
    // the closing brace): 42 library tests fail with this line, 29 without it.
    // The 13 that only this line catches are about cache notes, efficiency
    // classes, caveats, NUMA lines and partitioning levels -- not one of them
    // mentions the row's syntax. They inherit the check purely by rendering a
    // report, which is the whole argument for binding the renderer instead of
    // adding a test beside the others.
    //
    // **The previous version of this paragraph was false, and that is worth
    // recording.** It claimed a prose/NDJSON processor-count contradiction turned
    // 13 tests red through this line. That was true before M3 and silently
    // stopped being true when the oracle stopped reading prose: re-run in full,
    // the sabotage it names now leaves all 228 library tests AND all 10 real-host
    // integration tests green. A reviewer inferred it from the code; the check
    // that settled it was running it. An evidence paragraph nothing executes is
    // the same rot as a test nothing runs -- so when this mechanism changes
    // again, re-measure rather than re-word.
    //
    // The counts above are dated for that reason. The invariant is the durable
    // half: nothing else in the library catches a malformed row, because no
    // per-part test parses one.
    //
    // **Why the gate is not `cfg(test)` alone.** It was, and the claim above was
    // then false for half of what "every test" means: cargo compiles this
    // library as an ordinary dependency, WITHOUT `cfg(test)`, for anything under
    // `tests/`. Found by a review and measured -- an integration test rendered a
    // report whose banner architecture contradicted its NDJSON `arch` and did
    // not panic. The `oracle-in-renderer` feature, enabled explicitly for the
    // required-feature integration target, closes that.
    //
    // **A DEFAULT-FEATURE build still prints rather than panics**, which is the
    // contract this gate exists to preserve: a self-contradicting report is a
    // finding about the probe, and a fleet survey needs the row, not a crash.
    // Verified by inspecting the built artifacts for the assertion's panic
    // string:
    //
    //   cargo build                  assertion ABSENT
    //   cargo build --all-features   assertion PRESENT
    //   built under cargo test       assertion PRESENT
    //
    // **The first version of this comment said "off for every non-test build",
    // and that was wrong.** `--all-features` enables it like any other feature,
    // so a non-test binary built that way panics on a contradiction instead of
    // printing it -- and this workspace does use `--all-features` in practice,
    // for cargo-mutants runs. Cargo has no stable way to declare a feature that
    // `--all-features` skips, so the honest fix is to state the boundary rather
    // than to claim one the mechanism cannot hold. Found by a review.
    //
    // Two consequences, both deliberate and both named here rather than left to
    // be discovered: binaries built by the feature-on test command assert, so a
    // probe spawned by the required-feature integration target aborts on a
    // contradiction instead of printing it; and so does an `--all-features`
    // build. The evidence survives in every case, because the assertion's
    // message carries the whole report -- what is lost is the NDJSON row a
    // survey would have mined, which is why the default build is the one that
    // matters and is the one pinned above.
    //
    // **The schema's SHAPES are bound here for the same reason.** Well-formed
    // says the row parses; the schema says `processors` is a number and each
    // diagnostic entry carries a code. Measured: publishing `processors` through
    // `.to_string()` left all 230 library tests and all 10 real-host integration
    // tests green before this line existed. Found by a review.
    //
    // **Both inside ONE `cfg` block, which the first attempt got wrong.** A
    // `#[cfg]` attribute governs the single statement that follows it, so
    // adding a second call beneath the gated one left that call ungated -- and
    // `report_oracle` does not exist in a default build. Nothing local caught
    // it: the feature-on test wiring compiles only the enabled arm in those
    // runs, so the feature-off arm was never checked there. CI's `cargo run
    // --bin` is, and that is where it broke.
    #[cfg(any(test, feature = "oracle-in-renderer"))]
    {
        crate::report_oracle::assert_row_is_well_formed(&out);
        crate::report_oracle::assert_row_has_the_schemas_shapes(&out, MEASURED_ROW_SHAPES);
    }
    out
}
