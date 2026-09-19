// Copyright (c) Mike Grier.
use std::fmt::Write;
use std::path::Path;
use std::process::Command;

const HASH_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const HASH_PRIME: u64 = 0x0000_0100_0000_01b3;

fn hash(bytes: &[u8], state: &mut u64) {
    for &byte in bytes {
        *state = (*state ^ u64::from(byte)).wrapping_mul(HASH_PRIME);
    }
}

fn fingerprint(path: &Path, root: &Path, state: &mut u64, directives: &mut String) {
    if path.is_dir() {
        let mut children: Vec<_> = std::fs::read_dir(path)
            .expect("read source directory")
            .map(|entry| entry.expect("source entry").path())
            .collect();
        children.sort();
        for child in children {
            fingerprint(&child, root, state, directives);
        }
    } else {
        writeln!(directives, "cargo:rerun-if-changed={}", path.display()).unwrap();
        hash(
            path.strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .as_bytes(),
            state,
        );
        hash(&std::fs::read(path).expect("read fingerprint input"), state);
    }
}

fn git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-pager")
        .args(args)
        .current_dir(root)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let root = Path::new(&manifest);
    let workspace = root.ancestors().nth(4).unwrap();
    let mut directives = String::new();
    let mut source = HASH_OFFSET;
    for relative in ["src", "build.rs", "Cargo.toml"] {
        fingerprint(&root.join(relative), root, &mut source, &mut directives);
    }
    fingerprint(
        &workspace.join("Cargo.lock"),
        workspace,
        &mut source,
        &mut directives,
    );
    fingerprint(
        &workspace.join("Cargo.toml"),
        workspace,
        &mut source,
        &mut directives,
    );
    let revision = git(workspace, &["rev-parse", "HEAD"])
        .map(|bytes| String::from_utf8(bytes).expect("git revision is UTF-8"))
        .unwrap_or_else(|| "unavailable".to_owned());
    let diff = git(workspace, &["diff", "HEAD", "--"]);
    let diff_fingerprint = diff
        .map(|bytes| {
            let mut state = HASH_OFFSET;
            hash(&bytes, &mut state);
            format!("{state:016x}")
        })
        .unwrap_or_else(|| "unavailable".to_owned());
    let status = git(workspace, &["status", "--porcelain"]);
    let worktree = match status {
        Some(bytes) if bytes.is_empty() => "clean",
        Some(_) => "dirty",
        None => "unavailable",
    };
    let rustc = Command::new(std::env::var_os("RUSTC").expect("RUSTC"))
        .arg("-vV")
        .output()
        .expect("rustc version");
    assert!(rustc.status.success(), "rustc version failed");
    let compiler = String::from_utf8(rustc.stdout).expect("rustc version is UTF-8");
    for (key, value) in [
        ("RC_GIT_REVISION", revision.trim().to_owned()),
        ("RC_WORKTREE", worktree.to_owned()),
        ("RC_TRACKED_DIFF_FNV64", diff_fingerprint),
        ("RC_HARNESS_AND_MANIFESTS_FNV64", format!("{source:016x}")),
        ("RC_COMPILER", compiler.trim().replace('\n', " | ")),
        ("RC_TARGET", std::env::var("TARGET").unwrap()),
        ("RC_OPT_LEVEL", std::env::var("OPT_LEVEL").unwrap()),
        (
            "RC_RUSTFLAGS",
            std::env::var("CARGO_ENCODED_RUSTFLAGS")
                .unwrap_or_default()
                .replace('\x1f', " "),
        ),
    ] {
        writeln!(directives, "cargo:rustc-env={key}={value}").unwrap();
    }
    // Capture the checkout state on each invocation, including changes outside this crate.
    writeln!(
        directives,
        "cargo:rerun-if-changed={}",
        root.join(".provenance-always-rebuild").display()
    )
    .unwrap();
    print!("{directives}");
}
