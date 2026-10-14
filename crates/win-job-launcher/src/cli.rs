// Copyright (c) 2026 Mike Grier
//! The launcher's command line.
//!
//! ```text
//! win-job-launcher --timeout-ms <ms> --stdout <path> --stderr <path> --result <path>
//!                  [--trace] -- <program> [args...]
//! ```
//!
//! Every flag precedes `--`; everything after it is the command, passed through
//! untouched, so a command argument that looks like one of these flags is still
//! the command's.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

#[cfg(test)]
mod tests;

/// The flag spellings. Changing one is a breaking change for
/// `tools/run-sabotage.ps1`, which spells them too.
pub mod flags {
    // Each constant is its own documentation: its value is the spelling.
    #![allow(missing_docs)]
    pub const TIMEOUT_MS: &str = "--timeout-ms";
    pub const STDOUT: &str = "--stdout";
    pub const STDERR: &str = "--stderr";
    pub const RESULT: &str = "--result";
    pub const TRACE: &str = "--trace";
    pub const SEPARATOR: &str = "--";
}

/// The largest bound accepted. One more is `INFINITE` to `WaitForSingleObject`,
/// which would make the bound no bound at all.
pub const MAX_TIMEOUT_MS: u32 = u32::MAX - 1;

/// The usage text printed with any [`UsageError`].
pub const USAGE: &str = "usage: win-job-launcher --timeout-ms <ms> --stdout <path> --stderr <path> \
                         --result <path> [--trace] -- <program> [args...]";

/// One parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// The wall-clock bound, from the moment the command has been created: the
    /// launcher starts waiting when creating it returns.
    pub timeout_ms: u32,
    /// Where the command's stdout goes; created or truncated.
    pub stdout: PathBuf,
    /// Where the command's stderr goes; created or truncated.
    pub stderr: PathBuf,
    /// Where the outcome is written.
    pub result: PathBuf,
    /// Whether to narrate each step to the launcher's stderr.
    pub trace: bool,
    /// The command, resolved as `std::process::Command` resolves a program.
    pub program: OsString,
    /// The command's arguments, passed through untouched.
    pub args: Vec<OsString>,
}

/// Why a command line was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    /// An argument before `--` that is not one of [`flags`].
    UnknownFlag(OsString),
    /// A flag that takes a value was the last argument.
    MissingValue(&'static str),
    /// A flag given twice.
    Repeated(&'static str),
    /// A required flag never given.
    Missing(&'static str),
    /// `--timeout-ms`'s value is not a whole number in `1..=MAX_TIMEOUT_MS`.
    BadTimeout(OsString),
    /// No `--`, or nothing after it.
    NoProgram,
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFlag(arg) => write!(f, "unknown argument {}", arg.to_string_lossy()),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::Repeated(flag) => write!(f, "{flag} given more than once"),
            Self::Missing(flag) => write!(f, "{flag} is required"),
            Self::BadTimeout(value) => write!(
                f,
                "{} must be a whole number of milliseconds from 1 to {MAX_TIMEOUT_MS}, not {}",
                flags::TIMEOUT_MS,
                value.to_string_lossy()
            ),
            Self::NoProgram => write!(f, "no command: expected {} and a program", flags::SEPARATOR),
        }
    }
}

impl std::error::Error for UsageError {}

/// Parses the arguments after the program name.
///
/// # Errors
///
/// Returns the first [`UsageError`] met, reading left to right.
pub fn parse<I>(args: I) -> Result<Invocation, UsageError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args = args.into_iter();
    let mut timeout_ms = None;
    let mut stdout = None;
    let mut stderr = None;
    let mut result = None;
    let mut trace = false;

    loop {
        let Some(arg) = args.next() else {
            return Err(UsageError::NoProgram);
        };
        let Some(text) = arg.to_str() else {
            return Err(UsageError::UnknownFlag(arg));
        };
        match text {
            flags::SEPARATOR => break,
            flags::TRACE => {
                if trace {
                    return Err(UsageError::Repeated(flags::TRACE));
                }
                trace = true;
            }
            flags::TIMEOUT_MS => {
                let value = take_value(&mut args, flags::TIMEOUT_MS, &timeout_ms)?;
                timeout_ms = Some(parse_timeout(value)?);
            }
            flags::STDOUT => {
                stdout = Some(PathBuf::from(take_value(
                    &mut args,
                    flags::STDOUT,
                    &stdout,
                )?));
            }
            flags::STDERR => {
                stderr = Some(PathBuf::from(take_value(
                    &mut args,
                    flags::STDERR,
                    &stderr,
                )?));
            }
            flags::RESULT => {
                result = Some(PathBuf::from(take_value(
                    &mut args,
                    flags::RESULT,
                    &result,
                )?));
            }
            _ => return Err(UsageError::UnknownFlag(arg)),
        }
    }

    let program = args.next().ok_or(UsageError::NoProgram)?;
    Ok(Invocation {
        timeout_ms: timeout_ms.ok_or(UsageError::Missing(flags::TIMEOUT_MS))?,
        stdout: stdout.ok_or(UsageError::Missing(flags::STDOUT))?,
        stderr: stderr.ok_or(UsageError::Missing(flags::STDERR))?,
        result: result.ok_or(UsageError::Missing(flags::RESULT))?,
        trace,
        program,
        args: args.collect(),
    })
}

/// The value after a flag, refusing a second occurrence of the flag.
fn take_value<T>(
    args: &mut impl Iterator<Item = OsString>,
    flag: &'static str,
    already: &Option<T>,
) -> Result<OsString, UsageError> {
    if already.is_some() {
        return Err(UsageError::Repeated(flag));
    }
    args.next().ok_or(UsageError::MissingValue(flag))
}

fn parse_timeout(value: OsString) -> Result<u32, UsageError> {
    match value.to_str().and_then(|text| text.parse::<u32>().ok()) {
        Some(ms) if (1..=MAX_TIMEOUT_MS).contains(&ms) => Ok(ms),
        _ => Err(UsageError::BadTimeout(value)),
    }
}
