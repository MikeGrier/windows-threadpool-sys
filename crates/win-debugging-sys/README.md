# win-debugging-sys

Memory-safe Rust over the Win32 debugging API, the area Microsoft Learn files `OutputDebugString` under:
*Diagnostics > Debugging and Error Handling > Basic Debugging > Debugging Reference > Debugging Functions*. Thin over
Win32, adding no policy.

- `output_debug_string` is `OutputDebugStringW`: it sends a slice of UTF-16 code units to the debugger. The call takes
  a NUL-terminated string, so the units are copied with the terminator added, and nothing past the end of the slice is
  read. `output_debug_str` is the same for a `str`.
- The call returns nothing and reports no error, so a call that is made cannot fail. A string with a NUL in it is
  refused as `OutputDebugStringError::EmbeddedNul`, naming the unit, and the call is not made: the call would take the
  string to end at the NUL and show only what came before. That is this crate's refusal and not the call's. See
  [WD-D-2](DESIGN-NOTES.md#wd-d-2).
- Whether anything is shown depends on a debugger being attached, or on the system debugger being active and its filter
  allowing the string. With neither, the page says the call "does nothing", and it gives no way to tell.

```rust
use win_debugging_sys::{OutputDebugStringError, output_debug_str};

output_debug_str("a line for the debugger")?;

assert_eq!(
    output_debug_str("cut\0short"),
    Err(OutputDebugStringError::EmbeddedNul { index: 3 })
);
# Ok::<(), OutputDebugStringError>(())
```

With the off-by-default `test-util` feature the crate also holds `run_debugged`, a debugger that runs a program and
returns the strings it sent, for a test to see that what a crate sends reaches a debugger. It is a test facility and not a
wrapper of the debugging functions. See [WD-D-3](DESIGN-NOTES.md#wd-d-3).

The crate was made so that the diagnostic sink of `win-durable-ioring` that writes to the debugger has a safe call to
make. See [WD-D-1](DESIGN-NOTES.md#wd-d-1) for the name and where the function is filed.
