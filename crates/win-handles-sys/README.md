# win-handles-sys

Memory-safe Rust over the Win32 handle and object API, the area Microsoft Learn files under *System Services >
Windows System Information > Handles and Objects*. Thin over Win32, adding no policy.

- `duplicate_handle_with_access` is `DuplicateHandle` over any handle, in this process, with the access mask
  chosen. It returns an `OwnedHandle`, so the duplicate is closed when it is dropped and nothing else has to.
- `get_handle_information` is `GetHandleInformation`: the `HandleFlags` a handle carries, whether it is
  inheritable and whether closing it is refused. A bit Windows defines that is not named is kept, not refused.
- `set_handle_information` is `SetHandleInformation`: the same `HandleFlags`, with a mask of which are changed. A flag in the
  mask is set if the flags have it and cleared if they do not, and one outside the mask is left as it was. Nothing is refused
  before the call. A handle protected from close is not closed by dropping it, so clear the flag first.
- `Access` is the mask, a newtype over the Win32 bits combined with `|`. `SYNCHRONIZE` and
  `EVENT_MODIFY_STATE` are named; any other right is `Access::from_bits`.

**Windows does not confine a duplicate to the source handle's access.** A duplicate may ask for rights the
source lacks and get them, so narrowing is a guard against a handle being used for what it was not handed out
for, and not a boundary against code in the process that sets out to cross it.

```rust
use std::fs::File;

use win_handles_sys::{Access, duplicate_handle_with_access};

let path = std::env::temp_dir().join(format!("wh-doc-{}.tmp", std::process::id()));
let file = File::create(&path)?;

// A second handle to the same file, that can wait on it and do nothing else.
let waiter = duplicate_handle_with_access(&file, Access::SYNCHRONIZE)?;
drop(waiter);
# drop(file);
# std::fs::remove_file(&path)?;
# Ok::<(), std::io::Error>(())
```

The crate was made by moving this code out of `win-sync-sys`. From that crate's release that depends on this
one, it re-exports this crate at the paths it had; a release from before then has its own copy, whose types are
distinct from these.
