// Copyright (c) 2026 Mike Grier
//! A created activity id: never zero, never the same twice, from one thread or several.

use std::collections::HashSet;
use std::sync::Mutex;
use std::thread;

use super::create_activity_id;
use crate::Guid;

const ZERO: Guid = Guid::from_u128(0);

#[test]
fn an_id_is_created() {
    assert!(create_activity_id().is_ok());
}

#[test]
fn a_created_id_is_not_zero_which_the_page_says_is_no_activity() {
    assert_ne!(create_activity_id().expect("created"), ZERO);
}

#[test]
fn two_ids_in_a_row_differ() {
    let first = create_activity_id().expect("created");
    let second = create_activity_id().expect("created");
    assert_ne!(first, second);
}

#[test]
fn a_thousand_ids_are_all_different() {
    let ids: HashSet<Guid> = (0..1000)
        .map(|_| create_activity_id().expect("created"))
        .collect();
    assert_eq!(ids.len(), 1000);
}

#[test]
fn ids_created_on_several_threads_are_all_different() {
    let ids = Mutex::new(HashSet::new());
    thread::scope(|scope| {
        for _ in 0..8 {
            let ids = &ids;
            scope.spawn(move || {
                for _ in 0..100 {
                    let id = create_activity_id().expect("created");
                    assert!(ids.lock().expect("the set").insert(id), "{id} twice");
                }
            });
        }
    });
    assert_eq!(ids.lock().expect("the set").len(), 800);
}
