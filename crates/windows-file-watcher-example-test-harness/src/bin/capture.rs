// Copyright (c) 2026 Mike Grier
//! `capture`: run the seeded generator across a range of seeds against a
//! built-in example handler, and save every schedule that trips an oracle as a
//! JSON [`Recording`](windows_file_watcher_example_test_harness::Recording).
//!
//! This bin is a **handler-linked exemplar** (crate DESIGN-NOTES D-3): it always
//! drives [`example_handler::BuggyHandler`], a small, intentionally-buggy
//! handler shipped so this bin has something to find. Write your own capture
//! bin against your own `Handler` the same way: generate a schedule, `run` it,
//! and `save` whatever trips your handler's oracle.
//!
//! ```text
//! cargo run --bin capture -- [--seeds N] [--start N] [--out DIR]
//! ```
//!
//! [`example_handler`]: windows_file_watcher_example_test_harness::example_handler
//! [`example_handler::BuggyHandler`]: windows_file_watcher_example_test_harness::example_handler::BuggyHandler

fn main() {
    #[cfg(windows)]
    imp::main();
    #[cfg(not(windows))]
    eprintln!("windows-file-watcher-example-test-harness is Windows-only; nothing to do here.");
}

#[cfg(windows)]
mod imp {
    use std::io::{self, Write};
    use std::path::PathBuf;

    use windows_file_watcher_example_test_harness::{
        Generator, Recording, example_handler::BuggyHandler, run,
    };

    pub fn main() {
        let args = Args::parse();
        // All reporting is routed through one writer (repository architecture
        // rule: never call print!/eprintln! from more than one site), so this
        // exemplar's storage target -- here `stdout`, easily any `impl Write`
        // -- stays separable from the formatting at each call site.
        capture(&args, &mut io::stdout().lock());
    }

    fn capture(args: &Args, out: &mut impl Write) {
        std::fs::create_dir_all(&args.out).expect("create output directory");
        let end = args
            .start
            .checked_add(args.seeds)
            .expect("--start + --seeds overflowed u64");

        let generator = Generator::new();
        let mut found = 0usize;
        for seed in args.start..end {
            let schedule = generator.generate(seed);
            let mut handler = BuggyHandler::new();
            let outcome = run(&schedule, &mut handler);
            if let Some(pathology) = outcome.pathology() {
                let recording = Recording::new(seed, schedule, outcome.clone());
                let path = args.out.join(format!("capture-{seed}.json"));
                recording.save(&path).expect("save recording");
                writeln!(out, "seed {seed}: {pathology:?} -> {}", path.display()).expect("write");
                found += 1;
            }
        }
        writeln!(
            out,
            "checked {} seed(s) [{}, {end}), captured {found} pathology(ies) into {}",
            args.seeds,
            args.start,
            args.out.display(),
        )
        .expect("write");
    }

    /// Minimal hand-rolled argument parsing -- deliberately no CLI-argument
    /// dependency, so this exemplar stays as small as its purpose warrants.
    struct Args {
        seeds: u64,
        start: u64,
        out: PathBuf,
    }

    impl Args {
        fn parse() -> Self {
            let mut seeds = 1000u64;
            let mut start = 0u64;
            let mut out = PathBuf::from("captures");
            let mut args = std::env::args().skip(1);
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--seeds" => {
                        seeds = args
                            .next()
                            .expect("--seeds needs a value")
                            .parse()
                            .expect("--seeds must be a number");
                    }
                    "--start" => {
                        start = args
                            .next()
                            .expect("--start needs a value")
                            .parse()
                            .expect("--start must be a number");
                    }
                    "--out" => {
                        out = PathBuf::from(args.next().expect("--out needs a value"));
                    }
                    other => panic!("unrecognized argument: {other}"),
                }
            }
            Self { seeds, start, out }
        }
    }
}
