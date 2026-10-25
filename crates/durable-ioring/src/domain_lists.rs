// Copyright (c) 2026 Mike Grier
//! The flush-domain lists a target is declared with and an import by domains names (DI-D-108):
//! rows of one table made when the instance is built, so declaring a target or importing a failure
//! allocates nothing, and a list is a row and a length, copied wherever a write, a span, a kept
//! record or a failure's reach names it.
//!
//! There are `targets` plus `imports` rows, each as wide as the domains declared, since one list
//! names each domain at most once. A target holds its row from `add_target` to `remove_target`, an
//! import by domains from its observation to its resolution, and an empty list holds none.
//!
//! **A row is not reused while anything names it.** A target is removed only when nothing holds
//! it: no write to it in flight, held, uncovered or kept, and no unresolved failure suspecting a
//! write to it (DI-D-45). A failed flush of a file reaches by that file's row, and the failure
//! suspects the write the flush answered for (DI-D-109), which holds the file until the failure is
//! resolved. An import's row is the failure's own, let go once it is resolved.

use alloc::vec::Vec;

use crate::capacity::{ConstructionError, list};
use crate::domains::DomainId;

#[cfg(test)]
mod tests;

/// A list of interned domains, sorted and without repeats, held in a row of [`DomainLists`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DomainList {
    row: u32,
    len: u32,
}

impl DomainList {
    /// The list naming no domain, which holds no row.
    pub(crate) const EMPTY: Self = Self { row: 0, len: 0 };

    /// Whether it names no domain.
    pub(crate) fn is_empty(self) -> bool {
        self.len == 0
    }
}

/// The rows the domain lists are held in, and which of them are free.
pub(crate) struct DomainLists {
    ids: Vec<DomainId>,
    width: usize,
    free: Vec<u32>,
}

impl DomainLists {
    /// `rows` rows of `width` domains each; `None` is a count past a `usize`.
    ///
    /// # Errors
    ///
    /// [`ConstructionError::Table`] when the rows cannot be made.
    pub(crate) fn new(rows: Option<usize>, width: usize) -> Result<Self, ConstructionError> {
        let cells = rows.and_then(|rows| rows.checked_mul(width));
        let mut ids = list("the flush-domain lists", cells)?;
        ids.resize(cells.unwrap_or(0), DomainId(0));
        let rows = rows.unwrap_or(0);
        let mut free = list("the free flush-domain lists", Some(rows))?;
        free.extend((0..rows).rev().filter_map(|row| u32::try_from(row).ok()));
        Ok(Self { ids, width, free })
    }

    /// The domains `list` names, sorted.
    pub(crate) fn get(&self, list: DomainList) -> &[DomainId] {
        if list.is_empty() {
            return &[];
        }
        let start = list.row as usize * self.width;
        self.ids
            .get(start..start + list.len as usize)
            .unwrap_or(&[])
    }

    /// A list of `ids`, sorted and without repeats, in a row of its own; no row for none.
    ///
    /// # Errors
    ///
    /// What is inconsistent: no row is free, or `ids` name more distinct domains than a row holds.
    /// Neither happens within the capacity the instance was built for.
    pub(crate) fn claim(
        &mut self,
        ids: impl IntoIterator<Item = DomainId>,
    ) -> Result<DomainList, &'static str> {
        let mut ids = ids.into_iter().peekable();
        if ids.peek().is_none() {
            return Ok(DomainList::EMPTY);
        }
        let row = self.free.pop().ok_or("no flush-domain list was free")?;
        let start = row as usize * self.width;
        let cells = &mut self.ids[start..start + self.width];
        let mut len = 0;
        for id in ids {
            match cells[..len].binary_search(&id) {
                Ok(_) => {}
                Err(_) if len == cells.len() => {
                    self.free.push(row);
                    return Err("a flush-domain list named more domains than were declared");
                }
                Err(at) => {
                    cells.copy_within(at..len, at + 1);
                    cells[at] = id;
                    len += 1;
                }
            }
        }
        Ok(DomainList {
            row,
            len: u32::try_from(len).unwrap_or(u32::MAX),
        })
    }

    /// Let go of `list`'s row: nothing names it any more.
    pub(crate) fn release(&mut self, list: DomainList) {
        if !list.is_empty() {
            self.free.push(list.row);
        }
    }

    /// Whether `list`'s row is held, as anything naming it requires.
    #[cfg(test)]
    pub(crate) fn is_held(&self, list: DomainList) -> bool {
        list.is_empty() || !self.free.contains(&list.row)
    }

    /// A list naming `ids`: the row of a held list naming exactly those, or a new one. For tests
    /// that push writes to a file without declaring a target, so each file keeps one row.
    ///
    /// Only rows this has handed out are searched, so a row a target or an import holds is never
    /// shared by accident.
    #[cfg(test)]
    pub(crate) fn shared(
        &mut self,
        given: &mut std::vec::Vec<DomainList>,
        ids: &[DomainId],
    ) -> DomainList {
        let mut sorted = ids.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        if let Some(&list) = given
            .iter()
            .find(|&&list| self.get(list) == sorted.as_slice())
        {
            return list;
        }
        let list = self.claim(sorted).expect("a free row");
        given.push(list);
        list
    }
}
