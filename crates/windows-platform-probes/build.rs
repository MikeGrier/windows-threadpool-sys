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
//! opt every probe in this crate into long paths, silently changing what all
//! the others measure -- stated without a count on purpose, so it stays true as
//! probes are added.

fn main() {
    // Declared unconditionally, because the cfg's *absence* is as meaningful as
    // its presence and an undeclared name would warn under `unexpected_cfgs`.
    println!("cargo::rustc-check-cfg=cfg(long_path_manifest_embedded)");

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

        // The aware binary reads this rather than claiming the opt-in outright,
        // so its report cannot say `manifest longPathAware : yes` for an
        // executable this script did not manifest.
        //
        // That mattered: with the claim hardcoded, a non-MSVC target skipped the
        // block above and produced two binaries with *no* manifest between them,
        // one of which still announced it had one. Both halves then measured the
        // un-opted-in case and a reader comparing them would conclude the opt-in
        // does not work -- a wrong answer, silently, in the one place this probe
        // cannot fail loudly instead, because the whole finding is the difference
        // between the two. Emitting the flag from the same branch that does the
        // embedding is what keeps the label and the linker in step.
        println!("cargo::rustc-cfg=long_path_manifest_embedded");
    }
}
