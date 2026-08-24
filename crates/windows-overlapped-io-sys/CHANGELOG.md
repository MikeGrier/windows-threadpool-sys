# Changelog

Both 0.1.0 and 1.0.0 were yanked from crates.io: the 1.0.0 bump was
accidental, and this crate is intended to stay pre-1.0 for now. Releases
resume at 0.1.1, which supersedes both.

## [0.1.2](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-overlapped-io-sys-v0.1.1...windows-overlapped-io-sys-v0.1.2) (2026-08-24)


### Bug Fixes

* **build:** use table headers for windows-sys so release-please can parse the manifests ([76a0d53](https://github.com/MikeGrier/windows-threadpool-sys/commit/76a0d53f3b06db73d6a2567933f66bcd4edac260))

## [1.0.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/windows-overlapped-io-sys-v0.1.0...windows-overlapped-io-sys-v1.0.0) (2026-08-18)


### ⚠ BREAKING CHANGES

* make the generic DeviceIoControl ioctl unsafe (arbitrary control code)
* CompletionPort::outstanding no longer counts an operation whose completion packet has been dequeued but whose Completion is still held, and AssociatedEndpoint::cancel now reports NotFound for such an operation. Both follow from registration ending at dequeue; cancelling an operation whose packet has arrived was always meaningless.
* OperationId::from_parts is removed; use the OperationId returned by OperationRegistry::remove or identify, or the unsafe OperationId::forge. OperationRegistry::generation_of is renamed to identify and returns Option<OperationId>; remove now returns Option<OperationId>.
* ThreadpoolPool::set_max_threads returns io::Result<()> and rejects a maximum of zero. A pool with a maximum of zero never runs a callback, so no working configuration is affected.
* BlockingEndpoint::read, write, read_scatter, write_gather and ioctl now take &mut self. Calling them on a shared endpoint was already unsound, so no correct usage is affected.
* OperationId carries a generation in addition to the OVERLAPPED address, so it is no longer a bare pointer wrapper. OperationId::from_ptr is replaced by OperationId::mint and OperationId::from_parts, and Completion::id returns Option<OperationId>. AssociatedEndpoint::cancel now rejects an identity that no longer names a live operation with ErrorKind::NotFound rather than passing the address to CancelIoEx.

### Features

* **overlapped-io:** add blocking GetOverlappedResult backend ([f5331a1](https://github.com/MikeGrier/windows-threadpool-sys/commit/f5331a1294a49da1b2baf2049e00bbafe3e3641d))
* **overlapped-io:** add CancelIoEx cancellation and a real-device pipe test ([079d5bf](https://github.com/MikeGrier/windows-threadpool-sys/commit/079d5bf81d77373876c24ea22637a5423e3013aa))
* **overlapped-io:** add pinned per-operation storage with OVERLAPPED identity ([51432cb](https://github.com/MikeGrier/windows-threadpool-sys/commit/51432cbbffa57b7c5a0aa0279fd59217e16146dc))
* **overlapped-io:** add raw IOCP backend with port ownership and association ([a844d21](https://github.com/MikeGrier/windows-threadpool-sys/commit/a844d213b1ffa3668ee2b99818164f306be4903e))
* **overlapped-io:** add unassociated endpoint ownership with provenance seam ([23cb715](https://github.com/MikeGrier/windows-threadpool-sys/commit/23cb7153ea5050dc601cb466c53f4c67e021460a))
* **overlapped-io:** blocking rundown with per-operation source tracking ([14adcb0](https://github.com/MikeGrier/windows-threadpool-sys/commit/14adcb02416fee43128ac72af2c3fca81c6d13b0))
* **overlapped-io:** expose the TP_IO submission seam ([a16e554](https://github.com/MikeGrier/windows-threadpool-sys/commit/a16e554d970850559f6995e6ee5049a2abaef2f6))
* **overlapped-io:** lock-free rundown with opt-in source tracking ([444367b](https://github.com/MikeGrier/windows-threadpool-sys/commit/444367be4139ac9bf9ea28eda2613c284a37d284))
* **overlapped-io:** prototype owned-operation submission and completion claim ([7bad8b3](https://github.com/MikeGrier/windows-threadpool-sys/commit/7bad8b3d4a53cb6971387672d12add5ccc21f2d4))
* thread-pool objects and generation-stamped operation identities ([99d12c4](https://github.com/MikeGrier/windows-threadpool-sys/commit/99d12c4d2e487b92a60d1d3a43d0c42b62b3e665))


### Bug Fixes

* bound rundown waits so a concurrent drain cannot hang teardown ([f70ec05](https://github.com/MikeGrier/windows-threadpool-sys/commit/f70ec058ed0e4f0a4b2a24f9735b3498aeb6c7c1))
* close the wrap window in the operation-generation sequence ([aeec77d](https://github.com/MikeGrier/windows-threadpool-sys/commit/aeec77dde944da3a9d1cea84c8f482c9ffc5df57))
* deregister an operation when its completion packet is dequeued ([bf1a09f](https://github.com/MikeGrier/windows-threadpool-sys/commit/bf1a09f429784d710bfa2830ba681de1d9f0f216))
* gate fs-only BlockingEndpoint doctests and cover the default config in CI ([09d8f61](https://github.com/MikeGrier/windows-threadpool-sys/commit/09d8f61944f884e284c051beda7c54b4f7fe4ccf))
* make an operation identity unforgeable by safe code ([3e0e244](https://github.com/MikeGrier/windows-threadpool-sys/commit/3e0e244afaebc67e1f88aefefc2ff18d0be9a889))
* make the generic DeviceIoControl ioctl unsafe (arbitrary control code) ([cbd4cc1](https://github.com/MikeGrier/windows-threadpool-sys/commit/cbd4cc135494025e8c91ccee21f2ab8715bcbefd))
* refuse to wrap the operation-generation sequence ([2be554a](https://github.com/MikeGrier/windows-threadpool-sys/commit/2be554ae5d1416ec90f8a7b8e6a84ce4a834ff06))
* reject a zero page count in scatter reads instead of panicking ([ceb8022](https://github.com/MikeGrier/windows-threadpool-sys/commit/ceb8022ca9924ecf7031c2201c5c57e59008a1c1))
* reject a zero thread maximum and correct what the maximum does ([0827237](https://github.com/MikeGrier/windows-threadpool-sys/commit/082723703c598fa21719145b013dfb3ff99d7f1e))
* reject file and scatter/gather lengths too large for the Win32 field ([4645230](https://github.com/MikeGrier/windows-threadpool-sys/commit/4645230d347eb5fd72e72971916e40a94b894d75))
* reject ioctl buffers too large for the Win32 length field ([2ba79a7](https://github.com/MikeGrier/windows-threadpool-sys/commit/2ba79a785669486ff2cf6f3ee6a72fb311877bee))
* reject oversized socket lengths and check file reads before allocating ([99681e4](https://github.com/MikeGrier/windows-threadpool-sys/commit/99681e461600aca29a247bf92d81b9316b36b463))
* remove committed form feeds and detect control characters in CI ([29f9d89](https://github.com/MikeGrier/windows-threadpool-sys/commit/29f9d89f020403095179f19ca13f87112e4b0d73))
* require exclusive access for the safe blocking adapters ([77876c0](https://github.com/MikeGrier/windows-threadpool-sys/commit/77876c0fb004f9ace1c089c899983c7975129f14))
* **test:** remove the identity tests' global hook mutation and false-pass mode ([d50e064](https://github.com/MikeGrier/windows-threadpool-sys/commit/d50e0649a269a4a5a5a5213a93011495af0e8f58))

## Changelog

All notable changes to this project will be documented in this file.
