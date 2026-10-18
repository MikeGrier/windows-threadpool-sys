// Copyright (c) 2026 Mike Grier
#![cfg(windows)]
#![doc = include_str!("../README.md")]

mod security;

pub use security::{
    AclState, SecurityAttributes, SecurityCaptureError, SecurityCaptureFailure, SecurityDescriptor,
    SecurityInformation,
};
