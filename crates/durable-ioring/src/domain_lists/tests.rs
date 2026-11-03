// Copyright (c) 2026 Mike Grier
//! The flush-domain lists on their own: sorted and without repeats, an empty list holding no row,
//! the rows refused once all are held, a released row taken again, and a list wider than a row
//! refused with its row handed back.

use super::{DomainList, DomainLists};
use crate::domains::DomainId;

fn ids(values: &[u32]) -> Vec<DomainId> {
    values.iter().map(|&value| DomainId(value)).collect()
}

#[test]
fn a_list_is_sorted_and_names_each_domain_once() {
    let mut lists = DomainLists::new(Some(2), 4).expect("small");
    let list = lists.claim(ids(&[3, 1, 3, 2, 1])).expect("a free row");
    assert_eq!(lists.get(list), ids(&[1, 2, 3]).as_slice());
    let other = lists.claim(ids(&[9])).expect("the other row");
    assert_eq!(lists.get(other), ids(&[9]).as_slice());
    assert_eq!(lists.get(list), ids(&[1, 2, 3]).as_slice(), "untouched");
}

#[test]
fn an_empty_list_holds_no_row() {
    let mut lists = DomainLists::new(Some(1), 2).expect("small");
    let empty = lists.claim(ids(&[])).expect("no row needed");
    assert_eq!(empty, DomainList::EMPTY);
    assert!(lists.get(empty).is_empty());
    let held = lists.claim(ids(&[5])).expect("the one row is still free");
    lists.release(empty);
    assert_eq!(
        lists.claim(ids(&[6])),
        Err("no flush-domain list was free"),
        "releasing the empty list freed nothing"
    );
    lists.release(held);
    assert!(lists.claim(ids(&[6])).is_ok());
}

#[test]
fn a_released_row_is_taken_again_and_holds_the_new_list() {
    let mut lists = DomainLists::new(Some(1), 3).expect("small");
    for round in 0..5_u32 {
        let list = lists
            .claim(ids(&[round, round + 1]))
            .expect("the row was let go");
        assert_eq!(lists.get(list), ids(&[round, round + 1]).as_slice());
        assert!(lists.is_held(list));
        lists.release(list);
        assert!(!lists.is_held(list));
    }
}

#[test]
fn a_list_wider_than_a_row_is_refused_and_hands_its_row_back() {
    let mut lists = DomainLists::new(Some(1), 2).expect("small");
    assert_eq!(
        lists.claim(ids(&[1, 2, 3])),
        Err("a flush-domain list named more domains than were declared")
    );
    let list = lists.claim(ids(&[2, 2, 1, 1])).expect("the row came back");
    assert_eq!(
        lists.get(list),
        ids(&[1, 2]).as_slice(),
        "repeats are not wider"
    );
}

#[test]
fn no_rows_or_no_width_hold_only_the_empty_list() {
    let mut none = DomainLists::new(Some(0), 4).expect("empty");
    assert_eq!(none.claim(ids(&[])), Ok(DomainList::EMPTY));
    assert!(none.claim(ids(&[1])).is_err());
    let mut narrow = DomainLists::new(Some(2), 0).expect("no width");
    assert!(narrow.claim(ids(&[1])).is_err());
    assert!(
        narrow.claim(ids(&[1])).is_err(),
        "the row was handed back each time"
    );
}

#[test]
fn a_size_past_a_usize_is_refused() {
    assert!(DomainLists::new(None, 1).is_err());
    assert!(DomainLists::new(Some(usize::MAX), 2).is_err());
}
