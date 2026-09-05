// Copyright (c) 2026 Mike Grier
//! Tests for [`MetadataPolicy`](super::MetadataPolicy).

use super::MetadataPolicy;

#[test]
fn the_default_withholds_everything() {
    // The point of the whole design: a caller who does not think about this
    // collects the least, rather than the most.
    let policy = MetadataPolicy::default();

    assert!(!policy.includes_timestamp());
    assert!(!policy.includes_cpu_model());
    assert!(!policy.includes_os_build());
    assert!(!policy.includes_virtualisation());
    assert!(!policy.includes_anything());
}

#[test]
fn the_default_is_the_redacted_policy() {
    // Two spellings of one policy, and a test rather than a comment, because a
    // `Default` that drifted from `redacted` would silently change what the
    // tool collects without any call site changing.
    assert_eq!(MetadataPolicy::default(), MetadataPolicy::redacted());
}

#[test]
fn including_covers_every_field() {
    // Every field this crate can withhold must be reachable by the one opt-in,
    // or a runner who asked to help would be silently sending less than they
    // agreed to.
    let policy = MetadataPolicy::included();

    assert!(policy.includes_timestamp());
    assert!(policy.includes_cpu_model());
    assert!(policy.includes_os_build());
    assert!(policy.includes_virtualisation());
    assert!(policy.includes_anything());
}

#[test]
fn withholding_the_model_subtracts_only_the_model() {
    // The subtraction is a scalpel, not a switch back to the default: a runner
    // who opted in and then withheld the name is still sending the rest.
    let policy = MetadataPolicy::included().without_cpu_model();

    assert!(!policy.includes_cpu_model());
    assert!(policy.includes_timestamp());
    assert!(policy.includes_os_build());
    assert!(policy.includes_virtualisation());
    assert!(policy.includes_anything());
}

#[test]
fn withholding_the_model_from_a_redacted_policy_changes_nothing() {
    // `--no-cpu-model` without `--include-metadata` is redundant rather than
    // wrong, and must stay harmless: a cautious runner passing both should not
    // get a different record from one passing neither.
    assert_eq!(
        MetadataPolicy::redacted().without_cpu_model(),
        MetadataPolicy::redacted()
    );
}

#[test]
fn a_policy_with_one_field_still_counts_as_including_something() {
    // `includes_anything` decides which advice the notice prints, so it must
    // not be a synonym for `included`.
    let policy = MetadataPolicy::included().without_cpu_model();

    assert_ne!(policy, MetadataPolicy::included());
    assert!(policy.includes_anything());
}
