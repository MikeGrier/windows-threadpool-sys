// Copyright (c) 2026 Mike Grier
//! The one thing a runner is asked to execute.
//!
//! **One binary, one run, one paste.** "Run these three and send me all three
//! outputs" is friction for someone doing a favour, and invites partial
//! submissions that cannot be compared with each other.

use std::fmt::Write as _;
use std::process::ExitCode;

mod sink;
#[cfg(test)]
mod tests;

use sink::{Sink, Stdio, emit};

use windows_placement_probe::build_identity::BuildIdentity;
use windows_placement_probe::core_affinity::{self, RunPlan};
use windows_placement_probe::fingerprint::{Fingerprint, places_from_topology};
use windows_placement_probe::machine::{MachineDescription, VirtualisationHint};
use windows_placement_probe::record::SubmissionRecord;
use windows_placement_probe::redaction::MetadataPolicy;
use windows_placement_probe::submission::{self, DISCUSSION_URL};
use windows_topology_sys::MachineMemoryTopology;

/// What the run was asked to do.
struct Options {
    /// Show what would be collected, and measure nothing.
    preview: bool,
    /// Include the secondary metadata, which is withheld by default.
    include_metadata: bool,
    /// Withhold the CPU model.
    ///
    /// Kept as a separate switch rather than folded into
    /// [`Self::include_metadata`] because it subtracts from it: a runner
    /// willing to send an OS build and a hypervisor name may still be sitting
    /// in front of a part whose name is not theirs to publish. Redundant on its
    /// own, and harmless -- see [`Options::metadata_policy`].
    suppress_model: bool,
    /// Skip writing the backup file.
    no_file: bool,
    /// Print the usage message and exit successfully.
    ///
    /// A separate field from a parse error on purpose: asking for help is a
    /// request the tool can satisfy, not a mistake. Returned as Err it went
    /// to stderr and exited non-zero, so placement-probe --help | less showed
    /// nothing and any script running it reported failure.
    help: bool,
    /// Print the build identity and exit.
    version: bool,
}

impl Options {
    /// Which secondary metadata this run will collect.
    ///
    /// The one place the switches become a policy, so every consumer -- the
    /// notice, the machine read, the record -- is deciding from the same
    /// answer rather than re-deriving it from the flags.
    ///
    /// `--no-cpu-model` without `--include-metadata` withholds something
    /// already withheld. That is redundant rather than an error, and stays
    /// harmless on purpose: a cautious runner who passes both must get the same
    /// record as one who passes neither, not a worse one.
    fn metadata_policy(&self) -> MetadataPolicy {
        let policy = if self.include_metadata {
            MetadataPolicy::included()
        } else {
            MetadataPolicy::redacted()
        };
        if self.suppress_model {
            policy.without_cpu_model()
        } else {
            policy
        }
    }
}

fn main() -> ExitCode {
    run(&mut Stdio)
}

/// The whole tool, against any [`Sink`].
///
/// Separate from [`main`] so the streams are a parameter rather than a global.
/// `main` is the only place that names [`Stdio`].
fn run(out: &mut impl Sink) -> ExitCode {
    let options = match parse_arguments() {
        Ok(options) => options,
        Err(message) => {
            out.problem(&message);
            return ExitCode::FAILURE;
        }
    };

    if options.help {
        // The report stream and success: help was asked for and was given.
        //
        // The trailing blank line is deliberate here and was verified against
        // the previous build: `help()` ends with a newline and the `println!`
        // that used to print it added a second, so the output ended with a
        // blank line. `emit` drops a trailing newline rather than yielding an
        // empty line, so keeping the spacing takes an explicit line. Whether
        // that blank should exist at all is a question about the help text, not
        // about where output goes -- and this change is only about the latter.
        emit(out, &help());
        out.line("");
        return ExitCode::SUCCESS;
    }

    if options.version {
        // Deliberately the whole identity rather than just a version number.
        // CI asserts on this line that a released artifact reports itself
        // official, and a runner can check the same thing before relying on a
        // download -- both need the commit and the source, not just "0.1.0".
        out.line(&BuildIdentity::current().to_string());
        return ExitCode::SUCCESS;
    }

    let policy = options.metadata_policy();
    let machine = MachineDescription::read(policy);

    // **One discovery, two derivations.** The announced plan and the recorded
    // fingerprint used to come from separate `MachineMemoryTopology::discover()` calls, so a
    // processor going offline between them would have the notice describing one
    // machine and the record another, with nothing in the output saying which
    // was which. Both now come from this reading.
    let topology = match MachineMemoryTopology::discover() {
        Ok(topology) => topology,
        Err(error) => {
            out.problem(&format!("could not read this machine's topology: {error}"));
            return ExitCode::FAILURE;
        }
    };

    let places = match places_from_topology(&topology) {
        Ok(places) => places,
        Err(error) => {
            out.problem(&format!("could not read this machine's topology: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let plan = RunPlan::for_processors(&places);

    // Derived from the same topology as the plan, and read before the notice
    // rather than after the measurement, because the notice is what a runner
    // decides on and it cannot show a value it does not have.
    //
    // `core_affinity::measure` deliberately discovers again rather than being
    // handed these places, and that is not an oversight -- see its
    // documentation. A `measure_with(places)` seam would accept a processor list
    // whose *numbers* are valid on this host while its node labels are not, so
    // every pin would succeed and real timings would be filed under fabricated
    // labels. Its rows carry their own places, so each row says what it measured.
    //
    // That leaves two readings of one machine taken at different instants, which
    // is a real hazard rather than a theoretical one -- so the measurement
    // reports the shape it saw and the two are compared below before anything is
    // recorded. The seam stays closed; the skew it left behind is checked.
    let host = Fingerprint::from_topology(&topology);

    emit(out, &render_collection_notice(&machine, &host, policy));
    emit(out, &render_plan(&plan));

    if options.preview {
        out.line("");
        out.line("Preview only -- nothing was measured and no file was written.");
        out.line("Run again without --preview to take the measurement.");
        return ExitCode::SUCCESS;
    }

    out.line("");
    out.line("Measuring. This machine will be busy until it finishes.");
    out.line("");

    let observation = match core_affinity::measure() {
        Ok(observation) => observation,
        Err(error) => {
            out.problem(&format!("the measurement could not run: {error}"));
            return ExitCode::FAILURE;
        }
    };
    // The announced shape and the measured one must be the same shape, or the
    // record is a splice of two machines: `host` would come from the reading
    // above while every row came from the one `measure` took, with nothing in
    // the file saying so and a reader interpreting the rows through the wrong
    // machine. A processor going offline, or moving group or node, between the
    // notice and the end of the measurement is enough to produce it.
    //
    // Refusing rather than quietly recording the measured shape, because the
    // notice is what the runner read and consented to. A record that silently
    // describes a different machine than the one they were shown is the outcome
    // this tool's whole disclosure story exists to prevent -- and the run is
    // cheap to repeat, whereas a wrong record in a corpus is not.
    if observation.host != host {
        out.problem(
            "this machine changed while it was being measured, so the result was discarded.",
        );
        out.problem(&format!("  announced: {host}"));
        out.problem(&format!("  measured:  {}", observation.host));
        out.problem("nothing was written. Run again on an otherwise idle machine.");
        return ExitCode::FAILURE;
    }

    // Cannot fail: the equality was just checked above. Handled rather than
    // unwrapped anyway, because the constructor owns that invariant and a panic
    // here would discard a measurement the runner has already paid for.
    // The coherence of the reading `host` came from -- the one the runner was
    // shown in the notice -- rather than of the one `measure` took internally,
    // so the record's account of how this machine described itself matches the
    // shape it reports.
    let record = match SubmissionRecord::new(
        &observation,
        host,
        machine,
        policy,
        topology.coherence.clone(),
    ) {
        Ok(record) => record,
        Err(error) => {
            out.problem(&format!("the record could not be assembled: {error}"));
            return ExitCode::FAILURE;
        }
    };
    let text = match submission::render_submission(&record) {
        Ok(text) => text,
        Err(error) => {
            out.problem(&format!("the record could not be written out: {error}"));
            return ExitCode::FAILURE;
        }
    };

    if !options.no_file {
        write_backup(out, &record);
    }

    emit(out, &text);
    ExitCode::SUCCESS
}

/// State what is collected **before** the run, not after.
///
/// A person deciding whether to do this a favour should be able to decide with
/// the real values in front of them rather than a promise about them, which is
/// why the preview exists and why this prints what was actually read.
///
/// # A withheld field shows as withheld, and no value is read to show it
///
/// Under the default policy the secondary rows say they are withheld rather
/// than showing what they would have contained. Reading a value only to preview
/// something the record will not carry would contradict the module's promise
/// that a withheld field is never read at all -- and there is nothing for the
/// runner to judge in a value that is not being sent. The row a runner does
/// need to judge, the topology, is always shown, because it is always sent.
fn render_collection_notice(
    machine: &MachineDescription,
    host: &Fingerprint,
    policy: MetadataPolicy,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "== windows-placement-probe ==");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "This measures what thread placement costs on your machine, and prints"
    );
    let _ = writeln!(
        out,
        "a result you can paste into a discussion thread. It makes no network"
    );
    let _ = writeln!(
        out,
        "connections; sending the result is your decision and your action."
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "What this run will put in the record, as read just now:"
    );
    let _ = writeln!(
        out,
        "  cpu model      {}",
        match (&machine.cpu_model, machine.model_suppressed) {
            (Some(model), _) => model.as_str(),
            (None, true) => "(withheld)",
            (None, false) => "(this host would not say)",
        }
    );
    let _ = writeln!(
        out,
        "  os build       {}",
        match (&machine.os_build, machine.os_build_suppressed) {
            (Some(build), _) => build.as_str(),
            (None, true) => "(withheld)",
            (None, false) => "(this host would not say)",
        }
    );
    // Parenthesised when withheld, so the column reads the same way as the two
    // rows above it. The hint's own `Display` stays a plain word, because it is
    // the rendering of a value rather than of this table's cell.
    let _ = writeln!(
        out,
        "  virtualisation {}",
        match (machine.virtualisation, &machine.virtualisation_name) {
            (VirtualisationHint::Suppressed, _) => "(withheld)".to_owned(),
            (hint, Some(name)) => format!("{hint} ({name})"),
            (hint, None) => hint.to_string(),
        }
    );
    let _ = writeln!(
        out,
        "  run time       {}",
        if policy.includes_timestamp() {
            "the minute this run finished, in UTC"
        } else {
            "(withheld)"
        }
    ); // **The value, not the category.** Every other row here shows what was
    // actually read, and this one named a subject instead -- while the
    // paragraph below warns that the topology identifies the part whether or
    // not the model is named. A runner asked to judge that could not see the
    // thing they were being asked to judge, which is the one job the preview
    // has.
    let _ = writeln!(out, "  topology       {host}");
    let _ = writeln!(
        out,
        "                 (processor, core, cache and NUMA layout)"
    );
    let _ = writeln!(
        out,
        "  timings        how long a handoff takes at each placement"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "What it does NOT collect: your host name, your user name, file paths,"
    );
    let _ = writeln!(
        out,
        "environment variables, serial numbers, or anything about installed"
    );
    let _ = writeln!(
        out,
        "software. Read the printed record before sending it -- if you are not"
    );
    let _ = writeln!(out, "happy with something in it, do not send it.");
    let _ = writeln!(out);
    if policy.includes_anything() {
        let _ = writeln!(
            out,
            "You passed --include-metadata, so the rows above that this machine"
        );
        let _ = writeln!(
            out,
            "would answer are being sent. Thank you -- they are what lets a result"
        );
        let _ = writeln!(out, "be tied to an OS build or a hypervisor.");
        if !machine.model_suppressed {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Pass --no-cpu-model to withhold just the model. Note that it does not"
            );
            let _ = writeln!(
                out,
                "make confidential hardware safe to submit: the topology describes the"
            );
            let _ = writeln!(out, "part whether or not it is named.");
        }
    } else {
        let _ = writeln!(
            out,
            "Everything above except the topology and the timings is withheld by"
        );
        let _ = writeln!(
            out,
            "default. Pass --include-metadata to send it too: a defect that appears"
        );
        let _ = writeln!(
            out,
            "only on one OS build, or only under one hypervisor, can only be found"
        );
        let _ = writeln!(out, "when somebody sends that context.");
    }
    out
}
fn render_plan(plan: &RunPlan) -> String {
    let mut out = String::new();
    let _ = writeln!(out);
    let _ = writeln!(out, "-- what this run will do --");
    let _ = writeln!(out, "  {:>3} placement(s) on this machine", plan.placements);
    let _ = writeln!(out, "  {:>3} NUMA node pair(s)", plan.node_hops);
    let _ = writeln!(out, "  {:>3} efficiency class comparison(s)", plan.classes);
    let _ = writeln!(
        out,
        "  {:>3} timed handoffs in total ({} strategies x {} repetitions)",
        plan.timed_runs(),
        plan.strategies,
        plan.repetitions
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  Should take under {:.0} seconds, and usually much less. That is an",
        plan.estimated_seconds().ceil().max(1.0)
    );
    let _ = writeln!(
        out,
        "  upper bound taken from the slowest machine measured so far -- how long"
    );
    let _ = writeln!(
        out,
        "  a handoff takes is the thing being measured, so it cannot be exact."
    );
    out
}

/// Write the record beside the report, as a convenience rather than a step.
///
/// A failure here is reported and does not fail the run: the submission is the
/// text on screen, and losing the backup copy costs nothing that matters.
fn write_backup(out: &mut impl Sink, record: &SubmissionRecord) {
    // The same layout as the printed record, so the JSON a runner attaches and
    // the JSON embedded in the text they paste are byte-identical.
    //
    // The two artifacts are not: the terminal text also carries the
    // instructions, the human-readable report, the checksum line and the
    // markdown fences. Saying otherwise promised an equivalence a collector
    // might rely on -- diffing a pasted comment against an attached file would
    // report a difference on every submission.
    let json = match windows_placement_probe::paste_json::to_paste_json(record) {
        Ok(json) => json,
        Err(error) => {
            out.line(&format!(
                "(could not serialize the record to a file: {error})"
            ));
            return;
        }
    };

    // The report stream, not the problem stream, and deliberately so: the
    // backup is a convenience, the submission is the text on screen, and a
    // failure here belongs in the narrative the runner is reading rather than
    // on a stream they may not see.
    match write_backup_to_new_file(&submission::file_name(record), &json) {
        Ok(name) => out.line(&format!(
            "(a copy of the record was also written to {name})"
        )),
        Err(error) => out.line(&format!(
            "(could not write the backup: {error} -- paste the text below)"
        )),
    }
}

/// Write `json` to a file that did not already exist, and return its name.
///
/// **`create_new`, not `write`, and this is a correction.** The name carries a
/// timestamp so that a second run does not overwrite a first, but the stamp has
/// one-second resolution and `fs::write` truncates whatever it finds. A run
/// takes well under a second on a small machine, so two of them could land in
/// the same second and the second would silently destroy the first -- exactly
/// the loss the naming scheme promised to prevent, and worst for someone
/// re-running the tool because they were unsure the first result was good.
///
/// Exclusive creation makes the collision visible instead of silent, and a
/// suffix resolves it. The suffix is only reached on a real collision, so the
/// ordinary name stays the predictable one.
///
/// **The record's name never exists half-written, and this is a second
/// correction -- twice over.** Writing straight into the final name leaves a
/// truncated `.json` behind when the write fails, indistinguishable to a
/// collector from a complete record. Reserving the final name with an empty file
/// and renaming onto it afterwards fixes only half of that: the empty
/// reservation is *itself* visible under the record's name for the whole
/// duration of the write, and a process killed in that window leaves it there
/// permanently.
///
/// So the content is written to a temporary and **published with a single
/// atomic no-replace move**. The final name comes into existence already
/// complete, or not at all. `std::fs::rename` cannot be used for this: on
/// Windows it always passes `MOVEFILE_REPLACE_EXISTING`, so it would silently
/// clobber a record another run had placed while this one was writing --
/// destroying the very collision guarantee the suffix exists to provide.
fn write_backup_to_new_file(name: &str, json: &str) -> std::io::Result<String> {
    write_backup_with(name, json, |file, bytes| {
        std::io::Write::write_all(file, bytes)
    })
}

/// The body of [`write_backup_to_new_file`], with the write itself injectable.
///
/// The failure this guards is a write that fails after the temporary exists, and
/// no test can provoke that by filling the disk. The seam is the smallest thing
/// that makes it reachable: a test supplies a writer that fails, and asserts
/// that nothing is left behind under any name.
fn write_backup_with(
    name: &str,
    json: &str,
    mut write: impl FnMut(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<String> {
    /// Enough to outlast any plausible burst of same-second runs; past this,
    /// failing is better than looping while a caller waits.
    const MAX_ATTEMPTS: u32 = 100;

    // Written first and in full, so every candidate below is offered a file that
    // is already complete. Publication is then a single move per candidate,
    // rather than a window in which a name exists but its content does not.
    let (temporary, outcome) = write_temporary(name, json, &mut write)?;
    if let Err(error) = outcome {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    for attempt in 0..MAX_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            match name.strip_suffix(".json") {
                Some(stem) => format!("{stem}-{attempt}.json"),
                None => format!("{name}-{attempt}"),
            }
        };

        match publish(&temporary, &candidate) {
            Ok(()) => return Ok(candidate),
            // Someone else has this name. Not a failure yet: try the next.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                let _ = std::fs::remove_file(&temporary);
                return Err(error);
            }
        }
    }

    let _ = std::fs::remove_file(&temporary);
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{MAX_ATTEMPTS} names starting from {name} were all taken"),
    ))
}

/// Create a temporary beside `name` and fill it, returning its path and the
/// write's outcome.
///
/// The path is returned even when the write failed, so the caller can remove it:
/// a temporary left behind is litter under a name a collector might not
/// recognise, which is only marginally better than litter under one it would.
///
/// # Why the process id is not enough
///
/// A hard-killed run leaves its `.partial` behind, and Windows reuses process
/// ids. A later run issued the same id would find the corpse under the only name
/// it would ever try, and `create_new` would fail with `AlreadyExists` -- from
/// *here*, before the caller's suffix loop is reached, so the whole backup would
/// fail rather than land under a next-best name. The id still separates
/// concurrent runs cheaply; the counter is what survives a stale one.
fn write_temporary(
    name: &str,
    json: &str,
    write: &mut impl FnMut(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<(String, std::io::Result<()>)> {
    /// Matches the caller's final-name budget: the two loops fail for the same
    /// reason and there is no cause to give one more patience than the other.
    const MAX_ATTEMPTS: u32 = 100;

    let id = std::process::id();
    let mut last = None;
    for attempt in 0..MAX_ATTEMPTS {
        // The `.partial` suffix keeps the file out of any `*.json` collection.
        let temporary = if attempt == 0 {
            format!("{name}.{id}.partial")
        } else {
            format!("{name}.{id}-{attempt}.partial")
        };
        match std::fs::File::create_new(&temporary) {
            Ok(mut file) => {
                let outcome = write(&mut file, json.as_bytes()).and_then(|()| file.sync_all());
                drop(file);
                return Ok((temporary, outcome));
            }
            // Taken -- by a live concurrent run, or by the remains of a dead one
            // that held this id first. Either way the next name is untried.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => last = Some(error),
            Err(error) => return Err(error),
        }
    }

    Err(last.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{MAX_ATTEMPTS} temporaries starting from {name}.{id}.partial were all taken"),
        )
    }))
}

/// Move `temporary` onto `final_name`, failing rather than replacing.
///
/// # Why not `std::fs::rename`
///
/// On Windows it passes `MOVEFILE_REPLACE_EXISTING`, so it would overwrite a
/// record another run had already written -- silently undoing the collision
/// handling the caller's suffix loop exists to provide. Without that flag
/// `MoveFileExW` fails with `ERROR_ALREADY_EXISTS` instead, which is exactly the
/// signal the loop wants, and the move is atomic: the destination appears
/// complete or does not appear.
fn publish(temporary: &str, final_name: &str) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;

    fn wide(path: &str) -> Vec<u16> {
        std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let from = wide(temporary);
    let to = wide(final_name);
    // SAFETY: both pointers address NUL-terminated wide strings that outlive the
    // call, and the flags word is zero, which is the documented "fail if the
    // destination exists" behaviour rather than a sentinel.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    };
    if moved == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn parse_arguments() -> Result<Options, String> {
    let mut options = Options {
        preview: false,
        include_metadata: false,
        suppress_model: false,
        no_file: false,
        version: false,
        help: false,
    };

    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--preview" => options.preview = true,
            "--include-metadata" => options.include_metadata = true,
            "--no-cpu-model" => options.suppress_model = true,
            "--no-file" => options.no_file = true,
            "--version" | "-V" => options.version = true,
            "--help" | "-h" => options.help = true,
            other => {
                return Err(format!("unrecognised argument {other:?}\n\n{}", help()));
            }
        }
    }

    Ok(options)
}

fn help() -> String {
    format!(
        "windows-placement-probe -- measures what thread placement costs\n\
         \n\
         USAGE:\n\
         \x20   placement-probe [OPTIONS]\n\
         \n\
         OPTIONS:\n\
         \x20   --preview            Show what would be collected and measure nothing.\n\
         \x20   --include-metadata   Also send the run time, the CPU model, the OS\n\
         \x20                        build and the virtualisation hint, all of which\n\
         \x20                        are withheld by default.\n\
         \x20   --no-cpu-model       Withhold the CPU model from the record. Only\n\
         \x20                        does anything beside --include-metadata.\n\
         \x20   --no-file            Do not write the backup copy of the record.\n\
         \x20   -V, --version        Print this build's identity and exit.\n\
         \x20   -h, --help           Print this message.\n\
         \n\
         Results are collected at:\n\
         \x20   {DISCUSSION_URL}\n"
    )
}
