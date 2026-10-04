// Copyright (c) 2026 Mike Grier
//! `ring-copy` (M7): a topology-aligned sample copying one file to another
//! through per-domain `IoRing`s, pinned threads, and NUMA-placed registered
//! buffers.
//!
//! This is a **sample**, not library surface: `windows-ioring-sys` owns no
//! partitioning policy (D-8 in its `DESIGN-NOTES.md`), so the policy lives
//! here instead, giving M6's guidance something executable behind it.

mod engine;
mod plan;
mod policy;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use policy::Policy;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};
use windows_topology_sys::MachineMemoryTopology;

const DEFAULT_CHUNK_LEN: usize = 1024 * 1024;

/// Rounds `--compare` runs when `--rounds` is not given.
///
/// One complete rotation over the policies: the smallest count that gives every
/// policy each position in the running order exactly once. A partial cycle
/// leaves the order unbalanced -- at three rounds over five policies, two of
/// them never run first -- and position is not free even after the warm-up
/// pass, because a machine that drifts during the run drifts against whoever
/// holds the later slots.
///
/// It is also comfortably more than one, so a policy has a fastest, a slowest
/// and a middle: one run cannot show a spread, and a spread is the only handle
/// this sample offers on whether two arrangements were distinguished at all.
const DEFAULT_ROUNDS: usize = Policy::ALL.len();

/// The single sink every line of this sample's output goes through
/// (repository "Architectural pre-steps" rule: never call `println!`/
/// `eprintln!` from more than one call site -- introduce an abstraction at
/// the first occurrence instead). `out` carries ordinary progress and
/// results; `err` carries failures. Both are plain `Write` streams so a
/// future caller could redirect either without touching any call site
/// below.
struct Report<O, E> {
    out: O,
    err: E,
}

impl<O: io::Write, E: io::Write> Report<O, E> {
    fn new(out: O, err: E) -> Self {
        Self { out, err }
    }

    /// Write one line of ordinary output.
    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.out, "{args}");
    }

    /// Write one line of error output.
    fn error_line(&mut self, args: std::fmt::Arguments<'_>) {
        let _ = writeln!(self.err, "{args}");
    }
}

/// A raw handle the sample hands to more than one pinned thread.
///
/// Each domain thread only ever reads or writes its own, disjoint byte
/// range, through `IoRing`'s own explicit-offset ops -- never through a
/// shared file position -- so concurrent use of the same handle is sound;
/// this wrapper exists purely to assert that to the compiler, since a raw
/// pointer is not `Send` on its own.
#[derive(Clone, Copy)]
struct SendHandle(HANDLE);

// SAFETY: see the type's own doc comment.
unsafe impl Send for SendHandle {}

struct Args {
    source: PathBuf,
    destination: PathBuf,
    policy: Policy,
    remote_placement: bool,
    topology_path: Option<PathBuf>,
    chunk_len: usize,
    /// Price every policy, not just the chosen one (`M27.3`).
    compare: bool,
    /// How many times each policy is run in `--compare`.
    rounds: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut policy = Policy::ByCache;
    let mut remote_placement = false;
    let mut topology_path = None;
    let mut chunk_len = DEFAULT_CHUNK_LEN;
    let mut compare = false;
    let mut rounds = DEFAULT_ROUNDS;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--policy" => {
                let value = args.next().ok_or("--policy needs a value")?;
                policy =
                    Policy::parse(&value).ok_or_else(|| format!("unknown policy {value:?}"))?;
            }
            "--placement" => {
                let value = args.next().ok_or("--placement needs a value")?;
                remote_placement = match value.as_str() {
                    "local" => false,
                    "remote" => true,
                    other => {
                        return Err(format!(
                            "unknown placement {other:?} (expected local or remote)"
                        ));
                    }
                };
            }
            "--topology" => {
                topology_path = Some(PathBuf::from(
                    args.next().ok_or("--topology needs a value")?,
                ));
            }
            "--chunk-size" => {
                let value = args.next().ok_or("--chunk-size needs a value")?;
                let parsed: usize = value
                    .parse()
                    .map_err(|_| format!("invalid --chunk-size {value:?}"))?;
                if parsed == 0 || parsed > u32::MAX as usize {
                    return Err(format!(
                        "--chunk-size {parsed} must be between 1 and {} bytes",
                        u32::MAX
                    ));
                }
                chunk_len = parsed;
            }
            "--compare" => compare = true,
            "--rounds" => {
                let value = args.next().ok_or("--rounds needs a value")?;
                rounds = value
                    .parse()
                    .map_err(|_| format!("invalid --rounds {value:?}"))?;
                if rounds == 0 {
                    return Err("--rounds must be at least 1".to_string());
                }
            }
            other => positional.push(other.to_string()),
        }
    }

    let mut positional = positional.into_iter();
    let source = positional.next().ok_or(
        "usage: ring_copy <source> <destination> [--policy NAME] [--placement local|remote] \
         [--topology PATH] [--chunk-size BYTES] [--compare] [--rounds N]",
    )?;
    let destination = positional.next().ok_or("missing <destination>")?;

    Ok(Args {
        source: source.into(),
        destination: destination.into(),
        policy,
        remote_placement,
        topology_path,
        chunk_len,
        compare,
        rounds,
    })
}

fn load_topology(path: Option<&PathBuf>) -> io::Result<MachineMemoryTopology> {
    match path {
        Some(path) => {
            let file = std::fs::File::open(path)?;
            serde_json::from_reader(file).map_err(io::Error::other)
        }
        None => MachineMemoryTopology::discover(),
    }
}

/// Whether `a` and `b` are open handles onto the same file (PR #20 review
/// response), including two different paths that reach it via a hard link --
/// a plain path comparison would miss that case entirely.
///
/// Identity is the volume serial number plus the 64-bit file index
/// (`nFileIndexHigh`/`nFileIndexLow`), which Windows guarantees is unique per
/// volume for the life of a file; comparing paths cannot detect a hard link
/// to the same file under a different name.
fn same_file(a: &std::fs::File, b: &std::fs::File) -> io::Result<bool> {
    fn identity(file: &std::fs::File) -> io::Result<(u32, u64)> {
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        // SAFETY: `file`'s handle is live for the duration of this call, and
        // `info` is a valid, exclusively-borrowed out-parameter of the exact
        // type the API expects.
        let ok = unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        Ok((info.dwVolumeSerialNumber, index))
    }
    Ok(identity(a)? == identity(b)?)
}

/// What one arrangement cost on this machine.
struct ArrangementRun {
    policy: Policy,
    domains: usize,
    degraded: bool,
    /// Wall time for the whole arrangement, not the sum of its domains.
    ///
    /// The domains run concurrently, so summing them would count the same
    /// seconds once per domain and make a wider arrangement look slower the
    /// more parallelism it was given.
    wall: Duration,
    bytes: u64,
}

/// Run one arrangement end to end and time it.
///
/// Extracted from `main` so `--compare` can price the alternatives through the
/// identical path the chosen policy takes. A comparison whose arms do not share
/// a code path measures the difference between the arms' code as much as
/// between the arrangements.
#[allow(clippy::too_many_arguments)]
fn run_arrangement(
    policy: Policy,
    topology: &MachineMemoryTopology,
    remote_placement: bool,
    source: SendHandle,
    destination: SendHandle,
    source_len: u64,
    chunk_len: usize,
) -> io::Result<ArrangementRun> {
    let (domains, degraded) = policy.select(topology);
    let plans = plan::build_plan(topology, &domains)?;
    let domain_count = plans.len() as u64;
    let per_domain = source_len.div_ceil(domain_count.max(1));

    // Remote placement is resolved and validated HERE, over this arrangement's
    // own plans, before anything is timed.
    //
    // `main` validates remoteness only for the single policy `--policy` named,
    // and `--compare` runs every policy. A plan that could not name a remote
    // node used to fall back to local placement inside the spawn below, while
    // its row was still reported as a remote arm -- a measurement labelled as
    // something it was not, which is worse than no measurement. Refusing is the
    // honest answer, and refusing here covers both callers rather than only the
    // one that happened to check.
    let placement: Vec<_> = if remote_placement {
        let mut resolved = Vec::with_capacity(plans.len());
        for domain_plan in &plans {
            match plan::remote_numa_node(topology, domain_plan.local_numa_node) {
                plan::RemoteNode::Other(node) => resolved.push(Some(node)),
                refused => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        format!(
                            "{policy:?}: remote placement was requested, but domain {} cannot \
                             name a node to be remote from ({refused:?}). Refusing rather than \
                             running it locally and reporting the row as remote.",
                            domain_plan.label
                        ),
                    ));
                }
            }
        }
        resolved
    } else {
        plans.iter().map(|plan| plan.local_numa_node).collect()
    };

    let started = Instant::now();
    let reports: Vec<io::Result<engine::DomainReport>> = std::thread::scope(|scope| {
        let handles: Vec<_> = plans
            .iter()
            .enumerate()
            .map(|(index, domain_plan)| {
                let start = per_domain * index as u64;
                let end = (start + per_domain).min(source_len);
                let numa_node = placement[index];
                scope.spawn(move || {
                    let source = source;
                    let destination = destination;
                    engine::copy_domain(
                        domain_plan,
                        source.0,
                        destination.0,
                        start..end,
                        chunk_len,
                        numa_node,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("domain thread panicked"))
            .collect()
    });
    let wall = started.elapsed();

    let mut bytes = 0;
    for report in reports {
        bytes += report?.bytes_copied;
    }

    Ok(ArrangementRun {
        policy,
        domains: plans.len(),
        degraded,
        wall,
        bytes,
    })
}

fn throughput_mib_s(bytes: u64, wall: Duration) -> f64 {
    (bytes as f64 / (1024.0 * 1024.0)) / wall.as_secs_f64().max(f64::EPSILON)
}

/// Price every policy on this machine and report what each cost.
///
/// # This reports; it does not conclude
///
/// No arm is marked as best and no recommendation is printed, per OPTION
/// INTEGRITY. Which arrangement a consumer wants depends on what they value --
/// throughput, isolation, predictability under load -- and this sample cannot
/// know that. It says what happened on the machine in hand and stops.
fn compare_arrangements(
    report: &mut Report<io::Stdout, io::Stderr>,
    args: &Args,
    topology: &MachineMemoryTopology,
    source: SendHandle,
    destination: SendHandle,
    source_len: u64,
) -> io::Result<()> {
    let policies = Policy::ALL;
    let mut runs: Vec<Vec<ArrangementRun>> = (0..policies.len()).map(|_| Vec::new()).collect();

    report.line(format_args!(
        "comparing {} arrangements over {} round(s):",
        policies.len(),
        args.rounds
    ));

    if !args.rounds.is_multiple_of(policies.len()) {
        report.line(format_args!(
            "  note: {} rounds is not a whole number of rotations over {} policies, so each \
             policy does not get every position in the running order; pass --rounds as a \
             multiple of {} for a complete rotation",
            args.rounds,
            policies.len(),
            policies.len()
        ));
    }

    // An untimed pass over every policy, before anything is recorded.
    //
    // Rotating the order does NOT distribute the cold-cache effect, which is
    // what this comment used to claim. Nothing resets the cache between runs,
    // so across the whole comparison there is exactly ONE cold run -- the very
    // first -- and it lands on whichever policy happens to go first. Rotation
    // changes which policy that is from round to round, but round 0 still has a
    // uniquely cold sample, and it surfaces in exactly one policy's `slowest`
    // figure and nowhere else.
    //
    // Discarding a full pass fixes that at the source rather than redistributing
    // it: every recorded sample is then taken against a warm cache. The cost is
    // one extra pass per policy.
    for policy in policies {
        let _ = run_arrangement(
            policy,
            topology,
            args.remote_placement,
            source,
            destination,
            source_len,
            args.chunk_len,
        )?;
    }

    for round in 0..args.rounds {
        // The order still rotates, for an effect the warm-up does not cover: a
        // machine that drifts during the run -- thermal, or another tenant
        // arriving -- would otherwise hand that drift to whichever policy holds
        // a fixed slot. Rotating does not remove drift; it stops it being
        // *attributed* to one arm.
        for offset in 0..policies.len() {
            let index = (offset + round) % policies.len();
            let run = run_arrangement(
                policies[index],
                topology,
                args.remote_placement,
                source,
                destination,
                source_len,
                args.chunk_len,
            )?;
            runs[index].push(run);
        }
    }

    report.line(format_args!(""));
    report.line(format_args!(
        "  {:<10} {:>8}  {:>15}  {:>15}  {:>15}",
        "policy", "domains", "fastest", "slowest", "median"
    ));

    for per_policy in &runs {
        let mut rates: Vec<f64> = per_policy
            .iter()
            .map(|run| throughput_mib_s(run.bytes, run.wall))
            .collect();
        rates.sort_by(f64::total_cmp);
        // `rates.len() / 2` alone is the median only for an odd count, and
        // `--rounds` takes any positive value -- a balanced ten-round run is
        // even. The upper middle sample is not the median of an even set.
        let median = if rates.len().is_multiple_of(2) {
            let upper = rates.len() / 2;
            (rates[upper - 1] + rates[upper]) / 2.0
        } else {
            rates[rates.len() / 2]
        };
        let first = &per_policy[0];

        // Named from the run rather than from a parallel array, so a row cannot
        // come to label itself with a policy it did not use.
        report.line(format_args!(
            "  {:<10} {:>8}  {:>9.1} MiB/s  {:>9.1} MiB/s  {:>9.1} MiB/s{}",
            format!("{:?}", first.policy),
            first.domains,
            rates[rates.len() - 1],
            rates[0],
            median,
            if first.degraded { "  (degraded)" } else { "" }
        ));
    }

    report.line(format_args!(""));
    report.line(format_args!(
        "what these figures include: the filesystem cache, whatever else this machine was doing, \
         and this sample's own chunking. They are what the arrangements cost here, today, on this \
         file -- not a property of the policies."
    ));
    report.line(format_args!(
        "what they cannot separate: an arrangement that is genuinely better from one that ran \
         while the machine was quieter. The spread between fastest and slowest within a policy is \
         the handle on that -- where it overlaps another policy's spread, this run did not \
         distinguish them."
    ));
    report.line(format_args!(
        "no arm is marked best, deliberately. Which of these a consumer wants depends on what \
         they value, and that is theirs to decide against their own workload."
    ));

    // `Policy::Single` is excluded deliberately. One domain is what that policy
    // IS, by definition, so counting it here made the note below report that the
    // host could not express the one arrangement it had expressed exactly --
    // every run, on every machine. What remains is worth saying, but it is
    // "collapsed to the same shape as `single`", not "could not be expressed":
    // a one-node, one-package or one-core host resolves another policy to a
    // single domain legitimately, and that is the machine's topology showing
    // through rather than a failure.
    let collapsed: Vec<&ArrangementRun> = runs
        .iter()
        .filter_map(|per_policy| per_policy.first())
        .filter(|run| run.policy != Policy::Single && (run.degraded || run.domains == 1))
        .collect();
    if !collapsed.is_empty() {
        report.line(format_args!(""));
        report.line(format_args!(
            "note: {} of these policies fell back to, or resolved to, a single domain on this \
             host -- the same shape `single` has by definition. That is this machine's topology \
             showing through rather than a property of the policy, and it means those rows are \
             not independent measurements of different arrangements.",
            collapsed.len()
        ));
    }

    Ok(())
}

fn main() -> io::Result<()> {
    let mut report = Report::new(io::stdout(), io::stderr());

    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            report.error_line(format_args!("{message}"));
            std::process::exit(2);
        }
    };

    let topology = load_topology(args.topology_path.as_ref())?;
    let (domains, degraded) = args.policy.select(&topology);
    if degraded {
        report.line(format_args!(
            "note: {:?} found nothing to select on this topology; falling back to one whole-machine domain",
            args.policy
        ));
    }

    let plans = plan::build_plan(&topology, &domains)?;
    report.line(format_args!("{} domain(s) selected:", plans.len()));
    for domain_plan in &plans {
        report.line(format_args!(
            "  {} -- group {} mask {:#x}, local NUMA node {:?}",
            domain_plan.label, domain_plan.group, domain_plan.mask, domain_plan.local_numa_node
        ));
    }

    let source_file = std::fs::File::open(&args.source)?;
    let source_len = source_file.metadata()?.len();
    // Opened without truncation (PR #20 review response): truncating via
    // `OpenOptions::truncate` before checking identity would destroy the
    // source's content the instant `source` and `destination` name the same
    // file, including through a hard link -- the already-open source handle
    // would then read back the zeroed tail this call just produced. Identity
    // is compared below, on these untouched handles, and only once they are
    // confirmed distinct does `set_len` resize the destination in place --
    // which is also what makes omitting `truncate`/`append` deliberate here,
    // not an oversight the lint below would otherwise (correctly) flag.
    #[allow(clippy::suspicious_open_options)]
    let destination_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&args.destination)?;
    if same_file(&source_file, &destination_file)? {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "source and destination name the same file (directly or via a hard link); \
             refusing to copy a file onto itself",
        ));
    }
    destination_file.set_len(source_len)?;

    let source_handle = SendHandle(source_file.as_raw_handle());
    let destination_handle = SendHandle(destination_file.as_raw_handle());

    // Settled once, before any thread starts, because both answers are about
    // the topology rather than about a domain -- and because the refusal must
    // happen before the copy rather than per-chunk inside it.
    if args.remote_placement {
        // Three questions, asked at the level each belongs to. Answering only
        // the first was a defect caught by running this; then routing the first
        // through the per-domain function was a second one, which refused every
        // run including on a live machine.
        //
        // Machine level: does anything name a node at all? A restored
        // description names none, because deserialization drops the
        // observations that carry them.
        //
        // Domain level: is this domain's own node known, and is there a
        // different one? Neither can be asked of the machine, because both are
        // relative to one domain.
        let classified: Vec<plan::RemoteNode> = plans
            .iter()
            .map(|domain_plan| plan::remote_numa_node(&topology, domain_plan.local_numa_node))
            .collect();
        let machine_names_nodes = plan::names_any_numa_node(&topology);
        let outcome = if !machine_names_nodes {
            plan::RemoteNode::Unnamed
        } else if let Some(unknown) = classified
            .iter()
            .find(|node| matches!(node, plan::RemoteNode::LocalUnknown))
        {
            *unknown
        } else if let Some(remote) = classified
            .iter()
            .find(|node| matches!(node, plan::RemoteNode::Other(_)))
        {
            // The node actually observed, not `Other(0)`. The payload is
            // unused just below, but a fabricated id in a value that carries
            // one is a trap for the next reader -- and inventing node 0 is
            // indistinguishable from observing it. Raised in the PR #61
            // review.
            *remote
        } else {
            plan::RemoteNode::SameAsLocal
        };
        match outcome {
            plan::RemoteNode::Unnamed => {
                report.error_line(format_args!(
                    "--placement remote needs a topology that names its NUMA nodes, and this one \
                     does not. A restored description (--topology) carries no node numbers: \
                     deserialization deliberately drops the observations that hold them, because \
                     a file cannot establish what the relationship walk saw. Refusing rather \
                     than placing locally, which would report a remote run that measured a local \
                     one. Drop --topology to measure this machine, or use --placement local."
                ));
                std::process::exit(2);
            }
            plan::RemoteNode::LocalUnknown => {
                report.error_line(format_args!(
                    "--placement remote needs to know which NUMA node each domain is local to, and \
                     this topology does not say. A node cannot be shown to be remote without one \
                     to be remote from, so proceeding would report a remote run on no evidence. \
                     Use --placement local, or a topology that names its nodes."
                ));
                std::process::exit(2);
            }
            // `SameAsLocal` *is* "no domain had another node" -- it is only
            // reached when the search above found no `Other`. The recomputed
            // `any_remote` this replaces asked the same question a second
            // time, which the fabricated `Other(0)` had made look necessary.
            plan::RemoteNode::SameAsLocal => {
                report.line(format_args!(
                    "note: no domain has a NUMA node other than its own, so there is nothing \
                     remote to place on; --placement remote measures the same placement as \
                     --placement local on this machine"
                ));
            }
            plan::RemoteNode::Other(_) => {}
        }
    }

    if args.compare {
        compare_arrangements(
            &mut report,
            &args,
            &topology,
            source_handle,
            destination_handle,
            source_len,
        )?;
        return Ok(());
    }

    let domain_count = plans.len() as u64;
    let per_domain = source_len.div_ceil(domain_count.max(1));

    let reports: Vec<io::Result<engine::DomainReport>> = std::thread::scope(|scope| {
        let handles: Vec<_> = plans
            .iter()
            .enumerate()
            .map(|(index, domain_plan)| {
                let start = per_domain * index as u64;
                let end = (start + per_domain).min(source_len);
                let numa_node = if args.remote_placement {
                    match plan::remote_numa_node(&topology, domain_plan.local_numa_node) {
                        plan::RemoteNode::Other(node) => Some(node),
                        // Both already reported above -- `Unnamed` exited, and
                        // `SameAsLocal` said that local is the only node there
                        // is. Neither may reach here as a silent substitution.
                        plan::RemoteNode::SameAsLocal
                        | plan::RemoteNode::Unnamed
                        | plan::RemoteNode::LocalUnknown => domain_plan.local_numa_node,
                    }
                } else {
                    domain_plan.local_numa_node
                };
                let chunk_len = args.chunk_len;
                scope.spawn(move || {
                    let source_handle = source_handle;
                    let destination_handle = destination_handle;
                    engine::copy_domain(
                        domain_plan,
                        source_handle.0,
                        destination_handle.0,
                        start..end,
                        chunk_len,
                        numa_node,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("domain thread panicked"))
            .collect()
    });

    let mut failed = false;
    report.line(format_args!(""));
    report.line(format_args!("results:"));
    for report_result in reports {
        match report_result {
            Ok(domain_report) => {
                let seconds = domain_report.elapsed.as_secs_f64().max(f64::EPSILON);
                let mib_per_sec = (domain_report.bytes_copied as f64 / (1024.0 * 1024.0)) / seconds;
                report.line(format_args!(
                    "  {}: {} bytes in {:?} ({mib_per_sec:.1} MiB/s)",
                    domain_report.label, domain_report.bytes_copied, domain_report.elapsed
                ));
            }
            Err(error) => {
                failed = true;
                report.error_line(format_args!("  domain failed: {error}"));
            }
        }
    }

    if plans.len() == 1 {
        report.line(format_args!(""));
        report.line(format_args!(
            "note: only one domain ran, so this cannot show a difference between policies or \
             buffer placements -- a single-domain or single-node machine produces noise here, \
             not a benchmark result (M7.5)."
        ));
    }

    if failed {
        return Err(io::Error::other("one or more domains failed"));
    }
    Ok(())
}
