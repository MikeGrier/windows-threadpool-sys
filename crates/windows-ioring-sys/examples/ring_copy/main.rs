// Copyright (c) 2026 Mike Grier
//! `ring-copy` (M7): a topology-aligned sample copying one file to another
//! through per-domain `IoRing`s, pinned threads, and NUMA-placed registered
//! buffers.
//!
//! This is a **sample**, not library surface: `windows-ioring-sys` owns no
//! partitioning policy (D-8 in its `DESIGN-NOTES.md`), so the policy lives
//! here instead, giving M6's guidance something executable behind it.

mod buffer;
mod engine;
mod plan;
mod policy;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::PathBuf;

use policy::Policy;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
};
use windows_topology_sys::MachineMemoryTopology;

const DEFAULT_CHUNK_LEN: usize = 1024 * 1024;

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
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut policy = Policy::ByL3;
    let mut remote_placement = false;
    let mut topology_path = None;
    let mut chunk_len = DEFAULT_CHUNK_LEN;

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
            other => positional.push(other.to_string()),
        }
    }

    let mut positional = positional.into_iter();
    let source = positional.next().ok_or(
        "usage: ring_copy <source> <destination> [--policy NAME] [--placement local|remote] \
         [--topology PATH] [--chunk-size BYTES]",
    )?;
    let destination = positional.next().ok_or("missing <destination>")?;

    Ok(Args {
        source: source.into(),
        destination: destination.into(),
        policy,
        remote_placement,
        topology_path,
        chunk_len,
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
        } else if classified
            .iter()
            .any(|node| matches!(node, plan::RemoteNode::Other(_)))
        {
            plan::RemoteNode::Other(0)
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
            plan::RemoteNode::SameAsLocal | plan::RemoteNode::Other(_) => {
                let any_remote = classified
                    .iter()
                    .any(|node| matches!(node, plan::RemoteNode::Other(_)));
                if !any_remote {
                    report.line(format_args!(
                        "note: no domain has a NUMA node other than its own, so there is nothing \
                         remote to place on; --placement remote measures the same placement as \
                         --placement local on this machine"
                    ));
                }
            }
        }
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
