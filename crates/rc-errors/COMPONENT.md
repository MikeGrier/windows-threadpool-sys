# Component: rc-errors

A shared, reference-counted error for `no_std` code that must not depend on the global allocator; see
[README.md](README.md). It holds the `unsafe` that durable-ioring's fault reports need, since durable-ioring forbids it
([DI-D-105](../durable-ioring/DESIGN-NOTES.md#di-d-105)). Decisions: [DESIGN-NOTES.md](DESIGN-NOTES.md). Work:
[CHECKLIST.md](CHECKLIST.md), [PARKED.md](PARKED.md), tracked in [PLANS.md](PLANS.md).
