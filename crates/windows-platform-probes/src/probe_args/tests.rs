// Copyright (c) Mike Grier.
//! Tests for the probes' argument parser.

use super::{ArgError, Flag, IterationMultiplier, parse, positive, positive_list, usage};

const FLAGS: [Flag; 2] = [
    Flag {
        name: "repetitions",
        value: "N",
        help: "how many times; 5 when not given",
    },
    Flag {
        name: "producers",
        value: "N,N,...",
        help: "which producer counts; 1,2 when not given",
    },
];

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|arg| (*arg).to_owned()).collect()
}

#[test]
fn no_arguments_give_no_values() {
    let given = parse(args(&[]), &FLAGS).expect("nothing to refuse");
    assert_eq!(given.get("repetitions"), None);
    assert_eq!(given.get("producers"), None);
}

#[test]
fn a_flag_takes_the_argument_after_it_or_the_text_after_an_equals_sign() {
    let given = parse(args(&["--repetitions", "7", "--producers=1,3"]), &FLAGS).expect("valid");
    assert_eq!(given.get("repetitions"), Some("7"));
    assert_eq!(given.get("producers"), Some("1,3"));
}

#[test]
fn the_flags_may_come_in_any_order_and_only_some_may_be_given() {
    let given = parse(args(&["--producers", "4"]), &FLAGS).expect("valid");
    assert_eq!(given.get("producers"), Some("4"));
    assert_eq!(given.get("repetitions"), None);
}

#[test]
fn an_unknown_flag_a_bare_word_and_help_are_refused_as_unknown() {
    for argument in ["--pushes", "repetitions", "--help", "-r", "--"] {
        assert_eq!(
            parse(args(&[argument, "1"]), &FLAGS),
            Err(ArgError::Unknown(argument.to_owned())),
            "{argument}"
        );
    }
}

#[test]
fn a_flag_with_nothing_after_it_is_refused() {
    assert_eq!(
        parse(args(&["--repetitions"]), &FLAGS),
        Err(ArgError::MissingValue("repetitions"))
    );
}

#[test]
fn a_flag_given_twice_is_refused_in_either_spelling() {
    assert_eq!(
        parse(args(&["--repetitions", "3", "--repetitions=5"]), &FLAGS),
        Err(ArgError::Repeated("repetitions"))
    );
}

#[test]
fn an_empty_value_after_an_equals_sign_is_a_value_for_the_probe_to_judge() {
    let given = parse(args(&["--repetitions="]), &FLAGS).expect("parsed");
    assert_eq!(given.get("repetitions"), Some(""));
    assert!(positive("repetitions", "").is_err());
}

#[test]
#[should_panic(expected = "is not a flag this probe declared")]
fn asking_for_a_flag_nothing_declared_is_the_probes_mistake() {
    let given = parse(args(&[]), &FLAGS).expect("valid");
    let _ = given.get("pushes");
}

#[test]
fn a_positive_number_is_a_whole_number_of_at_least_one() {
    assert_eq!(positive("repetitions", "1"), Ok(1));
    assert_eq!(positive("repetitions", "50000"), Ok(50_000));
    for refused in ["0", "-1", "1.5", "x", "", " 3"] {
        let error = positive("repetitions", refused).expect_err(refused);
        assert!(
            matches!(&error, ArgError::Invalid { flag: "repetitions", value, .. } if value == refused),
            "{refused}: {error:?}"
        );
    }
}

#[test]
fn a_list_keeps_its_order_and_tolerates_spaces_after_commas() {
    assert_eq!(positive_list("producers", "1,2,4"), Ok(vec![1, 2, 4]));
    assert_eq!(positive_list("producers", "8, 1"), Ok(vec![8, 1]));
    assert_eq!(positive_list("producers", "3"), Ok(vec![3]));
}

#[test]
fn a_list_that_is_empty_repeats_an_entry_or_holds_a_bad_one_is_refused() {
    for refused in ["", "1,,2", "1,0", "1,2,1", "1,x", "1,"] {
        let error = positive_list("producers", refused).expect_err(refused);
        assert!(
            matches!(&error, ArgError::Invalid { flag: "producers", value, .. } if value == refused),
            "{refused}: {error:?}"
        );
    }
    let repeated = positive_list("producers", "1,2,1").expect_err("repeated");
    assert!(
        repeated.to_string().contains("1 is listed more than once"),
        "{repeated}"
    );
}

#[test]
fn the_usage_names_every_flag_its_value_and_its_help() {
    let text = usage("probe-example", &FLAGS);
    assert!(text.starts_with("usage: probe-example [--repetitions N] [--producers N,N,...]"));
    for flag in FLAGS {
        assert!(
            text.contains(&format!("--{} {}", flag.name, flag.value)),
            "{text}"
        );
        assert!(text.contains(flag.help), "{text}");
    }
}

#[test]
fn each_refusal_says_which_flag_and_why() {
    assert_eq!(
        ArgError::Unknown("--x".into()).to_string(),
        "`--x` is not a setting this probe takes"
    );
    assert_eq!(
        ArgError::MissingValue("repetitions").to_string(),
        "--repetitions needs a value after it"
    );
    assert_eq!(
        ArgError::Repeated("repetitions").to_string(),
        "--repetitions was given more than once"
    );
    assert_eq!(
        positive("repetitions", "0").expect_err("zero").to_string(),
        "--repetitions 0: must be at least 1"
    );
}

// --- The cost probes' one setting ---------------------------------------------------------------

#[test]
fn the_multiplier_is_one_unless_a_command_line_says_otherwise() {
    let multiplier = IterationMultiplier::from_args(args(&[]), 100_000).expect("valid");
    assert_eq!(multiplier, IterationMultiplier::default());
    assert_eq!(multiplier.get(), 1);
    assert!(multiplier.is_default());
    assert_eq!(multiplier.scale(100_000), 100_000);
}

#[test]
fn a_multiplier_scales_every_loop_by_the_same_factor() {
    let multiplier =
        IterationMultiplier::from_args(args(&["--iteration-multiplier", "4"]), 100_000)
            .expect("valid");
    assert_eq!(multiplier.get(), 4);
    assert!(!multiplier.is_default());
    assert_eq!(multiplier.scale(100_000), 400_000);
    assert_eq!(multiplier.scale(20_000), 80_000);
    assert_eq!(multiplier.scale(1), 4);
}

#[test]
fn a_multiplier_is_refused_when_it_takes_the_longest_loop_past_what_a_loop_counts() {
    let largest = 200_000;
    let ceiling = u32::MAX / largest;
    let fits = ceiling.to_string();
    let over = (ceiling + 1).to_string();
    assert_eq!(
        IterationMultiplier::from_args(args(&["--iteration-multiplier", &fits]), largest)
            .expect("the largest that fits")
            .get(),
        ceiling
    );
    let error = IterationMultiplier::from_args(args(&["--iteration-multiplier", &over]), largest)
        .expect_err("one past it");
    assert!(
        matches!(
            &error,
            ArgError::Invalid {
                flag: "iteration-multiplier",
                ..
            }
        ),
        "{error:?}"
    );
    assert!(
        error
            .to_string()
            .contains("200000 iterations at the default"),
        "{error}"
    );

    let beyond_u32 = (u64::from(u32::MAX) + 1).to_string();
    assert!(
        IterationMultiplier::from_args(args(&["--iteration-multiplier", &beyond_u32]), 1).is_err(),
        "a multiplier that is not even a u32 is refused, not truncated"
    );
}

#[test]
fn a_multiplier_of_zero_or_text_is_refused_and_another_probes_setting_is_unknown() {
    for refused in ["0", "two", "-3"] {
        assert!(
            IterationMultiplier::from_args(args(&["--iteration-multiplier", refused]), 10).is_err(),
            "{refused}"
        );
    }
    assert_eq!(
        IterationMultiplier::from_args(args(&["--repetitions", "3"]), 10),
        Err(ArgError::Unknown("--repetitions".to_owned()))
    );
}

#[test]
#[should_panic(expected = "passes u32::MAX")]
fn scaling_a_loop_longer_than_the_one_the_multiplier_was_checked_against_is_the_probes_mistake() {
    let multiplier =
        IterationMultiplier::from_args(args(&["--iteration-multiplier", "3"]), 1).expect("valid");
    let _ = multiplier.scale(u32::MAX);
}

#[test]
fn the_report_line_says_the_multiplier_and_whether_it_is_the_default() {
    let default = IterationMultiplier::default().report_line();
    assert_eq!(default, "iteration multiplier: 1 (the default)");
    let set = IterationMultiplier::from_args(args(&["--iteration-multiplier=8"]), 10)
        .expect("valid")
        .report_line();
    assert!(
        set.starts_with("iteration multiplier: 8 (NOT the default"),
        "{set}"
    );
    // The cost probes' table readers take a line that starts with a word and then
    // a number for a row; this line must not look like one.
    for line in [default, set] {
        let second = line.split_whitespace().nth(1).expect("two words");
        assert!(second.parse::<f64>().is_err(), "{line}");
    }
}
