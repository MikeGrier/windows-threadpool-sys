// Copyright (c) Mike Grier
//! Unit tests for the self-heal registry.
//!
//! The registry is process-wide and these tests run as threads in one process,
//! so none of them asserts an absolute entry count or that a given pool is
//! absent -- a sibling test creating a pool object would make either flaky.
//! Each asserts a property of the entry it owns.

use crate::callback_env::CallbackEnviron;
use crate::pool::ThreadpoolPool;
/// Only the `self-heal` tests below create objects; the key tests need no pool
/// member, so without the feature this import would be unused.
#[cfg(feature = "self-heal")]
use crate::work::ThreadpoolWork;

#[test]
fn no_environment_means_the_default_pool() {
    assert_eq!(super::key_of(None), 0);
}

#[test]
fn an_environment_with_no_pool_means_the_default_pool() {
    let env = CallbackEnviron::new();
    assert_eq!(super::key_of(Some(&env)), 0);
}

#[test]
fn an_environment_naming_a_pool_yields_that_pool() {
    let pool = ThreadpoolPool::new().expect("create pool");
    let mut env = CallbackEnviron::new();
    env.set_pool(&pool);
    let key = super::key_of(Some(&env));
    assert_ne!(key, 0, "a named pool must not look like the default pool");
    assert_eq!(key, pool.as_raw() as usize);
}

#[cfg(feature = "self-heal")]
mod on {
    use super::*;
    use crate::heal::PoolEntry;
    use std::sync::Arc;

    /// The entry for a freshly created private pool, with one object on it.
    fn entry_for(pool: &ThreadpoolPool) -> (ThreadpoolWork, Arc<PoolEntry>) {
        let mut env = CallbackEnviron::new();
        env.set_pool(pool);
        let work = ThreadpoolWork::new(|| {}, Some(&mut env)).expect("create work");
        let entry = Arc::clone(
            crate::heal::entries()
                .iter()
                .find(|e| e.key() == pool.as_raw() as usize)
                .expect("the pool was registered when the object was created"),
        );
        (work, entry)
    }

    #[test]
    fn creating_an_object_registers_its_pool() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert_eq!(entry.key(), pool.as_raw() as usize);
    }

    #[test]
    fn the_repair_object_exists_before_it_is_needed() {
        // The load-bearing property: creating a work object was measured not to
        // release a stall, only submitting one is, so the object cannot be made
        // on the healing path.
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert_ne!(
            entry.repair_work(),
            0,
            "the entry must carry a usable repair object from registration"
        );
    }

    #[test]
    fn a_fresh_entry_owes_no_repair() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (_work, entry) = entry_for(&pool);
        assert_eq!(entry.repair_owed_at(), None);
    }

    #[test]
    fn dropping_the_last_object_retires_the_entry() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (work, _entry) = entry_for(&pool);
        drop(work);
        assert!(
            !crate::heal::entries().iter().any(|e| e.key() == key),
            "nothing is on the pool and no repair is owed, so the entry goes"
        );
    }

    #[test]
    fn the_entry_survives_while_other_objects_remain() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (first, _entry) = entry_for(&pool);
        let (second, _) = entry_for(&pool);
        drop(first);
        assert!(
            crate::heal::entries().iter().any(|e| e.key() == key),
            "one object remains, so the entry must remain"
        );
        drop(second);
        assert!(!crate::heal::entries().iter().any(|e| e.key() == key));
    }

    #[test]
    fn an_entry_owing_a_repair_outlives_its_last_object() {
        // The retention rule, and the reason it is uniform across pool kinds:
        // retiring an entry that still owes a repair would drop the repair at
        // exactly the moment it is needed.
        let pool = ThreadpoolPool::new().expect("create pool");
        let key = pool.as_raw() as usize;
        let (work, entry) = entry_for(&pool);
        entry.owe_repair(1);
        drop(work);
        assert!(
            crate::heal::entries().iter().any(|e| e.key() == key),
            "a repair is owed, so the entry is retained past its last object"
        );
        // Discharging it makes the entry retirable, which is what the self-heal
        // timer relies on.
        entry.clear_repair();
        crate::heal::retire_idle();
        assert!(!crate::heal::entries().iter().any(|e| e.key() == key));
    }

    #[test]
    fn the_first_cancellation_is_the_one_remembered() {
        // Overwriting with a later stamp could make a dispatch that genuinely
        // followed the first cancellation look as though it preceded the second,
        // which would suppress a repair that is still owed.
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.owe_repair(10);
        entry.owe_repair(20);
        assert_eq!(entry.repair_owed_at(), Some(10));
        entry.clear_repair();
        drop(work);
    }

    #[test]
    fn a_dispatch_after_the_cancellation_is_what_counts() {
        let pool = ThreadpoolPool::new().expect("create pool");
        let (work, entry) = entry_for(&pool);
        entry.stamp_dispatch(5);
        entry.owe_repair(10);
        assert!(
            !entry.dispatched_since(10),
            "a dispatch before the cancellation is no evidence the pool is live"
        );
        entry.stamp_dispatch(11);
        assert!(entry.dispatched_since(10));
        entry.clear_repair();
        drop(work);
    }

    #[test]
    fn two_pools_get_two_entries() {
        let first = ThreadpoolPool::new().expect("create pool");
        let second = ThreadpoolPool::new().expect("create pool");
        let (_w1, e1) = entry_for(&first);
        let (_w2, e2) = entry_for(&second);
        assert_ne!(e1.key(), e2.key());
        assert_ne!(
            e1.repair_work(),
            e2.repair_work(),
            "each pool needs its own repair object: a repair is a submit to one pool"
        );
    }

    #[test]
    fn objects_on_the_default_pool_share_one_entry() {
        let a = ThreadpoolWork::new(|| {}, None).expect("create work");
        let b = ThreadpoolWork::new(|| {}, None).expect("create work");
        let count = crate::heal::entries()
            .iter()
            .filter(|e| e.key() == 0)
            .count();
        assert_eq!(count, 1, "the default pool is one pool and gets one entry");
        drop(a);
        drop(b);
    }
}
