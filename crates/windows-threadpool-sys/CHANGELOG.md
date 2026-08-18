# Changelog

All notable changes to this project will be documented in this file.

## [1.0.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-threadpool-sys-v0.1.0...windows-threadpool-sys-v1.0.0) (2026-08-18)


### ⚠ BREAKING CHANGES

* take a closed CallbackPriority enum, not the open Win32 alias
* CompletionPort::outstanding no longer counts an operation whose completion packet has been dequeued but whose Completion is still held, and AssociatedEndpoint::cancel now reports NotFound for such an operation. Both follow from registration ending at dequeue; cancelling an operation whose packet has arrived was always meaningless.
* set_min_threads now returns InvalidInput when the requested minimum exceeds a maximum previously set on the same pool, and set_max_threads likewise rejects a maximum below a previously set minimum. Both previously succeeded and let Win32 silently annul one of the two limits.
* OperationId::from_parts is removed; use the OperationId returned by OperationRegistry::remove or identify, or the unsafe OperationId::forge. OperationRegistry::generation_of is renamed to identify and returns Option<OperationId>; remove now returns Option<OperationId>.
* ThreadpoolPool::set_max_threads returns io::Result<()> and rejects a maximum of zero. A pool with a maximum of zero never runs a callback, so no working configuration is affected.
* ThreadpoolPeriodicTimer::new and CleanupGroup::create_periodic_timer now reject a period that is not a whole number of milliseconds, or that exceeds u32::MAX milliseconds. Neither was honoured as given, so no working configuration is affected.
* ThreadpoolPeriodicTimer::new and CleanupGroup::create_periodic_timer now reject any period shorter than one millisecond, not only zero. Such periods never repeated, so no working configuration is affected.
* ThreadpoolWait::new and CleanupGroup::create_wait take a WaitableHandle instead of an OwnedHandle. Construct one with WaitableHandle::event, or with the unsafe assume_waitable for a handle obtained elsewhere.
* OperationId carries a generation in addition to the OVERLAPPED address, so it is no longer a bare pointer wrapper. OperationId::from_ptr is replaced by OperationId::mint and OperationId::from_parts, and Completion::id returns Option<OperationId>. AssociatedEndpoint::cancel now rejects an identity that no longer names a live operation with ErrorKind::NotFound rather than passing the address to CancelIoEx.

### Features

* add stop_and_drain to the timer and the wait ([abca56d](https://github.com/MikeGrier/windows-threadpool-sys/commit/abca56d76ac3bb7d17f67715ffb85393dd3a082e))
* document and expose the wait re-arm overlap, adding WaitActivation::handle ([85adaa8](https://github.com/MikeGrier/windows-threadpool-sys/commit/85adaa8b96785292e60600208e657baa034d9a8f))
* thread-pool objects and generation-stamped operation identities ([99d12c4](https://github.com/MikeGrier/windows-threadpool-sys/commit/99d12c4d2e487b92a60d1d3a43d0c42b62b3e665))


### Bug Fixes

* deregister an operation when its completion packet is dequeued ([bf1a09f](https://github.com/MikeGrier/windows-threadpool-sys/commit/bf1a09f429784d710bfa2830ba681de1d9f0f216))
* enforce wait provenance and gate re-arming against teardown ([46ef87c](https://github.com/MikeGrier/windows-threadpool-sys/commit/46ef87c50e36f67db31a72631a487ca2906c1b81))
* gate windows-threadpool-sys behind cfg(windows) ([bec730f](https://github.com/MikeGrier/windows-threadpool-sys/commit/bec730f5e507f6fe50a5c7c0a58c6cb8a5d105d4))
* make an operation identity unforgeable by safe code ([3e0e244](https://github.com/MikeGrier/windows-threadpool-sys/commit/3e0e244afaebc67e1f88aefefc2ff18d0be9a889))
* name the LongFunction flag bit and record the scatter/gather limit finding ([020592b](https://github.com/MikeGrier/windows-threadpool-sys/commit/020592b1ede91c46c82e0489a4c618ee0fa750be))
* refuse contradictory thread limits and detect glued doc comments ([22dcff3](https://github.com/MikeGrier/windows-threadpool-sys/commit/22dcff34a4a6546f76d9318df6e878dd8014aaa1))
* reject a zero thread maximum and correct what the maximum does ([0827237](https://github.com/MikeGrier/windows-threadpool-sys/commit/082723703c598fa21719145b013dfb3ff99d7f1e))
* reject oversized socket lengths and check file reads before allocating ([99681e4](https://github.com/MikeGrier/windows-threadpool-sys/commit/99681e461600aca29a247bf92d81b9316b36b463))
* reject periodic periods that are not whole, representable milliseconds ([b7c1110](https://github.com/MikeGrier/windows-threadpool-sys/commit/b7c1110691bf8d28a256c7c340cbf4a402d5da9e))
* reject periodic timer periods below one millisecond ([eab3a13](https://github.com/MikeGrier/windows-threadpool-sys/commit/eab3a13fd5e0763322a156cd74faca5e0c945958))
* release cleanup group members created after an earlier release ([7217377](https://github.com/MikeGrier/windows-threadpool-sys/commit/721737720b7c18d13e6c73f8a137f5769aa2dba1))
* remove committed form feeds and detect control characters in CI ([29f9d89](https://github.com/MikeGrier/windows-threadpool-sys/commit/29f9d89f020403095179f19ca13f87112e4b0d73))
* suppress member re-arm before a cleanup group's bulk release ([2c94d0f](https://github.com/MikeGrier/windows-threadpool-sys/commit/2c94d0f73857e9a84abc5bff6c60a2432a577d5f))
* take a closed CallbackPriority enum, not the open Win32 alias ([0ce7e95](https://github.com/MikeGrier/windows-threadpool-sys/commit/0ce7e95d3c30ca59a6aa6ea2ec4566c217ae0d05))
* **test:** remove the identity tests' global hook mutation and false-pass mode ([d50e064](https://github.com/MikeGrier/windows-threadpool-sys/commit/d50e0649a269a4a5a5a5213a93011495af0e8f58))
* **test:** stop a stress scenario asserting non-overlap it does not serialize ([96420c4](https://github.com/MikeGrier/windows-threadpool-sys/commit/96420c4d365c1913eb7cc1bfe8bc53e7d523062f))

## [0.1.0] - 2026-08-16

- Specialized the repository and package metadata for `windows-threadpool-sys`.
- Established the initial crate and documentation for a memory-safe Windows
	thread pool API.
