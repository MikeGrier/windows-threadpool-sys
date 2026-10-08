// Copyright (c) 2026 Mike Grier
//! What happened to the command, as the launcher reports it.
//!
//! The result file holds exactly one line of JSON, an object whose `outcome`
//! is one of three spellings:
//!
//! | `outcome` | Meaning | Further fields |
//! |---|---|---|
//! | `exited` | The command exited by itself before the bound. | `code`, `strays`, `confirmed` |
//! | `timed-out` | The bound was reached and the job was terminated. | `confirmed` |
//! | `not-started` | The command never ran. | `error`, `osError` |
//!
//! `confirmed` is the launcher's statement that cleanup is finished: the job
//! was terminated and then seen to hold no process. On `exited` it is `true`
//! with no stray to clean up as well as after one was cleaned up; `false`
//! means the launcher could not establish that the tree is gone, and the
//! outcome is not one a caller may score as the command's own result.
//!
//! `strays` is the number of processes other than the command left in the job
//! when it exited, or `null` when the job's process list could not be read. It
//! is `null`, not `0`, then: the launcher does not know, and it terminates and
//! confirms whatever is there as though there were some.
//!
//! Every outcome carries `elapsedMs`, and carries the job's accounting
//! (`totalProcesses`, `activeProcesses`, `userCpuMs`, `kernelCpuMs`) whenever a
//! job existed to account for. `code` is the exit code as a **signed** 32-bit
//! integer, the form `%ERRORLEVEL%` and .NET's `Process.ExitCode` use, so an
//! NTSTATUS such as `STATUS_DLL_INIT_FAILED` reads as `-1073741502`.
//!
//! The text is 7-bit ASCII: anything else in a string is written as a `\u`
//! escape. Windows PowerShell 5.1 reads an unmarked file in the ANSI code page,
//! so a raw UTF-8 error message from a localised Windows would arrive mangled.
//!
//! **The spellings in [`keys`] and [`outcomes`] are the contract with
//! `tools/run-sabotage.ps1`; changing any of them is a breaking change for it.**

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::Path;

#[cfg(test)]
mod tests;

/// The `outcome` values.
pub mod outcomes {
    // Each constant is its own documentation: its value is the spelling.
    #![allow(missing_docs)]
    pub const EXITED: &str = "exited";
    pub const TIMED_OUT: &str = "timed-out";
    pub const NOT_STARTED: &str = "not-started";
}

/// The field names.
pub mod keys {
    // Each constant is its own documentation: its value is the spelling.
    #![allow(missing_docs)]
    pub const OUTCOME: &str = "outcome";
    pub const CODE: &str = "code";
    pub const STRAYS: &str = "strays";
    pub const CONFIRMED: &str = "confirmed";
    pub const ERROR: &str = "error";
    pub const OS_ERROR: &str = "osError";
    pub const ELAPSED_MS: &str = "elapsedMs";
    pub const TOTAL_PROCESSES: &str = "totalProcesses";
    pub const ACTIVE_PROCESSES: &str = "activeProcesses";
    pub const USER_CPU_MS: &str = "userCpuMs";
    pub const KERNEL_CPU_MS: &str = "kernelCpuMs";
}

/// How the command ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Exited by itself. `strays` counts the processes other than the command
    /// still in the job at that moment, which the launcher then terminated.
    Exited {
        /// The exit code, signed.
        code: i32,
        /// Processes other than the command still in the job when it exited,
        /// or `None` when the job's process list could not be read, so the
        /// count is not known -- which is not the same as none.
        strays: Option<u32>,
        /// Whether the job was seen empty afterwards, having been terminated
        /// if it was not already.
        confirmed: bool,
    },
    /// Killed at the bound. `confirmed` is whether the job was terminated and
    /// then seen empty within the confirmation bound.
    TimedOut {
        /// Whether the termination succeeded and the job emptied within the
        /// confirmation bound.
        confirmed: bool,
    },
    /// Never ran, with the reason and the Windows error code when there is one.
    NotStarted {
        /// What failed, and the error's own text.
        error: String,
        /// The Windows error code, when the failure carried one.
        os_error: Option<i32>,
    },
}

/// The job's own accounting, read from `JOBOBJECT_BASIC_ACCOUNTING_INFORMATION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accounting {
    /// Every process ever in the job, including those that have exited.
    pub total_processes: u32,
    /// Processes in the job now.
    pub active_processes: u32,
    /// User-mode CPU time used by every process ever in the job.
    pub user_cpu_ms: u64,
    /// Kernel-mode CPU time used by every process ever in the job.
    pub kernel_cpu_ms: u64,
}

/// Everything the result file records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// How the command ended.
    pub outcome: Outcome,
    /// Time from the launcher's start to the outcome.
    pub elapsed_ms: u64,
    /// Absent when no job could be created to account for.
    pub accounting: Option<Accounting>,
}

impl Report {
    /// The result file's one line, without a line terminator.
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut json = String::from("{");
        let mut field = |key: &str, value: &str| {
            if json.len() > 1 {
                json.push(',');
            }
            push_string(&mut json, key);
            json.push(':');
            json.push_str(value);
        };

        match &self.outcome {
            Outcome::Exited {
                code,
                strays,
                confirmed,
            } => {
                field(keys::OUTCOME, &quoted(outcomes::EXITED));
                field(keys::CODE, &code.to_string());
                field(
                    keys::STRAYS,
                    &strays.map_or_else(|| String::from("null"), |count| count.to_string()),
                );
                field(keys::CONFIRMED, if *confirmed { "true" } else { "false" });
            }
            Outcome::TimedOut { confirmed } => {
                field(keys::OUTCOME, &quoted(outcomes::TIMED_OUT));
                field(keys::CONFIRMED, if *confirmed { "true" } else { "false" });
            }
            Outcome::NotStarted { error, os_error } => {
                field(keys::OUTCOME, &quoted(outcomes::NOT_STARTED));
                field(keys::ERROR, &quoted(error));
                field(
                    keys::OS_ERROR,
                    &os_error.map_or_else(|| String::from("null"), |code| code.to_string()),
                );
            }
        }
        field(keys::ELAPSED_MS, &self.elapsed_ms.to_string());
        if let Some(a) = self.accounting {
            field(keys::TOTAL_PROCESSES, &a.total_processes.to_string());
            field(keys::ACTIVE_PROCESSES, &a.active_processes.to_string());
            field(keys::USER_CPU_MS, &a.user_cpu_ms.to_string());
            field(keys::KERNEL_CPU_MS, &a.kernel_cpu_ms.to_string());
        }
        json.push('}');
        json
    }
}

/// Writes `report` to `path` whole: to a sibling temporary file first, then
/// renamed over `path`, so a reader sees the complete result or none.
///
/// # Errors
///
/// Any error writing the temporary file or renaming it. The temporary file is
/// removed on a failed rename.
pub fn write_result(path: &Path, report: &Report) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".partial");
    let temporary = Path::new(&temporary);
    fs::write(temporary, report.to_json() + "\n")?;
    fs::rename(temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(temporary);
    })
}

fn quoted(text: &str) -> String {
    let mut out = String::new();
    push_string(&mut out, text);
    out
}

/// Appends `text` as a JSON string in 7-bit ASCII.
fn push_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            ' '..='~' => out.push(c),
            _ => {
                let mut units = [0_u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    let _ = write!(out, "\\u{unit:04x}");
                }
            }
        }
    }
    out.push('"');
}
