// Copyright (c) 2026 Mike Grier
//! The one thing a runner is asked to execute.
//!
//! **One binary, one run, one paste.** "Run these three and send me all three
//! outputs" is friction for someone doing a favour, and invites partial
//! submissions that cannot be compared with each other.

use std::process::ExitCode;

#[cfg(test)]
mod tests;

use windows_placement_probe::build_identity::BuildIdentity;
use windows_placement_probe::core_affinity::{self, RunPlan};
use windows_placement_probe::fingerprint::{Fingerprint, discover_places};
use windows_placement_probe::machine::MachineDescription;
use windows_placement_probe::record::SubmissionRecord;
use windows_placement_probe::submission::{self, DISCUSSION_URL};

/// What the run was asked to do.
struct Options {
    /// Show what would be collected, and measure nothing.
    preview: bool,
    /// Withhold the CPU model.
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

fn main() -> ExitCode {
    let options = match parse_arguments() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    if options.help {
        // stdout and success: help was asked for and was given.
        println!("{}", help());
        return ExitCode::SUCCESS;
    }

    if options.version {
        // Deliberately the whole identity rather than just a version number.
        // CI asserts on this line that a released artifact reports itself
        // official, and a runner can check the same thing before trusting a
        // download -- both need the commit and the source, not just "0.1.0".
        println!("{}", BuildIdentity::current());
        return ExitCode::SUCCESS;
    }

    let machine = MachineDescription::read(options.suppress_model);

    let places = match discover_places() {
        Ok(places) => places,
        Err(error) => {
            eprintln!("could not read this machine's topology: {error}");
            return ExitCode::FAILURE;
        }
    };
    let plan = RunPlan::for_processors(&places);

    // Read before the notice, not after the measurement, because the notice is
    // what a runner decides on and it cannot show a value it does not have.
    // One reading serves both the notice and the record, so the two can never
    // describe different machines.
    let host = match Fingerprint::discover() {
        Ok(host) => host,
        Err(error) => {
            eprintln!("could not read this machine's shape: {error}");
            return ExitCode::FAILURE;
        }
    };

    print_collection_notice(&machine, &host, options.suppress_model);
    print_plan(&plan);

    if options.preview {
        println!();
        println!("Preview only -- nothing was measured and no file was written.");
        println!("Run again without --preview to take the measurement.");
        return ExitCode::SUCCESS;
    }

    println!();
    println!("Measuring. This machine will be busy until it finishes.");
    println!();

    let observation = match core_affinity::measure() {
        Ok(observation) => observation,
        Err(error) => {
            eprintln!("the measurement could not run: {error}");
            return ExitCode::FAILURE;
        }
    };
    let record = SubmissionRecord::new(&observation, host, machine);
    let text = match submission::render_submission(&record) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("the record could not be written out: {error}");
            return ExitCode::FAILURE;
        }
    };

    if !options.no_file {
        write_backup(&record);
    }

    print!("{text}");
    ExitCode::SUCCESS
}

/// State what is collected **before** the run, not after.
///
/// A person deciding whether to do this a favour should be able to decide with
/// the real values in front of them rather than a promise about them, which is
/// why the preview exists and why this prints what was actually read.
fn print_collection_notice(machine: &MachineDescription, host: &Fingerprint, suppressed: bool) {
    println!("== windows-placement-probe ==");
    println!();
    println!("This measures what thread placement costs on your machine, and prints");
    println!("a result you can paste into a discussion thread. It makes no network");
    println!("connections; sending the result is your decision and your action.");
    println!();
    println!("What it collects about this machine, as read just now:");
    println!(
        "  cpu model      {}",
        match (&machine.cpu_model, suppressed) {
            (Some(model), _) => model.as_str(),
            (None, true) => "(withheld: --no-cpu-model)",
            (None, false) => "(this host would not say)",
        }
    );
    println!(
        "  os build       {}",
        machine.os_build.as_deref().unwrap_or("(unknown)")
    );
    println!(
        "  virtualisation {}{}",
        machine.virtualisation,
        match &machine.virtualisation_name {
            Some(name) => format!(" ({name})"),
            None => String::new(),
        }
    );
    // **The value, not the category.** Every other row here shows what was
    // actually read, and this one named a subject instead -- while the
    // paragraph below warns that the topology identifies the part whether or
    // not the model is named. A runner asked to judge that could not see the
    // thing they were being asked to judge, which is the one job the preview
    // has.
    println!("  topology       {host}");
    println!("                 (processor, core, cache and NUMA layout)");
    println!("  timings        how long a handoff takes at each placement");
    println!();
    println!("What it does NOT collect: your host name, your user name, file paths,");
    println!("environment variables, serial numbers, or anything about installed");
    println!("software. Read the printed record before sending it -- if you are not");
    println!("happy with something in it, do not send it.");
    if !suppressed {
        println!();
        println!("Pass --no-cpu-model to withhold the model. Note that it does not make");
        println!("confidential hardware safe to submit: the topology describes the part");
        println!("whether or not it is named.");
    }
}

fn print_plan(plan: &RunPlan) {
    println!();
    println!("-- what this run will do --");
    println!("  {:>3} placement(s) on this machine", plan.placements);
    println!("  {:>3} NUMA node pair(s)", plan.node_hops);
    println!("  {:>3} efficiency class comparison(s)", plan.classes);
    println!(
        "  {:>3} timed handoffs in total ({} strategies x {} repetitions)",
        plan.timed_runs(),
        plan.strategies,
        plan.repetitions
    );
    println!();
    println!(
        "  Should take under {:.0} seconds, and usually much less. That is an",
        plan.estimated_seconds().ceil().max(1.0)
    );
    println!("  upper bound taken from the slowest machine measured so far -- how long");
    println!("  a handoff takes is the thing being measured, so it cannot be exact.");
}

/// Write the record beside the report, as a convenience rather than a step.
///
/// A failure here is reported and does not fail the run: the submission is the
/// text on screen, and losing the backup copy costs nothing that matters.
fn write_backup(record: &SubmissionRecord) {
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
            println!("(could not serialize the record to a file: {error})");
            return;
        }
    };

    match write_backup_to_new_file(&submission::file_name(record), &json) {
        Ok(name) => println!("(a copy of the record was also written to {name})"),
        Err(error) => println!("(could not write the backup: {error} -- paste the text below)"),
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
fn write_backup_to_new_file(name: &str, json: &str) -> std::io::Result<String> {
    /// Enough to outlast any plausible burst of same-second runs; past this,
    /// failing is better than looping while a caller waits.
    const MAX_ATTEMPTS: u32 = 100;

    for attempt in 0..MAX_ATTEMPTS {
        let candidate = if attempt == 0 {
            name.to_owned()
        } else {
            match name.strip_suffix(".json") {
                Some(stem) => format!("{stem}-{attempt}.json"),
                None => format!("{name}-{attempt}"),
            }
        };

        match std::fs::File::create_new(&candidate) {
            Ok(mut file) => {
                std::io::Write::write_all(&mut file, json.as_bytes())?;
                return Ok(candidate);
            }
            // Someone else has this name. Not a failure yet: try the next.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("{MAX_ATTEMPTS} names starting from {name} were all taken"),
    ))
}

fn parse_arguments() -> Result<Options, String> {
    let mut options = Options {
        preview: false,
        suppress_model: false,
        no_file: false,
        version: false,
        help: false,
    };

    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--preview" => options.preview = true,
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
         \x20   --preview        Show what would be collected and measure nothing.\n\
         \x20   --no-cpu-model   Withhold the CPU model from the record.\n\
         \x20   --no-file        Do not write the backup copy of the record.\n\
         \x20   -V, --version    Print this build's identity and exit.\n\
         \x20   -h, --help       Print this message.\n\
         \n\
         Results are collected at:\n\
         \x20   {DISCUSSION_URL}\n"
    )
}
