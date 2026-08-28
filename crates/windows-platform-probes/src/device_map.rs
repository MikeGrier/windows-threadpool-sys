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

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows_sys::Win32::Security::{
    GetTokenInformation, ImpersonateAnonymousToken, RevertToSelf, TOKEN_QUERY, TOKEN_STATISTICS,
    TokenStatistics,
};
use windows_sys::Win32::Storage::FileSystem::{
    DDD_EXACT_MATCH_ON_REMOVE, DDD_RAW_TARGET_PATH, DDD_REMOVE_DEFINITION, DefineDosDeviceW,
    GetLogicalDrives, QueryDosDeviceW,
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

    /// The individual targets the letter resolves to.
    ///
    /// `QueryDosDeviceW` returns a `MULTI_SZ`, and a letter really can carry
    /// more than one target: `DefineDosDeviceW` *stacks* definitions rather
    /// than replacing them, and each removal pops one. More than one entry
    /// here means somebody else defined the same letter.
    #[must_use]
    pub fn entries(&self) -> Vec<&str> {
        self.target.as_deref().map_or_else(Vec::new, |text| {
            text.split('\0').filter(|entry| !entry.is_empty()).collect()
        })
    }

    /// The letter resolves to `expected` and to nothing else.
    #[must_use]
    pub fn is_exactly(&self, expected: &str) -> bool {
        self.entries() == [expected]
    }
}

/// What the whole probe concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMapFinding {
    /// The letter as the process's own session sees it.
    pub own_session: MapObservation,
    /// The same letter while impersonating the anonymous logon session.
    pub anonymous_session: MapObservation,
    /// The target this probe's own claim points at.
    ///
    /// Unique per claim, so a stray definition left by anything else is
    /// distinguishable from ours rather than being mistaken for it.
    pub target: String,
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

    /// Our own claim is the only definition on the letter.
    ///
    /// The fixture check. If another definition were stacked on the same
    /// letter, "the letter disappeared while impersonating" could be reporting
    /// somebody else's removal rather than a device-map difference.
    #[must_use]
    pub fn claim_is_exclusive(&self) -> bool {
        self.own_session.is_exactly(&self.target)
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

/// Serialises drive-letter claims within this process.
///
/// `GetLogicalDrives` is a snapshot and `DefineDosDeviceW` does not reserve
/// anything, so two threads reading the same snapshot both see the same letter
/// free and both define it. Definitions **stack** rather than replace, and each
/// removal pops one -- so one probe observes a two-entry `MULTI_SZ` and the
/// other observes the letter already gone.
///
/// That second outcome is the dangerous one. It is a false negative on the
/// fixture check, produced by a sibling test rather than by the platform, and
/// it looks exactly like Windows refusing something -- the failure mode this
/// crate exists to avoid.
static CLAIM: Mutex<()> = Mutex::new(());

/// A drive letter claimed for the duration of a probe, removed on drop.
///
/// Claiming is atomic with respect to other claims in this process: the lock is
/// held for the claim's whole life, so no two probes can be looking at the same
/// letter at once. The definition is removed on drop, including while
/// unwinding -- the previous code removed it with a plain statement after the
/// measurement, so a panic anywhere in between leaked a process-visible drive
/// letter.
pub struct SubstDrive {
    letter: String,
    target: String,
    name: Vec<u16>,
    path: Vec<u16>,
    /// Held for as long as the claim exists. Never read.
    _claim: MutexGuard<'static, ()>,
}

impl SubstDrive {
    /// Claims a free drive letter pointing at a target unique to this claim.
    ///
    /// Returns `None` when every candidate letter is taken, so the caller can
    /// report "cannot measure" rather than a misleading result.
    #[must_use]
    pub fn claim(label: &str) -> Option<Self> {
        // Poisoning only means a previous probe panicked; the letter it held
        // was still removed by its Drop, so the lock is safe to take over.
        let claim = CLAIM
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let unique = NEXT.fetch_add(1, Ordering::Relaxed);
        // Unique per claim so a definition left behind by anything else is
        // distinguishable from ours. The path is never opened -- the probe only
        // asks the object manager what the letter means -- so it does not need
        // to name a volume that exists.
        let target = format!(
            r"\Device\HarddiskVolume1\probe-{}-{label}-{unique}",
            std::process::id()
        );
        let path = wide(&target);

        // SAFETY: no preconditions.
        let used = unsafe { GetLogicalDrives() };

        // Start at H: to stay clear of the letters a system conventionally uses.
        for byte in b'H'..=b'Z' {
            if used & (1 << u32::from(byte - b'A')) != 0 {
                continue;
            }

            let letter = format!("{}:", byte as char);
            let name = wide(&letter);

            // SAFETY: both strings are NUL-terminated and outlive the call.
            let defined =
                unsafe { DefineDosDeviceW(DDD_RAW_TARGET_PATH, name.as_ptr(), path.as_ptr()) };
            if defined == 0 {
                continue;
            }

            // Defining succeeds even on a letter that is already defined, so
            // success is not proof of exclusivity -- it has to be read back.
            if query(&letter).is_exactly(&target) {
                return Some(Self {
                    letter,
                    target,
                    name,
                    path,
                    _claim: claim,
                });
            }

            // Something else holds this letter and ours is stacked on top of
            // it. Remove only our own entry and try the next letter.
            remove(&name, &path);
        }

        None
    }

    /// The claimed letter, such as `"H:"`.
    #[must_use]
    pub fn letter(&self) -> &str {
        &self.letter
    }

    /// The target this claim points at.
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }
}

impl Drop for SubstDrive {
    fn drop(&mut self) {
        remove(&self.name, &self.path);
    }
}

/// Removes exactly the `name` -> `path` definition, leaving any other alone.
///
/// `DDD_EXACT_MATCH_ON_REMOVE` is what makes that "exactly": without it,
/// Windows removes the most recent definition on the letter whatever it points
/// at, so a probe could remove somebody else's mapping instead of its own.
fn remove(name: &[u16], path: &[u16]) {
    // SAFETY: both strings are NUL-terminated and live for the call.
    unsafe {
        DefineDosDeviceW(
            DDD_REMOVE_DEFINITION | DDD_EXACT_MATCH_ON_REMOVE | DDD_RAW_TARGET_PATH,
            name.as_ptr(),
            path.as_ptr(),
        )
    };
}

/// A NUL-terminated UTF-16 copy, as every one of these calls wants.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Runs the probe against an already-claimed drive letter.
///
/// Takes the claim rather than a letter and target so the definition cannot
/// outlive the measurement, and so no caller can pass a letter it has not
/// established exclusive use of.
#[must_use]
pub fn measure_with_subst(drive: &SubstDrive) -> DeviceMapFinding {
    let own_session = query(drive.letter());

    // SAFETY: no preconditions; places this thread in the Anonymous logon
    // session, which needs no credentials.
    let impersonated = unsafe { ImpersonateAnonymousToken(GetCurrentThread()) };
    let anonymous_session = if impersonated == 0 {
        MapObservation {
            letter: drive.letter().to_owned(),
            target: None,
            logon_session: None,
        }
    } else {
        let observed = query(drive.letter());
        // SAFETY: restores this thread to its own identity.
        unsafe { RevertToSelf() };
        observed
    };

    DeviceMapFinding {
        own_session,
        anonymous_session,
        target: drive.target().to_owned(),
    }
}
