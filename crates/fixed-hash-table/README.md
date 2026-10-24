# fixed-hash-table

Hash tables whose capacity is fixed when they are made, and which **refuse an insert when full** rather than grow.

`std`'s `HashMap` is already open-addressed, but it reallocates when an insert outgrows it, exists only with `std`, and
seeds its default hasher from the operating system. Code that allocates everything it needs up front, and must not allocate
or panic for want of memory afterwards, needs a table that says "full" instead. This crate's tables do: an insert past
capacity hands the key and value back.

They are `no_std` and allocation-free by default, with their storage inline. A client that has an allocator can put the
storage on the heap, allocated once, when the table is made, and fallibly. The hasher is a type parameter, as
`core::hash::BuildHasher` makes it: a zero-sized one costs no storage, and one with state can differ per table.

The design is recorded in [DESIGN-NOTES.md](DESIGN-NOTES.md), and the work in [CHECKLIST.md](CHECKLIST.md).
