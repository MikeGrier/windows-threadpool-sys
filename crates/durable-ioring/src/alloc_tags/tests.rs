// Copyright (c) 2026 Mike Grier
//! The tag a call runs under, and the one it leaves behind.

use super::{Tag, tagged};

const OUTER: Tag = Tag::new("alloc_tags::tests::outer");
const INNER: Tag = Tag::new("alloc_tags::tests::inner");

#[test]
fn a_tagged_call_returns_what_its_closure_returns() {
    assert_eq!(tagged(OUTER, || 7), 7);
    let mut grown = Vec::new();
    tagged(OUTER, || grown.push(1));
    assert_eq!(grown, [1]);
}

#[test]
fn a_tag_has_its_name() {
    assert_eq!(OUTER.name(), "alloc_tags::tests::outer");
    assert_ne!(OUTER, INNER);
}

#[cfg(feature = "alloc-tags")]
mod current {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::super::current;
    use super::{INNER, OUTER, tagged};

    #[test]
    fn the_tag_is_set_for_the_call_and_cleared_after_it() {
        assert!(current().is_none());
        let seen = tagged(OUTER, || current().map(|site| site.tag));
        assert_eq!(seen, Some(OUTER));
        assert!(current().is_none());
    }

    #[test]
    fn a_nested_call_sets_its_own_tag_and_restores_the_outer_one() {
        let seen = tagged(OUTER, || {
            let inner = tagged(INNER, || current().map(|site| site.tag));
            (inner, current().map(|site| site.tag))
        });
        assert_eq!(seen, (Some(INNER), Some(OUTER)));
        assert!(current().is_none());
    }

    #[test]
    fn the_location_is_the_tagged_call_s() {
        let line = line!() + 1;
        let site = tagged(OUTER, current).expect("set");
        assert_eq!(site.location.file(), file!());
        assert_eq!(site.location.line(), line);
    }

    #[test]
    fn a_call_that_unwinds_restores_the_outer_tag() {
        let seen = tagged(OUTER, || {
            let unwound = catch_unwind(AssertUnwindSafe(|| tagged(INNER, || panic!("inside"))));
            assert!(unwound.is_err());
            current().map(|site| site.tag)
        });
        assert_eq!(seen, Some(OUTER));
        assert!(current().is_none());
    }
}
