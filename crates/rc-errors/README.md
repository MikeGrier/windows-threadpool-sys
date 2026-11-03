# rc-errors

A shared, reference-counted error for `no_std` code that must not depend on the global allocator.

`alloc::sync::Arc<dyn Error>` is the usual way to share an error between the places that report it. On stable Rust its
allocation and its deallocation always go through the global allocator, its fallible constructors are unstable, and a
library cannot require the global allocator to be replaced. Code that allocates everything it needs up front, and must not
panic on an allocation failure afterwards, has no way to share an error with it.

This crate's shared error has one reference count and no weak references, as Linux's `kernel::sync::Arc` has none. The
allocator that made it is recorded beside it, so the last reference, dropped on any thread, frees it through that allocator,
and the type carries no allocator parameter. Making one is fallible and never panics: a failed allocation hands the error
back.

Beside it is a static error, made in a `const`, for a report that needs no storage at all, and a report that is either, so
code that reports both kinds names one type.

The allocator interface is the smallest part of the standard library's unstable `Allocator` trait, with its names and
signatures, so that moving to the stabilised trait is a change of import.

## Reporting both kinds of error

A static error is made in a `static`, and a report of it allocates nothing. A shared error is made from any error type, and
a failed allocation hands the error back rather than panicking. Both become an `ErrorReport`, which reads as the error it
holds and compares by identity:

```rust
use rc_errors::{ErrorReport, SharedError, StaticError};

static AT_CAPACITY: StaticError = StaticError::new("the ring is at capacity");

fn refuse() -> ErrorReport {
    ErrorReport::from_static(&AT_CAPACITY)
}

fn fail(cause: std::io::Error) -> Result<ErrorReport, std::io::Error> {
    SharedError::try_new(cause)
        .map(ErrorReport::from)
        .map_err(|(cause, _)| cause)
}

let refused = refuse();
assert_eq!(refused.to_string(), "the ring is at capacity");
assert_eq!(refused, refuse(), "one static, one identity");

let failed = fail(std::io::Error::other("the device is gone")).expect("allocated");
let seen_elsewhere = failed.clone();
assert_eq!(failed, seen_elsewhere, "clones share one block");
assert!(seen_elsewhere.downcast_ref::<std::io::Error>().is_some());
assert_ne!(failed, refused);
```

## Choosing the allocator

`SharedError::try_new_in` takes any `&'static (dyn Allocator + Sync)`, and the last clone dropped frees the block through
it, on whatever thread that is. `Allocator` mirrors the standard library's unstable trait:

```rust
use core::alloc::Layout;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicUsize, Ordering};
use rc_errors::{AllocError, Allocator, Global, SharedError, StaticError};

/// Counts the blocks it has handed out and not yet had back.
struct Counting(AtomicUsize);

// SAFETY: every block is `Global`'s, and goes back to `Global` with its layout.
unsafe impl Allocator for Counting {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        let block = Global.allocate(layout)?;
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(block)
    }

    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        self.0.fetch_sub(1, Ordering::Relaxed);
        // SAFETY: the caller's promise is passed on to `Global`, which made the block.
        unsafe { Global.deallocate(ptr, layout) };
    }
}

static POOL: Counting = Counting(AtomicUsize::new(0));

let error = SharedError::try_new_in(StaticError::new("a fault"), &POOL).expect("allocated");
let clone = error.clone();
assert_eq!(POOL.0.load(Ordering::Relaxed), 1);
drop(error);
drop(clone);
assert_eq!(POOL.0.load(Ordering::Relaxed), 0);
```

The design is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the work in [CHECKLIST.md](CHECKLIST.md).
