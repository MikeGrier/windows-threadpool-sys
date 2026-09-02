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
use windows_placement_probe::fingerprint::{Fingerprint, places_from_topology};
use windows_placement_probe::machine::MachineDescription;
use windows_placement_probe::record::SubmissionRecord;
use windows_placement_probe::submission::{self, DISCUSSION_URL};
use windows_topology_sys::Topology;

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

    // **One discovery, two derivations.** The announced plan and the recorded
    // fingerprint used to come from separate `Topology::discover()` calls, so a
    // processor going offline between them would have the notice describing one
    // machine and the record another, with nothing in the output saying which
    // was which. Both now come from this reading.
    let topology = match Topology::discover() {
        Ok(topology) => topology,
        Err(error) => {
            eprintln!("could not read this machine's topology: {error}");
            return ExitCode::FAILURE;
        }
    };

    let places = match places_from_topology(&topology) {
        Ok(places) => places,
        Err(error) => {
            eprintln!("could not read this machine's topology: {error}");
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
    let host = Fingerprint::from_topology(&topology);

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
fn write_temporary(
    name: &str,
    json: &str,
    write: &mut impl FnMut(&mut std::fs::File, &[u8]) -> std::io::Result<()>,
) -> std::io::Result<(String, std::io::Result<()>)> {
    // The process id keeps two concurrent runs from colliding here, and the
    // `.partial` suffix keeps the file out of any `*.json` collection.
    let temporary = format!("{name}.{}.partial", std::process::id());
    let mut file = std::fs::File::create_new(&temporary)?;
    let outcome = write(&mut file, json.as_bytes()).and_then(|()| file.sync_all());
    drop(file);
    Ok((temporary, outcome))
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
