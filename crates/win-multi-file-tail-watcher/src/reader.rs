// Copyright (c) 2026 Mike Grier
//! The reading layer's sizes, and why one of them is refused.
//!
//! The caller chooses every size the reading layer works to, in a [`TailReaderOptions`], and
//! [`TailReaderOptions::validate`] refuses a size that cannot work before anything is allocated. A
//! refusal is an [`OptionsError`] that names the option, the value given and the [`Cause`].

mod options;
mod options_error;

pub use options::TailReaderOptions;
pub use options_error::{Cause, OptionsError};
