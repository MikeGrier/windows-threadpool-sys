// Copyright (c) Mike Grier.

//! The three-state value every capturable aspect carries.

/// What a capture produced for one aspect.
///
/// # Why this is not `Option`
///
/// *Not captured* and *captured, and the thread had none* are different facts
/// with the same observable outcome, and only one of them is a decision.
///
/// Take impersonation. If the aspect was left out of the capture set, the worker
/// runs under the process identity. If it was captured and the calling thread
/// had no token, the worker also runs under the process identity. A caller
/// reading back an `Option::None` cannot tell which happened -- so an omission
/// becomes indistinguishable from a deliberate statement about what the work
/// should run as, and nobody can later reconstruct which one it was.
///
/// The shape is uniform across aspects even where [`Absent`](Self::Absent) is
/// unreachable, because a per-aspect shape would make every consumer remember
/// which aspects can be absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Captured<T> {
    /// The aspect was not in the capture set, so nothing was read and the
    /// target thread's own value is left alone.
    NotCaptured,
    /// The aspect was captured, and the calling thread had no value for it.
    Absent,
    /// The aspect was captured.
    Present(T),
}

impl<T> Captured<T> {
    /// Whether capture was attempted at all.
    ///
    /// True for both [`Absent`](Self::Absent) and [`Present`](Self::Present):
    /// the question is whether the caller asked, not what the answer was.
    #[must_use]
    pub const fn was_captured(&self) -> bool {
        !matches!(self, Self::NotCaptured)
    }

    /// The captured value, if there is one.
    #[must_use]
    pub const fn present(&self) -> Option<&T> {
        match self {
            Self::Present(value) => Some(value),
            Self::NotCaptured | Self::Absent => None,
        }
    }

    /// Borrow the contents.
    #[must_use]
    pub const fn as_ref(&self) -> Captured<&T> {
        match self {
            Self::NotCaptured => Captured::NotCaptured,
            Self::Absent => Captured::Absent,
            Self::Present(value) => Captured::Present(value),
        }
    }

    /// Transform a present value, preserving which of the other two states it
    /// was otherwise.
    #[must_use]
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> Captured<U> {
        match self {
            Self::NotCaptured => Captured::NotCaptured,
            Self::Absent => Captured::Absent,
            Self::Present(value) => Captured::Present(f(value)),
        }
    }
}

#[cfg(test)]
mod tests;
