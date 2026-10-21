# win-etw-sys

Memory-safe Rust over the Win32 Event Tracing provider functions, the ones Microsoft Learn documents under *Event
Tracing*, in *Windows Events*. Thin over Win32, adding no policy.

- `Registration::register` is `EventRegister`: it registers a provider by its GUID and owns the handle, which is
  unregistered when the value is dropped, or by `Registration::unregister`, which says what the call returned. The handle
  is valid in this process only. No enable callback is made.
- `Registration::is_enabled` and `is_level_enabled` are `EventEnabled` and `EventProviderEnabled`: whether ETW can
  quickly tell that no session is listening. `write` makes the same test itself, so they are for a caller that wants to
  skip the work of preparing an event's data.
- `Registration::write` is `EventWrite`: an `EventDescriptor` and a list of typed `Field`s, written in order with no
  padding, as the page says the blocks are. A field is the bytes of a value, and a terminated string is its units and
  then a zero. What a decoder reads them back as is the template's, and the crate has none.
- The call's failure is a `WriteError::Call` with the code. A list of fields that cannot be given to the call, because
  a count or a size does not fit a `ULONG`, is refused as `WriteError::TooManyBlocks` or `BlockTooLong` and the call is not
  made: those are this crate's refusals and not the call's. See [WE-D-2](DESIGN-NOTES.md#we-d-2).
- With no session listening the write "will do nothing and return **ERROR_SUCCESS**". A manifest is not needed to record
  events: it decodes them for a reader.

```rust
use win_etw_sys::{EventDescriptor, Field, Guid, Registration, level};

let provider = Guid::from_u128(0x6e2f4b1a_0c3d_4a55_9d4e_1f2a3b4c5d6e);
let registration = Registration::register(provider)?;

let event = EventDescriptor { id: 1, level: level::INFO, keyword: 1, ..EventDescriptor::default() };
if registration.is_enabled(&event) {
    registration.write(&event, &[Field::U64(7), Field::Utf16Terminated(&[0x68, 0x69])])?;
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

The crate was made so that the ETW sink of durable-ioring, in its own crate, has a safe wrapper to write through. See
[WE-D-1](DESIGN-NOTES.md#we-d-1) for the name and where the functions are filed.
