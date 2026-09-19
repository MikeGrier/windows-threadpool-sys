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

fn fingerprint(path: &Path, state: &mut u64, directives: &mut String) {
    if path.is_dir() {
        let mut children: Vec<_> = std::fs::read_dir(path)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        children.sort();
        for child in children {
            fingerprint(&child, state, directives);
        }
    } else {
        writeln!(directives, "cargo:rerun-if-changed={}", path.display()).unwrap();
        hash(
            path.file_name().unwrap().to_str().unwrap().as_bytes(),
            state,
        );
        hash(&std::fs::read(path).unwrap(), state);
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
        fingerprint(&root.join(relative), &mut source, &mut directives);
    }
    for relative in ["Cargo.toml", "Cargo.lock"] {
        fingerprint(&workspace.join(relative), &mut source, &mut directives);
    }
    let revision = git(workspace, &["rev-parse", "HEAD"])
        .map(|bytes| String::from_utf8(bytes).unwrap())
        .unwrap_or_else(|| "unavailable".into());
    let state = match git(workspace, &["status", "--porcelain"]) {
        Some(bytes) if bytes.is_empty() => "clean",
        Some(_) => "dirty",
        None => "unavailable",
    };
    let compiler = Command::new(std::env::var_os("RUSTC").unwrap())
        .arg("-vV")
        .output()
        .unwrap();
    assert!(compiler.status.success());
    for (key, value) in [
        ("RR_REVISION", revision.trim().to_owned()),
        ("RR_WORKTREE", state.into()),
        ("RR_SOURCE_FNV64", format!("{source:016x}")),
        (
            "RR_COMPILER",
            String::from_utf8(compiler.stdout)
                .unwrap()
                .trim()
                .replace('\n', " | "),
        ),
        ("RR_TARGET", std::env::var("TARGET").unwrap()),
        ("RR_OPT_LEVEL", std::env::var("OPT_LEVEL").unwrap()),
        (
            "RR_RUSTFLAGS",
            std::env::var("CARGO_ENCODED_RUSTFLAGS")
                .unwrap_or_default()
                .replace('\x1f', " "),
        ),
    ] {
        writeln!(directives, "cargo:rustc-env={key}={value}").unwrap();
    }
    writeln!(
        directives,
        "cargo:rerun-if-changed={}",
        root.join(".provenance-always-rebuild").display()
    )
    .unwrap();
    print!("{directives}");
}
