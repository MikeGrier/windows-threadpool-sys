// Copyright (c) 2026 Mike Grier
//! Running one command in a job under a bound: the launcher's whole behaviour.

use std::fs::File;
use std::io::{self, Write};
use std::os::windows::io::AsRawHandle;
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

use crate::cli::Invocation;
use crate::job::Job;
use crate::narrate::Narrator;
use crate::outcome::{Accounting, Outcome, Report};
use crate::process::{self, Process};

/// How long, after terminating the job, the launcher waits to see it empty
/// before reporting. Termination is asynchronous; reporting before the tree is
/// gone would let a caller go on to touch files the tree still holds open.
pub const CONFIRM_BOUND: Duration = Duration::from_secs(10);

/// The exit code every process in the job is given when the launcher kills it.
/// `WAIT_TIMEOUT`, because that is why it was killed.
pub const KILLED_EXIT_CODE: u32 = WAIT_TIMEOUT;

/// How often the job is re-read while waiting for it to empty.
const CONFIRM_POLL: Duration = Duration::from_millis(10);

/// Runs `invocation`'s command to an [`Outcome`], narrating through `n`.
///
/// Never returns before every process in the job has been terminated or the
/// confirmation bound has passed, whichever is first.
pub fn run<W: Write>(invocation: &Invocation, n: &mut Narrator<W>) -> Report {
    let job = match Job::new_kill_on_close() {
        Ok(job) => job,
        Err(e) => return not_started(n, None, "could not create a job object", &e),
    };
    n.trace(format_args!("created a kill-on-close job"));

    let child = match spawn_in_job(invocation, &job, n) {
        Ok(child) => child,
        Err((context, e)) => return not_started(n, Some(&job), context, &e),
    };

    let bound = invocation.timeout_ms;
    // SAFETY: the child's handle is live for as long as `child` is.
    let waited = unsafe { WaitForSingleObject(child.as_raw_handle(), bound) };
    match waited {
        WAIT_OBJECT_0 => exited(&child, &job, n),
        WAIT_TIMEOUT => timed_out(&child, &job, bound, n),
        // Unreachable in practice: the handle is one this process owns and
        // holds open, with SYNCHRONIZE access. Reported rather than ignored,
        // because guessing an outcome would be a wrong answer.
        WAIT_FAILED => panic!(
            "WaitForSingleObject failed on the command's own handle: {}",
            io::Error::last_os_error()
        ),
        other => panic!("WaitForSingleObject returned {other}, which a process handle cannot"),
    }
}

/// Creates the command as a member of the job, so that nothing it starts, and
/// not the command itself, can ever exist outside it.
fn spawn_in_job<W: Write>(
    invocation: &Invocation,
    job: &Job,
    n: &mut Narrator<W>,
) -> Result<Process, (&'static str, io::Error)> {
    let stdout = File::create(&invocation.stdout).map_err(|e| ("could not create --stdout", e))?;
    let stderr = File::create(&invocation.stderr).map_err(|e| ("could not create --stderr", e))?;

    let child = process::spawn_in_job(job, &invocation.program, &invocation.args, &stdout, &stderr)
        .map_err(|e| ("could not start the command", e))?;
    n.trace(format_args!("created PID {} in the job", child.id()));
    Ok(child)
}

fn exited<W: Write>(child: &Process, job: &Job, n: &mut Narrator<W>) -> Report {
    let code = match child.exit_code() {
        Ok(code) => code,
        // Unreachable in practice: the process has already been seen to exit.
        Err(e) => panic!("the command exited but its status could not be read: {e}"),
    };
    let at_exit = read_accounting(job, n);
    let strays = at_exit.map_or(0, |a| a.active_processes);
    n.trace(format_args!(
        "the command exited with code {code} after {}ms; {}",
        n.elapsed().as_millis(),
        describe(at_exit)
    ));
    // With the accounting unreadable there is no evidence the job is empty, so
    // it is cleaned up and confirmed as if it were not.
    let confirmed = if strays > 0 || at_exit.is_none() {
        n.trace(format_args!(
            "terminating {strays} stray process(es) still in the job"
        ));
        let terminated = terminate(job, n);
        let emptied = confirm_empty(job, None, n);
        terminated && emptied
    } else {
        true
    };
    report(
        n,
        Outcome::Exited {
            code,
            strays,
            confirmed,
        },
        at_exit,
    )
}

fn timed_out<W: Write>(child: &Process, job: &Job, bound: u32, n: &mut Narrator<W>) -> Report {
    // Read before the kill: CPU time against the bound is what tells a spin
    // from a parked wait.
    let at_bound = read_accounting(job, n);
    n.trace(format_args!(
        "the bound of {bound}ms was reached; {}",
        describe(at_bound)
    ));
    let terminated = terminate(job, n);
    let emptied = confirm_empty(job, Some(child), n);
    report(
        n,
        Outcome::TimedOut {
            confirmed: terminated && emptied,
        },
        at_bound,
    )
}

/// Terminates the job, and says whether the call succeeded.
fn terminate<W: Write>(job: &Job, n: &mut Narrator<W>) -> bool {
    n.trace(format_args!("calling TerminateJobObject"));
    match job.terminate(KILLED_EXIT_CODE) {
        Ok(()) => {
            n.trace(format_args!("TerminateJobObject returned"));
            true
        }
        // The caller still waits for the job to empty, but a failed call is
        // never reported as a confirmed cleanup: the job is kill-on-close, so
        // the launcher's own exit would take the tree down, and the result is
        // written before that.
        Err(e) => {
            n.error(format_args!("TerminateJobObject failed: {e}"));
            false
        }
    }
}

/// Waits up to [`CONFIRM_BOUND`] for `child` to be signalled and the job to
/// hold no process, and says whether both happened.
fn confirm_empty<W: Write>(job: &Job, child: Option<&Process>, n: &mut Narrator<W>) -> bool {
    let deadline = Instant::now() + CONFIRM_BOUND;
    if let Some(child) = child {
        let ms = u32::try_from(CONFIRM_BOUND.as_millis()).expect("the bound fits a u32");
        // SAFETY: the handle is live for as long as `child` is.
        let signalled = unsafe { WaitForSingleObject(child.as_raw_handle(), ms) } == WAIT_OBJECT_0;
        n.trace(format_args!(
            "the command's handle {} signalled",
            if signalled { "was" } else { "was NOT" }
        ));
        if !signalled {
            return false;
        }
    }
    loop {
        match job.accounting() {
            Ok(a) if a.active_processes == 0 => {
                n.trace(format_args!("the job is empty"));
                return true;
            }
            Ok(a) if Instant::now() >= deadline => {
                n.error(format_args!(
                    "{} process(es) still in the job after {}ms",
                    a.active_processes,
                    CONFIRM_BOUND.as_millis()
                ));
                return false;
            }
            Ok(_) => thread::sleep(CONFIRM_POLL),
            Err(e) => {
                n.error(format_args!("could not read the job's accounting: {e}"));
                return false;
            }
        }
    }
}

fn read_accounting<W: Write>(job: &Job, n: &mut Narrator<W>) -> Option<Accounting> {
    match job.accounting() {
        Ok(a) => Some(a),
        Err(e) => {
            n.error(format_args!("could not read the job's accounting: {e}"));
            None
        }
    }
}

fn describe(accounting: Option<Accounting>) -> String {
    accounting.map_or_else(
        || String::from("job accounting unavailable"),
        |a| {
            format!(
                "job: {} active of {} process(es), {}ms user CPU, {}ms kernel CPU",
                a.active_processes, a.total_processes, a.user_cpu_ms, a.kernel_cpu_ms
            )
        },
    )
}

fn not_started<W: Write>(
    n: &mut Narrator<W>,
    job: Option<&Job>,
    context: &str,
    e: &io::Error,
) -> Report {
    n.trace(format_args!("{context}: {e}"));
    let accounting = job.and_then(|job| job.accounting().ok());
    report(
        n,
        Outcome::NotStarted {
            error: format!("{context}: {e}"),
            os_error: e.raw_os_error(),
        },
        accounting,
    )
}

fn report<W: Write>(n: &Narrator<W>, outcome: Outcome, accounting: Option<Accounting>) -> Report {
    Report {
        outcome,
        elapsed_ms: u64::try_from(n.elapsed().as_millis()).unwrap_or(u64::MAX),
        accounting,
    }
}
