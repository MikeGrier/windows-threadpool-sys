# Changelog

## [0.3.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.2.0...durable-ioring-v0.3.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.1.0...durable-ioring-v0.2.0) (2026-10-16)


### ⚠ BREAKING CHANGES

* **durable-ioring:** ErrorCode::from_win32 and ErrorCode::win32 are removed.

### Features

* **durable-ioring:** ErrorCode no longer wraps or reads Win32 codes ([5291eaa](https://github.com/MikeGrier/windows-threadpool-sys/commit/5291eaacaafa42294559621694efd0a995c48948))

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-v0.0.1...durable-ioring-v0.1.0) (2026-10-15)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring no longer contains Dioring, the Windows backend, delivery, the readiness check or the Windows re-exports; they are win-durable-ioring's. ErrorCode::of, the default DioringIds time type and the TimeBase bound on a win-time-sys clock are replaced as described above.
* **durable-ioring:** add pumped delivery, the consumer's own thread waiting on the backend's notification and draining

### Features

* **durable-ioring:** add pumped delivery, the consumer's own thread waiting on the backend's notification and draining ([d3832e7](https://github.com/MikeGrier/windows-threadpool-sys/commit/d3832e742df0e7c5161ff6141bffb9b60bb9b874))
* **durable-ioring:** add signalled delivery, the pool telling the consumer through an event while the consumer drains ([6691c23](https://github.com/MikeGrier/windows-threadpool-sys/commit/6691c23018d48c8cb02a1490a9ab0f8f1c8d056f))


### Bug Fixes

* **durable-ioring:** the engine rings the doorbell, and tells the consumer once when it fails ([dfebddf](https://github.com/MikeGrier/windows-threadpool-sys/commit/dfebddf7045d3cc00e9016e58a709f168df2f156))
* **durable-ioring:** the in-memory backend refuses a write a Vec cannot hold, reports one it cannot grow for, and keeps its registered buffers ([b56236d](https://github.com/MikeGrier/windows-threadpool-sys/commit/b56236deae38a9472304eccdf8586a8c87411b02))
* **win-durable-ioring:** end and retire refuse a lineage handle the instance did not mint ([00fb4a4](https://github.com/MikeGrier/windows-threadpool-sys/commit/00fb4a4bde839651646eeb31c177f465076c9a73))


### Code Refactoring

* **durable-ioring:** split the Windows driver out into win-durable-ioring ([e8ff929](https://github.com/MikeGrier/windows-threadpool-sys/commit/e8ff92981e05304c897a20e0836778c5f209f8c5))
