// Copyright (c) 2026 Mike Grier
//! A trace session a test starts, enables a provider in, and reads back (WE-D-3), so that a test, here or in another
//! crate, can see what a provider's events are when they reach a session by the documented route and not by a protocol
//! no page describes.
//!
//! This is a test facility and not a wrapper of the session functions: it is behind the `test-util` feature, it starts
//! a system-wide object, and the policy it carries (a unique name, a file in the temporary directory, stopped when
//! dropped) is a test's. It uses `StartTraceW` to start a session that logs to a file, `EnableTraceEx2` to enable a
//! provider in it, `ControlTraceW` to stop it, and then `OpenTraceW`, `ProcessTrace` and `CloseTrace` to read the file
//! back with the event-record callback.
//!
//! Starting a session is a privileged act: the calling process must be elevated, or its user in the Performance Log Users
//! group. [`Session::start`] says so in its error when it is not, and does not hide it.

use std::ffi::c_void;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::System::Diagnostics::Etw::{
    CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, EVENT_CONTROL_CODE_ENABLE_PROVIDER,
    EVENT_EXTENDED_ITEM_RELATED_ACTIVITYID, EVENT_HEADER_EXT_TYPE_RELATED_ACTIVITYID,
    EVENT_HEADER_EXTENDED_DATA_ITEM, EVENT_RECORD, EVENT_TRACE_CONTROL_STOP,
    EVENT_TRACE_FILE_MODE_SEQUENTIAL, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EnableTraceEx2,
    OpenTraceW, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESSTRACE_HANDLE, ProcessTrace, StartTraceW,
    WNODE_FLAG_TRACED_GUID,
};

use crate::{EventDescriptor, Guid};

/// `OpenTraceW`'s failure: "INVALID_PROCESSTRACE_HANDLE", which is all ones.
const INVALID_PROCESSTRACE: u64 = u64::MAX;

/// A session's buffers, in kilobytes: small, and a few, since a test writes a handful of events.
const BUFFER_KB: u32 = 64;

/// One event that a session recorded.
///
/// Read only: a field may be added, which is why it cannot be built outside the crate.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recorded {
    /// The provider's id.
    pub provider: Guid,
    /// The event's descriptor, as the provider gave it.
    pub descriptor: EventDescriptor,
    /// The event's user data: the blocks the provider wrote, concatenated.
    pub data: Vec<u8>,
    /// The id of the process that wrote it.
    pub process_id: u32,
    /// The event's activity id: the one the writer gave `EventWriteTransfer`, or the writing thread's.
    pub activity: Guid,
    /// The event's related activity id, if the writer gave one: the record's extended data item of type
    /// `EVENT_HEADER_EXT_TYPE_RELATED_ACTIVITYID`, which "contains the related activity identifier if you called
    /// EventWriteTransfer to write the event".
    pub related_activity: Option<Guid>,
}

/// A running session logging to a file. It is stopped, and its file removed, when this is dropped.
pub struct Session {
    handle: CONTROLTRACE_HANDLE,
    name: Vec<u16>,
    file: PathBuf,
    enabled: Vec<Guid>,
    stopped: bool,
}

/// What distinguishes the sessions one process starts.
static NEXT: AtomicU32 = AtomicU32::new(0);

impl Session {
    /// Starts a session logging to a file in the temporary directory, with a name no other session of this process has.
    ///
    /// # Errors
    ///
    /// The operating system's. `ERROR_ACCESS_DENIED` is the usual one, and means this process is neither elevated nor
    /// run by a member of the Performance Log Users group, which starting a session requires; the error's text says so.
    pub fn start() -> io::Result<Self> {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("win-etw-sys-test-{}-{n}", std::process::id());
        let file = std::env::temp_dir().join(format!("{name}.etl"));
        let name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        let file_units: Vec<u16> = file.as_os_str().encode_wide().chain([0]).collect();

        // The properties are followed by the session's name and the log file's name, in one buffer, which the call
        // fills in. It is 8-aligned: the structure holds 64-bit fields.
        let names = (name.len() + file_units.len()) * size_of::<u16>();
        let total = size_of::<EVENT_TRACE_PROPERTIES>() + names;
        let mut buffer = vec![0_u64; total.div_ceil(size_of::<u64>())];
        let properties = buffer.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>();
        // SAFETY: the buffer is 8-aligned, zeroed, and at least the structure's size, so the structure is valid in it;
        // the offsets are within the buffer and the names are copied to where they say, each with its terminator.
        unsafe {
            (*properties).Wnode.BufferSize = u32::try_from(total).expect("a small buffer");
            (*properties).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            // The clock the events are stamped with: 1 is the performance counter.
            (*properties).Wnode.ClientContext = 1;
            (*properties).BufferSize = BUFFER_KB;
            (*properties).MinimumBuffers = 2;
            (*properties).LogFileMode = EVENT_TRACE_FILE_MODE_SEQUENTIAL;
            let name_offset = size_of::<EVENT_TRACE_PROPERTIES>();
            let file_offset = name_offset + name.len() * size_of::<u16>();
            (*properties).LoggerNameOffset = u32::try_from(name_offset).expect("small");
            (*properties).LogFileNameOffset = u32::try_from(file_offset).expect("small");
            let base = buffer.as_mut_ptr().cast::<u8>();
            std::ptr::copy_nonoverlapping(
                name.as_ptr().cast::<u8>(),
                base.add(name_offset),
                name.len() * size_of::<u16>(),
            );
            std::ptr::copy_nonoverlapping(
                file_units.as_ptr().cast::<u8>(),
                base.add(file_offset),
                file_units.len() * size_of::<u16>(),
            );
        }
        let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
        // SAFETY: the handle is valid for the call to write, the name is NUL-terminated and outlives the call, and the
        // properties are the buffer built above, which the call may write to.
        let code = unsafe { StartTraceW(&mut handle, name.as_ptr(), properties) };
        if code != 0 {
            return Err(denied(code, "StartTraceW"));
        }
        Ok(Self {
            handle,
            name,
            file,
            enabled: Vec::new(),
            stopped: false,
        })
    }

    /// Enables `provider` in the session for events whose level is at most `level` and whose keyword shares a bit with
    /// `match_any_keyword`, by `EnableTraceEx2`.
    ///
    /// # Errors
    ///
    /// The operating system's.
    pub fn enable(&mut self, provider: Guid, level: u8, match_any_keyword: u64) -> io::Result<()> {
        // SAFETY: the handle is the session's, and the GUID is valid for the call to read; the filter and parameters are
        // null, which the call allows.
        let code = unsafe {
            EnableTraceEx2(
                self.handle,
                provider.raw(),
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                level,
                match_any_keyword,
                0,
                0,
                null(),
            )
        };
        if code != 0 {
            return Err(denied(code, "EnableTraceEx2"));
        }
        self.enabled.push(provider);
        Ok(())
    }

    /// Stops the session, reads its file back, and returns the events of the providers enabled in it, in the order they
    /// were recorded.
    ///
    /// # Errors
    ///
    /// The operating system's, for the stop or for reading the file. The session is stopped and its file removed either
    /// way.
    pub fn stop(mut self) -> io::Result<Vec<Recorded>> {
        self.stop_session()?;
        let read = read_file(&self.file, &self.enabled);
        let _ = std::fs::remove_file(&self.file);
        read
    }

    fn stop_session(&mut self) -> io::Result<()> {
        if std::mem::replace(&mut self.stopped, true) {
            return Ok(());
        }
        let total = size_of::<EVENT_TRACE_PROPERTIES>() + 2 * 1024;
        let mut buffer = vec![0_u64; total.div_ceil(size_of::<u64>())];
        let properties = buffer.as_mut_ptr().cast::<EVENT_TRACE_PROPERTIES>();
        // SAFETY: as in `start`: an aligned, zeroed buffer, large enough for the structure and the two names the call
        // writes back, whose offsets are set so that it knows where.
        let code = unsafe {
            (*properties).Wnode.BufferSize = u32::try_from(total).expect("a small buffer");
            (*properties).LoggerNameOffset =
                u32::try_from(size_of::<EVENT_TRACE_PROPERTIES>()).expect("small");
            (*properties).LogFileNameOffset =
                u32::try_from(size_of::<EVENT_TRACE_PROPERTIES>() + 1024).expect("small");
            ControlTraceW(
                self.handle,
                self.name.as_ptr(),
                properties,
                EVENT_TRACE_CONTROL_STOP,
            )
        };
        if code != 0 {
            return Err(denied(code, "ControlTraceW"));
        }
        Ok(())
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("file", &self.file)
            .field("enabled", &self.enabled)
            .field("stopped", &self.stopped)
            .finish_non_exhaustive()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.stop_session();
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// The error for a failed call, with what to do about the one that is usually the cause.
fn denied(code: u32, call: &str) -> io::Error {
    const ACCESS_DENIED: u32 = 5;
    let error = io::Error::from_raw_os_error(code.cast_signed());
    if code == ACCESS_DENIED {
        io::Error::new(
            error.kind(),
            format!(
                "{call} was denied: starting a trace session needs an elevated process, or a user in the Performance Log Users group ({error})"
            ),
        )
    } else {
        io::Error::new(error.kind(), format!("{call} failed: {error}"))
    }
}

/// What the callback collects, behind the context pointer the log file carries.
struct Collected {
    wanted: Vec<Guid>,
    events: Vec<Recorded>,
}

/// Called by `ProcessTrace` for each event record.
///
/// # Safety
///
/// `record` is the record `ProcessTrace` hands to its callback, whose context is the `Collected` that `read_file`
/// gave `OpenTraceW` and which is not otherwise in use.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // SAFETY: the caller's.
    let (header, user, length, extended, collected) = unsafe {
        let record = &*record;
        (
            record.EventHeader,
            record.UserData.cast::<u8>(),
            usize::from(record.UserDataLength),
            extended_items(record),
            &mut *record.UserContext.cast::<Collected>(),
        )
    };
    let provider = Guid::from_raw(header.ProviderId);
    if !collected.wanted.contains(&provider) {
        return;
    }
    let data = if length == 0 || user.is_null() {
        Vec::new()
    } else {
        // SAFETY: the record's user data is `UserDataLength` bytes at `UserData`, valid for the callback.
        unsafe { std::slice::from_raw_parts(user, length) }.to_vec()
    };
    let d = header.EventDescriptor;
    collected.events.push(Recorded {
        provider,
        descriptor: EventDescriptor {
            id: d.Id,
            version: d.Version,
            channel: d.Channel,
            level: d.Level,
            opcode: d.Opcode,
            task: d.Task,
            keyword: d.Keyword,
        },
        data,
        process_id: header.ProcessId,
        activity: Guid::from_raw(header.ActivityId),
        // SAFETY: the items are the record's, valid for the callback.
        related_activity: unsafe { related_activity(extended) },
    });
}

/// The record's extended data items.
///
/// # Safety
///
/// `record` is a record `ProcessTrace` handed to its callback, and the slice is used only while the callback runs.
unsafe fn extended_items(record: &EVENT_RECORD) -> &[EVENT_HEADER_EXTENDED_DATA_ITEM] {
    let count = usize::from(record.ExtendedDataCount);
    if count == 0 || record.ExtendedData.is_null() {
        return &[];
    }
    // SAFETY: the record's `ExtendedData` is `ExtendedDataCount` items, valid for the callback.
    unsafe { std::slice::from_raw_parts(record.ExtendedData, count) }
}

/// The related activity id among a record's extended data items, if it has one.
///
/// # Safety
///
/// The items are a record's, and each `DataPtr` is valid for the `DataSize` bytes it gives while the callback runs.
unsafe fn related_activity(items: &[EVENT_HEADER_EXTENDED_DATA_ITEM]) -> Option<Guid> {
    let item = items.iter().find(|item| {
        u32::from(item.ExtType) == EVENT_HEADER_EXT_TYPE_RELATED_ACTIVITYID
            && usize::from(item.DataSize) >= size_of::<EVENT_EXTENDED_ITEM_RELATED_ACTIVITYID>()
    })?;
    // SAFETY: the page says `DataPtr` points to an `EVENT_EXTENDED_ITEM_RELATED_ACTIVITYID` for this type, and its size
    // was checked; the read is unaligned because the page promises no alignment.
    let value = unsafe {
        std::ptr::read_unaligned(item.DataPtr as *const EVENT_EXTENDED_ITEM_RELATED_ACTIVITYID)
    };
    Some(Guid::from_raw(value.RelatedActivityId))
}

/// Reads the events of `wanted` providers out of a session's log file.
fn read_file(path: &Path, wanted: &[Guid]) -> io::Result<Vec<Recorded>> {
    let mut file: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut collected = Collected {
        wanted: wanted.to_vec(),
        events: Vec::new(),
    };
    // SAFETY: all-zero is a valid `EVENT_TRACE_LOGFILEW`, which the call and the fields set below describe.
    let mut log: EVENT_TRACE_LOGFILEW = unsafe { zeroed() };
    log.LogFileName = file.as_mut_ptr();
    log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_EVENT_RECORD;
    log.Anonymous2.EventRecordCallback = Some(on_event);
    log.Context = (&raw mut collected).cast::<c_void>();
    // SAFETY: `log` is valid for the call, its file name is NUL-terminated and outlives it, and its context is the
    // collection, which is not touched but by the callback until the trace is closed.
    let handle = unsafe { OpenTraceW(&mut log) };
    if handle.Value == INVALID_PROCESSTRACE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the handle is the one just opened, and the times are null, which the call allows.
    let code = unsafe { ProcessTrace(&handle, 1, null(), null()) };
    // SAFETY: the handle is the one opened above, closed once.
    unsafe {
        CloseTrace(PROCESSTRACE_HANDLE {
            Value: handle.Value,
        })
    };
    if code != 0 {
        return Err(denied(code, "ProcessTrace"));
    }
    Ok(collected.events)
}
