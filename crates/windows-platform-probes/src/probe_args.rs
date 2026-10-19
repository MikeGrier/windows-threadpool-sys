// Copyright (c) Mike Grier.

//! The settings a measurement probe takes on its command line, and the one parser for them
//! (`M4.2`).
//!
//! **An experiment, not a component.** These probes measure platform behaviour and are not for
//! production use. Do not call them from production code, and do not lift a technique out of
//! here. See this crate's DESIGN-NOTES.md.
//!
//! A probe declares its settings as [`Flag`]s and reads what was given through [`parse`]. Every
//! flag takes one value, written `--name value` or `--name=value`, and may appear once. An
//! argument that is not one of the probe's flags, a flag given twice, a flag with no value and a
//! value the probe cannot use are all refused, and the binary hands the refusal and the
//! [`usage`] text to [`refuse_arguments`](crate::report::refuse_arguments), which prints both and
//! exits with status 2. A run that measured something other than what was asked would be read
//! as a record of what was asked.
//!
//! **The defaults belong to the settings types**, `queue_contention::Sampling` for the queue
//! probe and [`IterationMultiplier`] below for the two cost probes, and a run given no arguments
//! measures exactly what it measured before the settings existed, so a published figure stays
//! reproducible.
//!
//! Only the axes a stated diagnostic step varies are reachable: the length of a timed span, and,
//! for the probe that repeats, the repetitions and the producer counts. The cost probes neither
//! repeat nor vary concurrency, so their one setting is the span.

use std::fmt;

#[cfg(test)]
mod tests;

/// One setting a probe accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    /// The name, without the leading `--`.
    pub name: &'static str,
    /// What the value looks like, for the usage text: `N`, or `N,N,...`.
    pub value: &'static str,
    /// What the setting does and what it is when not given, for the usage text.
    pub help: &'static str,
}

/// Why a probe's arguments were refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    /// An argument that is not one of the probe's flags.
    Unknown(String),
    /// A flag with no value after it.
    MissingValue(&'static str),
    /// A flag given more than once.
    Repeated(&'static str),
    /// A value the probe cannot use.
    Invalid {
        /// The flag the value was given for.
        flag: &'static str,
        /// The value as it was given.
        value: String,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for ArgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(argument) => write!(f, "`{argument}` is not a setting this probe takes"),
            Self::MissingValue(flag) => write!(f, "--{flag} needs a value after it"),
            Self::Repeated(flag) => write!(f, "--{flag} was given more than once"),
            Self::Invalid {
                flag,
                value,
                reason,
            } => write!(f, "--{flag} {value}: {reason}"),
        }
    }
}

impl std::error::Error for ArgError {}

/// What a command line gave, for the flags it was parsed against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Given {
    flags: Vec<Flag>,
    values: Vec<Option<String>>,
}

impl Given {
    /// The value given for the flag named `name`, or `None` when it was not given.
    ///
    /// # Panics
    ///
    /// Panics if `name` is not one of the flags the arguments were parsed against, which is a
    /// mistake in the probe and not in the command line: a lookup under a name nothing declared
    /// would otherwise read as a setting that was never given.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        let index = self
            .flags
            .iter()
            .position(|flag| flag.name == name)
            .unwrap_or_else(|| panic!("`{name}` is not a flag this probe declared"));
        self.values[index].as_deref()
    }
}

/// Read `args`, the command line without the program's name, against `flags`.
///
/// # Errors
///
/// [`ArgError::Unknown`] for an argument that is not `--` and one of the flags,
/// [`ArgError::MissingValue`] for a flag at the end with nothing after it, and
/// [`ArgError::Repeated`] for a flag given twice. A value is taken as given; whether the probe
/// can use it is the probe's to say.
pub fn parse(args: impl IntoIterator<Item = String>, flags: &[Flag]) -> Result<Given, ArgError> {
    let mut values: Vec<Option<String>> = vec![None; flags.len()];
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        let Some(named) = argument.strip_prefix("--") else {
            return Err(ArgError::Unknown(argument));
        };
        let (name, inline) = match named.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (named, None),
        };
        let Some(index) = flags.iter().position(|flag| flag.name == name) else {
            return Err(ArgError::Unknown(argument));
        };
        let flag = flags[index].name;
        let value = match inline {
            Some(value) => value,
            None => args.next().ok_or(ArgError::MissingValue(flag))?,
        };
        if values[index].is_some() {
            return Err(ArgError::Repeated(flag));
        }
        values[index] = Some(value);
    }
    Ok(Given {
        flags: flags.to_vec(),
        values,
    })
}

/// How to call `probe`, one line per flag.
#[must_use]
pub fn usage(probe: &str, flags: &[Flag]) -> String {
    let mut text = format!("usage: {probe}");
    for flag in flags {
        text.push_str(&format!(" [--{} {}]", flag.name, flag.value));
    }
    for flag in flags {
        text.push_str(&format!(
            "\n  --{} {}\n      {}",
            flag.name, flag.value, flag.help
        ));
    }
    text
}

/// How many times its usual count each timed loop of a cost probe runs: the length of the span
/// (`M4.2`).
///
/// One multiplier for every loop, rather than a count per loop, so the loops keep the proportions
/// they were given -- those that make a kernel call run fewer iterations than those that do not --
/// and lengthening the span is one decision. The default is 1, the run the published figures came
/// from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IterationMultiplier(u32);

impl Default for IterationMultiplier {
    fn default() -> Self {
        Self(1)
    }
}

impl IterationMultiplier {
    /// The one setting the cost probes take.
    pub const FLAGS: [Flag; 1] = [Flag {
        name: "iteration-multiplier",
        value: "N",
        help: "each timed loop runs N times its usual count, lengthening the span; 1 when not given",
    }];

    /// The multiplier a command line asks for, given that the probe's longest loop runs
    /// `largest` iterations at the default.
    ///
    /// # Errors
    ///
    /// What [`parse`] refuses, a value that is not a whole number of at least one, and a
    /// multiplier that would take the longest loop past `u32::MAX` iterations, which is what a
    /// loop counts in.
    pub fn from_args(
        args: impl IntoIterator<Item = String>,
        largest: u32,
    ) -> Result<Self, ArgError> {
        let given = parse(args, &Self::FLAGS)?;
        let Some(value) = given.get("iteration-multiplier") else {
            return Ok(Self::default());
        };
        let multiplier = u32::try_from(positive("iteration-multiplier", value)?).ok();
        match multiplier {
            Some(multiplier) if largest.checked_mul(multiplier).is_some() => Ok(Self(multiplier)),
            _ => Err(ArgError::Invalid {
                flag: "iteration-multiplier",
                value: value.to_owned(),
                reason: format!(
                    "takes the longest loop, {largest} iterations at the default, past {} iterations",
                    u32::MAX
                ),
            }),
        }
    }

    /// The multiplier.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }

    /// `iterations` times the multiplier.
    ///
    /// # Panics
    ///
    /// Panics if the product passes `u32::MAX`, which [`from_args`](Self::from_args) refuses for
    /// any loop no longer than the `largest` it was given: a loop longer than that is the probe's
    /// mistake.
    #[must_use]
    pub fn scale(self, iterations: u32) -> u32 {
        iterations.checked_mul(self.0).unwrap_or_else(|| {
            panic!(
                "{iterations} iterations times {} passes u32::MAX; the probe gave \
                 `from_args` a smaller longest loop than it runs",
                self.0
            )
        })
    }

    /// Whether this is the multiplier a run given no settings uses.
    #[must_use]
    pub fn is_default(self) -> bool {
        self == Self::default()
    }

    /// The line a cost probe's report carries beside its banner, saying what the run used.
    ///
    /// Its second word is not a number, so a reader of the probe's table that takes a line
    /// starting with a word and then a number as a row does not take this for one.
    #[must_use]
    pub fn report_line(self) -> String {
        if self.is_default() {
            format!("iteration multiplier: {} (the default)", self.0)
        } else {
            format!(
                "iteration multiplier: {} (NOT the default: set on the command line, so these \
                 figures are not the published ones)",
                self.0
            )
        }
    }
}

/// `value` as a whole number of at least one.
///
/// # Errors
///
/// [`ArgError::Invalid`] when `value` is not a whole number, or is zero.
pub fn positive(flag: &'static str, value: &str) -> Result<usize, ArgError> {
    let invalid = |reason: &str| ArgError::Invalid {
        flag,
        value: value.to_owned(),
        reason: reason.to_owned(),
    };
    match value.parse::<usize>() {
        Ok(0) => Err(invalid("must be at least 1")),
        Ok(number) => Ok(number),
        Err(_) => Err(invalid("is not a whole number")),
    }
}

/// `value` as a comma-separated list of whole numbers of at least one, none repeated, in the
/// order given.
///
/// # Errors
///
/// [`ArgError::Invalid`] when the list is empty, an entry is not a whole number of at least one,
/// or an entry is repeated: a repeated entry would measure the same configuration twice under
/// one name.
pub fn positive_list(flag: &'static str, value: &str) -> Result<Vec<usize>, ArgError> {
    let mut list: Vec<usize> = Vec::new();
    for entry in value.split(',') {
        let number = positive(flag, entry.trim()).map_err(|_| ArgError::Invalid {
            flag,
            value: value.to_owned(),
            reason: format!("`{entry}` is not a whole number of at least 1"),
        })?;
        if list.contains(&number) {
            return Err(ArgError::Invalid {
                flag,
                value: value.to_owned(),
                reason: format!("{number} is listed more than once"),
            });
        }
        list.push(number);
    }
    Ok(list)
}
