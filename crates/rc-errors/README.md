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

The design is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the work in [CHECKLIST.md](CHECKLIST.md).
