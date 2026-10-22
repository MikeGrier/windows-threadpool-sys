# Changelog

## [0.2.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-events-v0.1.0...durable-ioring-events-v0.2.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))

## [0.1.0](https://github.com/MikeGrier/windows-threadpool-sys/compare/durable-ioring-events-v0.0.1...durable-ioring-events-v0.1.0) (2026-10-22)


### ⚠ BREAKING CHANGES

* **durable-ioring:** durable-ioring's target and submit traits gain a declared access and carry it in a typed Key<A>, Diagnostic and DiagnosticKind gain the begin/end pair variants, and a sink is given every pair's end; win-durable-ioring asks for file access as a typed set of named rights, takes data access as the type parameter of every handle it opens, takes a share mode as a typed set, takes OpenedFile::open's path as a Wtf16String, uses Win32's own disposition enum, and removes HandleKindQuery and HandleKind.

### Features

* **durable-ioring:** a typed Key&lt;A&gt; carries a target's access, every sink takes the begin/end pairs, and the ETW provider writes each pair as one activity ([341858e](https://github.com/MikeGrier/windows-threadpool-sys/commit/341858ed566c12a38bfde2dd06a7d9c1a9da1a09))


### Dependencies

* The following workspace dependencies were updated
  * dependencies
    * durable-ioring bumped from 0.2.0 to 0.3.0
    * win-etw-sys bumped from 0.0.1 to 0.1.0
