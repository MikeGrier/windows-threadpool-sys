// Copyright (c) 2026 Mike Grier
//! Real-Windows capture, transport, application, and restoration behavior.

#![cfg(windows)]

use std::io;
use std::marker::PhantomData;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::rc::Rc;
use std::sync::{Arc, Barrier};

use windows_impersonation_token_sys::{CaptureFailure, ImpersonationToken};
use windows_sys::Win32::Foundation::{
    ERROR_CANT_OPEN_ANONYMOUS, ERROR_NO_TOKEN, FALSE, HANDLE, LUID, TRUE,
};
use windows_sys::Win32::Security::{
    DuplicateTokenEx, GetTokenInformation, SECURITY_IMPERSONATION_LEVEL, SecurityAnonymous,
    SecurityDelegation, SecurityIdentification, SecurityImpersonation, TOKEN_ACCESS_MASK,
    TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TOKEN_STATISTICS, TokenImpersonation,
    TokenImpersonationLevel, TokenStatistics,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
};

const TEST_TOKEN_ACCESS: TOKEN_ACCESS_MASK = TOKEN_IMPERSONATE | TOKEN_QUERY;
const CONCURRENT_WORKERS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TokenIdentity {
    token_id: (u32, i32),
    authentication_id: (u32, i32),
}

struct ThreadTokenScope {
    previous: Option<OwnedHandle>,
    _thread_bound: PhantomData<Rc<()>>,
}

impl ThreadTokenScope {
    fn replace(token: Option<&OwnedHandle>) -> Self {
        let previous = open_thread_token(TOKEN_IMPERSONATE);
        let raw = token.map_or(ptr::null_mut(), AsRawHandle::as_raw_handle);

        // SAFETY: a null thread pointer selects this thread. raw is null or a
        // live impersonation-token handle with TOKEN_IMPERSONATE.
        let applied = unsafe { SetThreadToken(ptr::null(), raw) };
        assert_ne!(
            applied,
            FALSE,
            "set test thread token: {}",
            io::Error::last_os_error()
        );

        Self {
            previous,
            _thread_bound: PhantomData,
        }
    }
}

impl Drop for ThreadTokenScope {
    fn drop(&mut self) {
        let raw = self
            .previous
            .as_ref()
            .map_or(ptr::null_mut(), AsRawHandle::as_raw_handle);

        // SAFETY: a null thread pointer selects this thread. raw is null or the
        // still-live exact token handle saved by replace.
        let restored = unsafe { SetThreadToken(ptr::null(), raw) };
        assert_ne!(
            restored,
            FALSE,
            "restore test thread token: {}",
            io::Error::last_os_error()
        );
    }
}

fn open_thread_token(access: TOKEN_ACCESS_MASK) -> Option<OwnedHandle> {
    let mut raw = ptr::null_mut();

    // SAFETY: GetCurrentThread is valid for this call and raw is writable.
    let opened = unsafe { OpenThreadToken(GetCurrentThread(), access, TRUE, &raw mut raw) };
    if opened != FALSE {
        // SAFETY: OpenThreadToken returned a new owned CloseHandle handle.
        return Some(unsafe { OwnedHandle::from_raw_handle(raw) });
    }

    let error = io::Error::last_os_error();
    if error.raw_os_error()
        == Some(i32::try_from(ERROR_NO_TOKEN).expect("ERROR_NO_TOKEN fits in i32"))
    {
        None
    } else {
        panic!("open current test thread token: {error}");
    }
}

fn duplicate_process_token(level: SECURITY_IMPERSONATION_LEVEL) -> OwnedHandle {
    let mut process_raw = ptr::null_mut();

    // SAFETY: GetCurrentProcess is valid for this call and process_raw is writable.
    let opened =
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE, &raw mut process_raw) };
    assert_ne!(
        opened,
        FALSE,
        "open process token for test: {}",
        io::Error::last_os_error()
    );
    // SAFETY: OpenProcessToken returned a new owned CloseHandle handle.
    let process = unsafe { OwnedHandle::from_raw_handle(process_raw) };

    let mut token_raw: HANDLE = ptr::null_mut();
    // SAFETY: process has TOKEN_DUPLICATE and token_raw is writable. Null
    // security attributes create a non-inheritable impersonation-token handle.
    let duplicated = unsafe {
        DuplicateTokenEx(
            process.as_raw_handle(),
            TEST_TOKEN_ACCESS,
            ptr::null(),
            level,
            TokenImpersonation,
            &raw mut token_raw,
        )
    };
    assert_ne!(
        duplicated,
        FALSE,
        "duplicate process token at level {level}: {}",
        io::Error::last_os_error()
    );

    // SAFETY: DuplicateTokenEx returned a new owned CloseHandle handle.
    unsafe { OwnedHandle::from_raw_handle(token_raw) }
}

fn token_statistics(handle: &OwnedHandle) -> TOKEN_STATISTICS {
    let mut statistics = TOKEN_STATISTICS::default();
    let mut returned = 0;
    let size = u32::try_from(size_of::<TOKEN_STATISTICS>()).expect("statistics size fits in u32");

    // SAFETY: handle has TOKEN_QUERY and statistics is a correctly sized,
    // writable TOKEN_STATISTICS buffer.
    let queried = unsafe {
        GetTokenInformation(
            handle.as_raw_handle(),
            TokenStatistics,
            (&raw mut statistics).cast(),
            size,
            &raw mut returned,
        )
    };
    assert_ne!(
        queried,
        FALSE,
        "query token statistics: {}",
        io::Error::last_os_error()
    );
    statistics
}

fn luid(value: LUID) -> (u32, i32) {
    (value.LowPart, value.HighPart)
}

fn token_identity(handle: &OwnedHandle) -> TokenIdentity {
    let statistics = token_statistics(handle);
    TokenIdentity {
        token_id: luid(statistics.TokenId),
        authentication_id: luid(statistics.AuthenticationId),
    }
}

fn current_identity() -> Option<TokenIdentity> {
    open_thread_token(TOKEN_QUERY).map(|handle| token_identity(&handle))
}

fn current_level() -> Option<SECURITY_IMPERSONATION_LEVEL> {
    let handle = open_thread_token(TOKEN_QUERY)?;
    let mut level = SecurityAnonymous;
    let mut returned = 0;
    let size = u32::try_from(size_of::<SECURITY_IMPERSONATION_LEVEL>())
        .expect("impersonation level size fits in u32");

    // SAFETY: handle has TOKEN_QUERY and level is a correctly sized writable
    // SECURITY_IMPERSONATION_LEVEL buffer.
    let queried = unsafe {
        GetTokenInformation(
            handle.as_raw_handle(),
            TokenImpersonationLevel,
            (&raw mut level).cast(),
            size,
            &raw mut returned,
        )
    };
    assert_ne!(
        queried,
        FALSE,
        "query current impersonation level: {}",
        io::Error::last_os_error()
    );
    Some(level)
}

fn capture_at_level(level: SECURITY_IMPERSONATION_LEVEL) -> ImpersonationToken {
    let source = duplicate_process_token(level);
    let scope = ThreadTokenScope::replace(Some(&source));
    let captured = ImpersonationToken::capture().expect("capture test impersonation token");
    drop(scope);
    captured
}

#[test]
fn captures_process_context_when_the_thread_has_no_token() {
    let _clean = ThreadTokenScope::replace(None);
    assert_eq!(current_level(), None);

    let captured = ImpersonationToken::capture().expect("capture process context");
    assert_eq!(current_level(), None);

    captured
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityImpersonation));
        })
        .expect("apply captured process context");
    assert_eq!(current_level(), None);
}

#[test]
fn captures_an_impersonated_thread() {
    let _clean = ThreadTokenScope::replace(None);
    let source = duplicate_process_token(SecurityImpersonation);
    let source_authentication = token_identity(&source).authentication_id;
    let captured = {
        let _source_scope = ThreadTokenScope::replace(Some(&source));
        ImpersonationToken::capture().expect("capture impersonated context")
    };

    captured
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityImpersonation));
            assert_eq!(
                current_identity()
                    .expect("captured context supplies a thread token")
                    .authentication_id,
                source_authentication
            );
        })
        .expect("apply impersonated context");
    assert_eq!(current_level(), None);
}

#[test]
fn transports_a_captured_context_to_another_thread() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityIdentification);

    std::thread::spawn(move || {
        assert_eq!(current_level(), None);
        captured
            .with_impersonation(|| {
                assert_eq!(current_level(), Some(SecurityIdentification));
            })
            .expect("apply on another thread");
        assert_eq!(current_level(), None);
    })
    .join()
    .expect("cross-thread capture test does not panic");
}

#[test]
fn reuses_one_capture_repeatedly() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityIdentification);

    for _ in 0..16 {
        captured
            .with_impersonation(|| {
                assert_eq!(current_level(), Some(SecurityIdentification));
            })
            .expect("reapply captured token");
        assert_eq!(current_level(), None);
    }
}

#[test]
fn nested_scopes_restore_the_outer_captured_context() {
    let _clean = ThreadTokenScope::replace(None);
    let outer = capture_at_level(SecurityIdentification);
    let inner = capture_at_level(SecurityDelegation);

    outer
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityIdentification));
            inner
                .with_impersonation(|| {
                    assert_eq!(current_level(), Some(SecurityDelegation));
                })
                .expect("apply nested captured token");
            assert_eq!(current_level(), Some(SecurityIdentification));
        })
        .expect("apply outer captured token");
    assert_eq!(current_level(), None);
}

#[test]
fn restores_the_exact_prior_token_object() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityDelegation);
    let previous = duplicate_process_token(SecurityIdentification);
    let _previous_scope = ThreadTokenScope::replace(Some(&previous));
    let before = current_identity().expect("previous token is applied");

    captured
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityDelegation));
            assert_ne!(
                current_identity().expect("captured token is applied"),
                before
            );
        })
        .expect("apply captured token");

    assert_eq!(
        current_identity().expect("previous token is restored"),
        before
    );
    assert_eq!(current_level(), Some(SecurityIdentification));
}

#[test]
fn returns_a_successful_closure_value() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = ImpersonationToken::capture().expect("capture process context");

    let value = captured
        .with_impersonation(|| 42)
        .expect("apply captured process context");

    assert_eq!(value, 42);
    assert_eq!(current_level(), None);
}

#[test]
fn preserves_a_fallible_closure_error() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = ImpersonationToken::capture().expect("capture process context");

    let result = captured
        .with_impersonation(|| Err::<(), _>("closure error"))
        .expect("token application succeeds");

    assert_eq!(result, Err("closure error"));
    assert_eq!(current_level(), None);
}

#[test]
fn restores_the_exact_prior_token_during_unwind() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityDelegation);
    let previous = duplicate_process_token(SecurityIdentification);
    let _previous_scope = ThreadTokenScope::replace(Some(&previous));
    let before = current_identity().expect("previous token is applied");

    let unwind = catch_unwind(AssertUnwindSafe(|| {
        let _ = captured.with_impersonation(|| -> () { panic!("closure panic") });
    }));

    assert!(unwind.is_err());
    assert_eq!(
        current_identity().expect("previous token is restored during unwind"),
        before
    );
    assert_eq!(current_level(), Some(SecurityIdentification));
}

#[test]
fn capture_outlives_the_source_token_handle() {
    let _clean = ThreadTokenScope::replace(None);
    let source = duplicate_process_token(SecurityImpersonation);
    let source_authentication = token_identity(&source).authentication_id;
    let captured = {
        let _source_scope = ThreadTokenScope::replace(Some(&source));
        ImpersonationToken::capture().expect("capture source token")
    };
    drop(source);

    captured
        .with_impersonation(|| {
            assert_eq!(
                current_identity()
                    .expect("captured token remains live")
                    .authentication_id,
                source_authentication
            );
        })
        .expect("apply after source handle closes");
}

#[test]
fn one_capture_can_be_applied_concurrently() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityImpersonation);
    let barrier = Arc::new(Barrier::new(CONCURRENT_WORKERS + 1));
    let mut workers = Vec::with_capacity(CONCURRENT_WORKERS);

    for _ in 0..CONCURRENT_WORKERS {
        let token = captured.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            assert_eq!(current_level(), None);
            barrier.wait();
            token
                .with_impersonation(|| {
                    assert_eq!(current_level(), Some(SecurityImpersonation));
                })
                .expect("apply shared captured token");
            assert_eq!(current_level(), None);
        }));
    }

    barrier.wait();
    for worker in workers {
        worker.join().expect("concurrent worker does not panic");
    }
}

#[test]
fn preserves_identification_level() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityIdentification);

    captured
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityIdentification));
        })
        .expect("apply identification token");
}

#[test]
fn preserves_delegation_level() {
    let _clean = ThreadTokenScope::replace(None);
    let captured = capture_at_level(SecurityDelegation);

    captured
        .with_impersonation(|| {
            assert_eq!(current_level(), Some(SecurityDelegation));
        })
        .expect("apply delegation token");
}

#[test]
fn rejects_anonymous_impersonation_synchronously() {
    let _clean = ThreadTokenScope::replace(None);
    let anonymous = duplicate_process_token(SecurityAnonymous);
    let _anonymous_scope = ThreadTokenScope::replace(Some(&anonymous));

    let error = ImpersonationToken::capture().expect_err("anonymous capture must fail");

    assert_eq!(error.failure(), CaptureFailure::AnonymousContext);
    assert_eq!(
        error.raw_os_error(),
        Some(i32::try_from(ERROR_CANT_OPEN_ANONYMOUS).expect("error code fits in i32"))
    );
}
