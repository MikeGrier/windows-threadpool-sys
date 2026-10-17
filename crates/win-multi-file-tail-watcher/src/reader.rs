// Copyright (c) 2026 Mike Grier
//! The reading layer's sizes, and why one of them is refused.
//!
//! The caller chooses every size the reading layer works to, in a [`TailReaderOptions`], and
//! [`TailReaderOptions::validate`] refuses a size that cannot work before anything is allocated. A
//! refusal is an [`OptionsError`] that names the option, the value given and the [`Cause`].
//!
//! What the layer delivers is a [`ReadEntry`] on a [`ReadSq`] the client drains: [`Data`] entries
//! carry a [`Chunk`] of the bytes read, with the range of the file they are from, and a client
//! that keeps a chunk keeps its buffer out of the layer's pool.

mod options;
mod options_error;

pub use options::TailReaderOptions;
pub use options_error::{Cause, OptionsError};

pub use crate::pool::Chunk;
pub use crate::read_sq::{Data, ReadDrain, ReadEntry, ReadSq, Shrink};
