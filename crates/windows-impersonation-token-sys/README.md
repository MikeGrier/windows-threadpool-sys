# windows-impersonation-token-sys

Memory-safe capture, transport, and scoped application of Windows impersonation
tokens.

**Windows only.** Every public item is behind `cfg(windows)`; the crate builds to
an empty shell on other platforms.

## Status

The crate's capture, transport, scoped application, exact restoration, failure
handling, and deterministic test matrix are complete. It is ready for its
initial crates.io release.

## Examples

### Scope access-checked work

`with_impersonation` does not interpret the closure's return value. A fallible
closure therefore returns a nested `Result`: the outer error reports token
application failure and the inner error belongs to the operation.

```rust,no_run
use std::io;
use windows_impersonation_token_sys::ImpersonationToken;

fn access_checked_work() -> io::Result<()> {
    // Perform access-checked Windows work here.
    Ok(())
}

let token = ImpersonationToken::capture()?;
token.with_impersonation(access_checked_work)??;
# Ok::<(), Box<dyn std::error::Error>>(())
```

### Transport a context to another thread

The owned token is `Send + Sync`, and concurrent applications affect only the
thread running each closure.

```rust,no_run
use std::thread;
use windows_impersonation_token_sys::ImpersonationToken;

let token = ImpersonationToken::capture()?;
let worker = thread::spawn(move || {
    token.with_impersonation(|| {
        // Perform access-checked Windows work here.
    })
});

worker.join().expect("worker panicked")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Scope

This crate owns the narrow impersonation-token lifecycle needed by
cross-thread Windows work:

- capture the calling thread's effective impersonation state;
- transport owned state to another thread;
- apply it for a bounded operation; and
- restore the exact prior thread-token state.

It is not a general Windows security or access-token utility collection. The
canonical contract is in [DESIGN-NOTES.md](DESIGN-NOTES.md), with historical
reasoning in [DESIGN-RATIONALE.md](DESIGN-RATIONALE.md).

## Security and restoration guarantees

- Capture produces a real, owned, non-inheritable handle with only
  `TOKEN_IMPERSONATE` access.
- Clones share the same immutable token object; no safe API exposes its handle,
  mutation rights, or rights-expansion path.
- Scoped application saves and restores the exact thread-token object present on
  entry. It does not restore a duplicate and does not use `RevertToSelf`.
- The closure does not run if saving or applying the context fails.
- Restoration failure panics. If the closure is already unwinding, Rust's
  double-panic behavior aborts the process rather than returning a shared thread
  under an unknown identity.

## License

MIT. Copyright (c) Mike Grier.
