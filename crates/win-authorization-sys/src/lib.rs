// Copyright (c) 2026 Mike Grier
#![cfg(windows)]
#![doc = include_str!("../README.md")]

mod security;

pub use security::{
    AclState, ObjectType, SecurityAttributes, SecurityCaptureError, SecurityCaptureFailure,
    SecurityDescriptor, SecurityInformation, get_security_info, get_security_info_unchecked,
};
