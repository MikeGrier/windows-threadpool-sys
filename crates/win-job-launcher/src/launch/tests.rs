// Copyright (c) 2026 Mike Grier

use super::{needs_cleanup, strays_among};

#[test]
fn a_job_known_to_hold_only_the_command_needs_no_cleanup() {
    assert!(!needs_cleanup(Some(0)));
}

#[test]
fn a_job_holding_strays_needs_cleanup() {
    for count in [1, 2, 10, u32::MAX] {
        assert!(needs_cleanup(Some(count)), "{count}");
    }
}

#[test]
fn a_job_whose_count_is_unknown_needs_cleanup() {
    // Not read as zero: with the process list unreadable nothing shows the job
    // holds only the command.
    assert!(needs_cleanup(None));
}

#[test]
fn a_job_holding_only_the_command_has_no_strays() {
    // The case the job's active count got wrong: the command has exited, its
    // handle is signalled, and it is still listed for a moment.
    assert_eq!(strays_among(&[4242], 4242), 0);
}

#[test]
fn an_empty_job_has_no_strays() {
    assert_eq!(strays_among(&[], 4242), 0);
}

#[test]
fn every_process_but_the_command_is_a_stray() {
    assert_eq!(strays_among(&[4242, 7], 4242), 1);
    assert_eq!(strays_among(&[7, 4242], 4242), 1);
    assert_eq!(strays_among(&[7, 4242, 9, 11], 4242), 3);
}

#[test]
fn when_the_command_is_no_longer_listed_everything_left_is_a_stray() {
    assert_eq!(strays_among(&[7], 4242), 1);
    assert_eq!(strays_among(&[7, 9, 11], 4242), 3);
}

#[test]
fn a_similar_id_is_not_the_command() {
    assert_eq!(strays_among(&[4241, 4243, 42420, 424], 4242), 4);
}
