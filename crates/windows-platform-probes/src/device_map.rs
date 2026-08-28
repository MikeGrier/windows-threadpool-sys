// Copyright (c) Mike Grier.

//! Whether impersonation changes which DOS device map a thread resolves drive
//! letters in.
//!
//! Drive letters are symbolic links in the object manager namespace. Real local
//! volumes live in the machine-wide `\GLOBAL??` directory, but `subst` drives
//! and mapped network drives live in a **per-logon-session** directory keyed by
//! the token's authentication id (LUID).
//!
//! This is why a path resolved on a submitting thread and opened on a worker
//! under a captured token is not obviously the same path. It is the measurement
//! behind the session-relative drive-letter hazard that
//! `windows-namespace-request-sys` documents and deliberately does not close.
//!
//! # No credentials are needed to measure it
//!
//! An earlier reading was that this needed a second logon session and a
//! password. It does not: `ImpersonateAnonymousToken` places the calling thread
//! in the Anonymous logon session -- a genuinely different LUID -- and requires
//! nothing. A `subst` drive created in our own session is then the instrument:
//! it exists in our map and not in the anonymous one.
//!
//! `QueryDosDeviceW` is used rather than `CreateFileW` because it asks the
//! object manager directly, so a negative cannot be confused with a file that
//! merely is not there.
//!
//! Migrated from the throwaway `ctx-probe` spike (Probe DM).
//!
//! # Tier: ignored
//!
//! It defines and removes a `subst`-style drive letter, which is
//! process-visible state, and it needs a free drive letter to do it. Both make
//! it wrong to run in an ordinary test pass without asking.

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LUID};
use windows_sys::Win32::Security::{
    GetTokenInformation, ImpersonateAnonymousToken, RevertToSelf, TOKEN_QUERY, TOKEN_STATISTICS,
    TokenStatistics,
};
use windows_sys::Win32::Storage::FileSystem::{
    DDD_RAW_TARGET_PATH, DDD_REMOVE_DEFINITION, DefineDosDeviceW, GetLogicalDrives, QueryDosDeviceW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
};

/// What one observation of a drive letter found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapObservation {
    /// The drive letter asked about, such as `"Q:"`.
    pub letter: String,
    /// What the letter resolved to, if anything.
    pub target: Option<String>,
    /// The logon-session id of the token in effect, when it could be read.
    ///
    /// The *effective* token: the thread's when impersonating, and the
    /// process's otherwise. Printed by the binary because it is the proof that
    /// the impersonated context really is a different session rather than a
    /// no-op.
    pub logon_session: Option<(u32, i32)>,
}

impl MapObservation {
    /// The letter resolved to something.
    #[must_use]
    pub fn is_found(&self) -> bool {
        self.target.is_some()
    }
}

/// What the whole probe concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMapFinding {
    /// The letter as the process's own session sees it.
    pub own_session: MapObservation,
    /// The same letter while impersonating the anonymous logon session.
    pub anonymous_session: MapObservation,
}

impl DeviceMapFinding {
    /// Impersonation changed which map the letter resolved in.
    ///
    /// This is the finding: the same letter, on the same thread, means
    /// different things depending on the token in effect.
    #[must_use]
    pub fn impersonation_changes_the_map(&self) -> bool {
        self.own_session.is_found() && !self.anonymous_session.is_found()
    }

    /// The two contexts really were different logon sessions.
    ///
    /// The control. If the LUIDs matched, "the letter disappeared" would be
    /// evidence of something else entirely.
    #[must_use]
    pub fn sessions_differ(&self) -> bool {
        match (
            self.own_session.logon_session,
            self.anonymous_session.logon_session,
        ) {
            (Some(own), Some(anonymous)) => own != anonymous,
            _ => false,
        }
    }
}

/// Reads the logon-session id of the token currently in effect.
///
/// Falls back to the **process** token when the thread is not impersonating.
/// That fallback is the whole point of the control: without it the
/// non-impersonating observation reports "no session", and a comparison against
/// it could never show two sessions differing -- so the control would silently
/// be incapable of passing, which is worse than having no control.
fn effective_logon_session() -> Option<(u32, i32)> {
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: GetCurrentThread is a pseudo-handle; `token` is writable.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &raw mut token) };

    if opened == 0 {
        // SAFETY: GetCurrentProcess is a pseudo-handle; `token` is writable.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) };
        if opened == 0 {
            return None;
        }
    }

    let mut statistics = unsafe { std::mem::zeroed::<TOKEN_STATISTICS>() };
    let mut returned = 0_u32;
    // SAFETY: `token` is live; the destination matches TokenStatistics' size.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenStatistics,
            std::ptr::from_mut(&mut statistics).cast(),
            u32::try_from(size_of::<TOKEN_STATISTICS>()).expect("a small struct fits a u32"),
            &raw mut returned,
        )
    };
    // SAFETY: the handle is owned here and closed once.
    unsafe { CloseHandle(token) };

    if read == 0 {
        return None;
    }

    let LUID { LowPart, HighPart } = statistics.AuthenticationId;
    Some((LowPart, HighPart))
}

/// Asks the object manager what `letter` (such as `"Q:"`) currently means.
fn query(letter: &str) -> MapObservation {
    let name: Vec<u16> = letter.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buffer = vec![0_u16; 1024];

    // SAFETY: `name` is NUL-terminated and `buffer` is writable for its length.
    let written = unsafe {
        QueryDosDeviceW(
            name.as_ptr(),
            buffer.as_mut_ptr(),
            u32::try_from(buffer.len()).expect("a fixed buffer length fits a u32"),
        )
    };

    let target = if written == 0 {
        None
    } else {
        let text: String = String::from_utf16_lossy(&buffer[..written as usize]);
        Some(text.trim_end_matches('\0').to_owned())
    };

    MapObservation {
        letter: letter.to_owned(),
        target,
        logon_session: effective_logon_session(),
    }
}

/// Finds a drive letter this session is not using.
///
/// Returns `None` when every letter is taken, in which case the probe cannot
/// run rather than reporting a misleading result.
#[must_use]
pub fn free_drive_letter() -> Option<String> {
    // SAFETY: no preconditions.
    let used = unsafe { GetLogicalDrives() };

    // Start at H: to stay clear of the letters a system conventionally uses.
    (b'H'..=b'Z').find_map(|letter| {
        let index = u32::from(letter - b'A');
        (used & (1 << index) == 0).then(|| format!("{}:", letter as char))
    })
}

/// Defines `letter` as a `subst`-style link to `target`, runs the probe, and
/// removes it again.
///
/// # Panics
///
/// Panics if the drive cannot be defined, since a probe that measured nothing
/// would be worse than one that stopped.
#[must_use]
pub fn measure_with_subst(letter: &str, target: &str) -> DeviceMapFinding {
    let name: Vec<u16> = letter.encode_utf16().chain(std::iter::once(0)).collect();
    let path: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: both strings are NUL-terminated and outlive the call.
    let defined = unsafe { DefineDosDeviceW(DDD_RAW_TARGET_PATH, name.as_ptr(), path.as_ptr()) };
    assert_ne!(
        defined,
        0,
        "define {letter} -> {target} (last error {})",
        // SAFETY: no preconditions.
        unsafe { GetLastError() }
    );

    let own_session = query(letter);

    // SAFETY: no preconditions; places this thread in the Anonymous logon
    // session, which needs no credentials.
    let impersonated = unsafe { ImpersonateAnonymousToken(GetCurrentThread()) };
    let anonymous_session = if impersonated == 0 {
        MapObservation {
            letter: letter.to_owned(),
            target: None,
            logon_session: None,
        }
    } else {
        let observed = query(letter);
        // SAFETY: restores this thread to its own identity.
        unsafe { RevertToSelf() };
        observed
    };

    // SAFETY: removes the definition added above; both strings are still live.
    unsafe {
        DefineDosDeviceW(
            DDD_REMOVE_DEFINITION | DDD_RAW_TARGET_PATH,
            name.as_ptr(),
            path.as_ptr(),
        )
    };

    DeviceMapFinding {
        own_session,
        anonymous_session,
    }
}
