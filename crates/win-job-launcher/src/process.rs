// Copyright (c) 2026 Mike Grier
//! Creating the command already inside the job.
//!
//! The job is named in `PROC_THREAD_ATTRIBUTE_JOB_LIST`, so the kernel makes
//! the new process a member as part of creating it. There is no moment at which
//! the process exists outside the job -- which is what creating it suspended
//! and then calling `AssignProcessToJobObject` could not give: a launcher
//! killed between those two steps left a suspended command that no job held.
//!
//! `std::process::Command` cannot do this on the pinned toolchain: its
//! `spawn_with_attributes` and `ProcThreadAttributeList` are nightly-only. So
//! this module calls `CreateProcessW` itself, and owns the three things
//! `Command` would otherwise have done:
//!
//! - **Finding the program.** `SearchPathW`, with `.exe` appended when the name
//!   has no extension: the launcher's directory, the system directories, then
//!   `PATH`, as `CreateProcess` itself searches.
//! - **Quoting the arguments.** The rules the Microsoft C runtime reads back
//!   with, which every Rust and C program started this way parses.
//! - **Running `.cmd` and `.bat` files**, which `CreateProcess` cannot run
//!   directly: through `cmd.exe /c`. `cmd` re-parses its command line, so an
//!   argument it would treat as syntax is refused rather than escaped. The
//!   refused set is `"`, `%`, a carriage return and a line feed; every other
//!   argument is quoted whole, which `cmd` leaves inert.
//!
//! Only the command's own three standard handles are inherited, named by
//! `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`, so nothing else the launcher holds open
//! reaches the command or its descendants.

use std::ffi::{OsStr, OsString, c_void};
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::Path;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE, TRUE,
};
use windows_sys::Win32::Storage::FileSystem::SearchPathW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW, UpdateProcThreadAttribute,
};

use crate::job::Job;

#[cfg(test)]
mod tests;

/// The characters of the wide strings below, named so the quoting reads as the
/// rules it implements.
const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const QUOTE: u16 = b'"' as u16;
const BACKSLASH: u16 = b'\\' as u16;
const PERCENT: u16 = b'%' as u16;
const CR: u16 = b'\r' as u16;
const LF: u16 = b'\n' as u16;

/// The extension `SearchPathW` appends to a name that has none.
const DEFAULT_EXTENSION: &str = ".exe";

/// The initial size, in UTF-16 units, of the `SearchPathW` result buffer; it is
/// grown to fit when the path is longer.
const SEARCH_BUFFER: usize = 1024;

/// A process the launcher created. The handle is closed on drop.
#[derive(Debug)]
pub struct Process {
    handle: OwnedHandle,
    id: u32,
}

impl Process {
    /// The process id.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.id
    }

    /// The exit code, signed as `%ERRORLEVEL%` reads it. Meaningful only once
    /// the process has been seen to exit.
    ///
    /// # Errors
    ///
    /// The OS error from `GetExitCodeProcess`.
    pub fn exit_code(&self) -> io::Result<i32> {
        let mut code = 0;
        // SAFETY: the handle is live and `code` is writable.
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &raw mut code) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(code.cast_signed())
    }
}

impl AsRawHandle for Process {
    fn as_raw_handle(&self) -> RawHandle {
        self.handle.as_raw_handle()
    }
}

/// Starts `program` with `args` as a member of `job` from its first instruction,
/// with a null stdin and `stdout`/`stderr` as its output streams. The caller's
/// handles are not modified: the command is given inheritable duplicates.
///
/// # Errors
///
/// A failure to find the program, an argument the command line cannot carry
/// (see the module documentation), or the OS error from preparing or making
/// the call.
pub fn spawn_in_job(
    job: &Job,
    program: &OsStr,
    args: &[OsString],
    stdout: &File,
    stderr: &File,
) -> io::Result<Process> {
    let mut plan = plan(program, args)?;

    // The command gets inheritable DUPLICATES, closed when this call returns.
    // Marking the caller's own handles inheritable would leave them so for good:
    // a later or concurrent process creation, by anything, would inherit them.
    let stdin = File::open("NUL")?;
    let owned = [
        duplicate_inheritable(stdin.as_raw_handle())?,
        duplicate_inheritable(stdout.as_raw_handle())?,
        duplicate_inheritable(stderr.as_raw_handle())?,
    ];
    let stdio: [HANDLE; 3] = owned.each_ref().map(AsRawHandle::as_raw_handle);
    let jobs: [HANDLE; 1] = [job.as_raw_handle()];

    let mut attributes = AttributeList::new(2)?;
    // SAFETY: `jobs` and `stdio` are locals that outlive the `CreateProcessW`
    // call below, the only use of the list.
    unsafe {
        attributes.set(
            PROC_THREAD_ATTRIBUTE_JOB_LIST,
            jobs.as_ptr().cast(),
            size_of_val(&jobs),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            stdio.as_ptr().cast(),
            size_of_val(&stdio),
        )?;
    }

    // SAFETY: an all-zero STARTUPINFOEXW is valid plain data.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = u32::try_from(size_of::<STARTUPINFOEXW>()).expect("a size fits a u32");
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdio[0];
    startup.StartupInfo.hStdOutput = stdio[1];
    startup.StartupInfo.hStdError = stdio[2];
    startup.lpAttributeList = attributes.as_ptr();

    // SAFETY: an all-zero PROCESS_INFORMATION is valid plain data.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the application name and command line are NUL-terminated and the
    // command line is writable, as `CreateProcessW` requires; the extended
    // startup structure begins with the STARTUPINFOW it is passed as, and
    // carries EXTENDED_STARTUPINFO_PRESENT; every pointer in it is live for
    // the call.
    let created = unsafe {
        CreateProcessW(
            plan.application.as_ptr(),
            plan.command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            TRUE,
            EXTENDED_STARTUPINFO_PRESENT,
            ptr::null(),
            ptr::null(),
            ptr::from_ref(&startup).cast::<STARTUPINFOW>(),
            &raw mut info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call succeeded, so both handles are new and this process
    // owns them. The thread handle is never needed.
    let handle = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    unsafe { CloseHandle(info.hThread) };
    Ok(Process {
        handle,
        id: info.dwProcessId,
    })
}

/// What `CreateProcessW` is given, both NUL-terminated.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    /// The file to run.
    application: Vec<u16>,
    /// The command line it receives.
    command_line: Vec<u16>,
}

fn plan(program: &OsStr, args: &[OsString]) -> io::Result<Plan> {
    let resolved = resolve(program)?;
    if is_batch(&resolved) {
        let cmd = resolve(OsStr::new("cmd.exe"))?;
        batch_plan(&cmd, &resolved, args)
    } else {
        exe_plan(&resolved, args)
    }
}

fn is_batch(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
    })
}

/// An executable run directly, its arguments quoted for the C runtime.
fn exe_plan(exe: &Path, args: &[OsString]) -> io::Result<Plan> {
    let mut command_line = Vec::new();
    push_program(&mut command_line, exe.as_os_str())?;
    for arg in args {
        command_line.push(SPACE);
        push_arg(&mut command_line, arg)?;
    }
    command_line.push(0);
    Ok(Plan {
        application: wide_nul(exe.as_os_str())?,
        command_line,
    })
}

/// A script run as `cmd.exe /c ""script" "arg" ..."`: the outer quotes are the
/// pair `cmd` strips, and every argument is quoted whole inside them.
fn batch_plan(cmd: &Path, script: &Path, args: &[OsString]) -> io::Result<Plan> {
    let mut command_line = Vec::new();
    push_program(&mut command_line, cmd.as_os_str())?;
    command_line.extend(" /e:ON /v:OFF /d /c \"".encode_utf16());
    push_batch_part(&mut command_line, script.as_os_str())?;
    for arg in args {
        command_line.push(SPACE);
        push_batch_part(&mut command_line, arg)?;
    }
    command_line.push(QUOTE);
    command_line.push(0);
    Ok(Plan {
        application: wide_nul(cmd.as_os_str())?,
        command_line,
    })
}

/// Appends the program's path in quotes, verbatim. A path cannot hold a quote,
/// so none is escaped; one that somehow did is refused.
fn push_program(out: &mut Vec<u16>, program: &OsStr) -> io::Result<()> {
    let units = units_without_nul(program)?;
    if units.contains(&QUOTE) {
        return Err(invalid("a program path cannot contain a quote"));
    }
    out.push(QUOTE);
    out.extend(units);
    out.push(QUOTE);
    Ok(())
}

/// Appends one argument as the C runtime's command-line parser reads it back:
/// quoted when empty or holding a space or tab, with each quote backslash
/// escaped and the backslashes before it doubled.
fn push_arg(out: &mut Vec<u16>, arg: &OsStr) -> io::Result<()> {
    let units = units_without_nul(arg)?;
    let quoted = units.is_empty() || units.iter().any(|&u| u == SPACE || u == TAB);
    if quoted {
        out.push(QUOTE);
    }
    let mut backslashes = 0_usize;
    for u in units {
        if u == BACKSLASH {
            backslashes += 1;
        } else {
            if u == QUOTE {
                out.extend(std::iter::repeat_n(BACKSLASH, backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(u);
    }
    if quoted {
        out.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        out.push(QUOTE);
    }
    Ok(())
}

/// Appends a script path or one of its arguments, quoted whole, for `cmd` to
/// hand on without re-reading it as syntax.
fn push_batch_part(out: &mut Vec<u16>, part: &OsStr) -> io::Result<()> {
    let units = units_without_nul(part)?;
    if units
        .iter()
        .any(|&u| matches!(u, QUOTE | PERCENT | CR | LF))
    {
        return Err(invalid(
            "cmd.exe would re-read a quote, percent sign or line break in a batch \
             file's path or arguments as syntax, so it is refused",
        ));
    }
    out.push(QUOTE);
    out.extend(units);
    out.push(QUOTE);
    Ok(())
}

fn units_without_nul(text: &OsStr) -> io::Result<Vec<u16>> {
    let units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(invalid("a command line cannot contain a NUL"));
    }
    Ok(units)
}

fn wide_nul(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut units = units_without_nul(text)?;
    units.push(0);
    Ok(units)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// The file `name` names, found as `CreateProcess` would find it.
fn resolve(name: &OsStr) -> io::Result<std::path::PathBuf> {
    let name = wide_nul(name)?;
    let extension = wide_nul(OsStr::new(DEFAULT_EXTENSION))?;
    let mut buffer = vec![0_u16; SEARCH_BUFFER];
    loop {
        let capacity = u32::try_from(buffer.len()).expect("the buffer fits a u32");
        // SAFETY: both strings are NUL-terminated and the buffer is writable
        // for `capacity` units; a null search path means the default order and
        // a null file-part pointer is allowed.
        let written = unsafe {
            SearchPathW(
                ptr::null(),
                name.as_ptr(),
                extension.as_ptr(),
                capacity,
                buffer.as_mut_ptr(),
                ptr::null_mut(),
            )
        } as usize;
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written < buffer.len() {
            buffer.truncate(written);
            return Ok(OsString::from_wide(&buffer).into());
        }
        // Too small: `written` is the size needed, including the NUL.
        buffer.resize(written + 1, 0);
    }
}

/// A new handle to the same object as `handle`, inheritable, owned by the caller.
fn duplicate_inheritable(handle: HANDLE) -> io::Result<OwnedHandle> {
    let mut duplicate: HANDLE = ptr::null_mut();
    // SAFETY: `handle` is live for the call; both process arguments are this
    // process's own pseudo-handle, and `duplicate` is writable.
    let ok = unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &raw mut duplicate,
            0,
            TRUE,
            DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a successful DuplicateHandle returns a new handle this process
    // owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(duplicate) })
}

/// A `PROC_THREAD_ATTRIBUTE_LIST`, deleted on drop.
struct AttributeList {
    // `usize` elements so the buffer is aligned for the structure the OS
    // builds in it.
    buffer: Vec<usize>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0_usize;
        // SAFETY: a null list with a size pointer is the documented size query.
        let queried =
            unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &raw mut size) };
        // The query is documented to fail, with exactly this error, while
        // telling the size needed.
        if queried == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER.cast_signed()) {
                return Err(e);
            }
        }
        if size == 0 {
            return Err(io::Error::other(
                "InitializeProcThreadAttributeList reported a zero size",
            ));
        }
        let mut buffer = vec![0_usize; size.div_ceil(size_of::<usize>())];
        // SAFETY: the buffer holds at least `size` bytes, aligned for the list.
        let ok = unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), count, 0, &raw mut size)
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn as_ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_mut_ptr().cast()
    }

    /// # Safety
    ///
    /// `value` must point to `size` readable bytes that stay valid until the
    /// list is last used.
    unsafe fn set(&mut self, attribute: u32, value: *const c_void, size: usize) -> io::Result<()> {
        // SAFETY: the list was initialised by `new`; the caller vouches for
        // `value`.
        let ok = unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                value,
                size,
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialised by `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}
