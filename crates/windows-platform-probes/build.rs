// Copyright (c) Mike Grier.

//! Embeds `longPathAware` into **one** binary, so the long-path opt-in can be
//! measured rather than read about.
//!
//! The opt-in has two halves and neither is a runtime switch: a machine-wide
//! registry value, and a per-executable manifest. The manifest half is what
//! this adds, and it is added to `probe-long-path-aware` **alone** --
//! `probe-long-path-unaware` is the same code without it, because a comparison
//! needs both sides and the un-opted-in case is what most consumers of this
//! workspace actually have.
//!
//! `rustc-link-arg-bin` rather than `rustc-link-arg-bins`: the latter would
//! opt every probe in this crate into long paths, silently changing what the
//! other thirteen measure.

fn main() {
    // Only the MSVC linker understands these, and this crate is Windows-only
    // anyway; guarding keeps a cross-compile from failing on a flag its linker
    // has never heard of.
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        let manifest =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("long-path-aware.manifest");
        println!("cargo::rerun-if-changed=long-path-aware.manifest");
        println!("cargo::rustc-link-arg-bin=probe-long-path-aware=/MANIFEST:EMBED");
        println!(
            "cargo::rustc-link-arg-bin=probe-long-path-aware=/MANIFESTINPUT:{}",
            manifest.display()
        );
    }
}
