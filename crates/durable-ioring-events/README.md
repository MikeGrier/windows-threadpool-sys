# durable-ioring-events

The ETW provider for [durable-ioring](../durable-ioring), as a crate of its own: the events it writes and the manifest that
describes them, and `EtwDiagnostics`, a diagnostic sink that writes each event to the provider.

- The provider is named `durable-ioring-events`, and `PROVIDER_ID` is its GUID, generated once. A session enables it by the
  GUID.
- `EtwDiagnostics` is a `DiagnosticSink`. It wants a kind of event only when a session is listening for it, so with none its
  cost is a test of that: no text is formatted and nothing is written. It registers the provider once for the process, and
  gives the registration back when the last sink is dropped.
- It records whether or not the provider's manifest is installed. **A reader needs the manifest**, to see an event's name and
  its fields and not only its bytes; the application that writes the events does not.
- `EVENTS` is each event as data: its id, name, level, keyword and fields with their types, and `manifest()` renders the
  manifest from it, so the two cannot disagree. The manifest is kept with the crate as `durable-ioring-events.man`, and a test
  holds the file equal to what is rendered.

```rust
use durable_ioring_events::{EtwDiagnostics, PROVIDER_NAME};

let sink = EtwDiagnostics::new();
assert!(sink.is_registered());
assert_eq!(PROVIDER_NAME, "durable-ioring-events");
```

## Reading the events

Without the manifest a trace shows the provider's GUID, each event's id, level and keyword, and its data as bytes. With it a
reader sees the names. Installing it is a step for whoever reads the trace, from an elevated prompt, with the Windows SDK's
tools on the path:

1. Write the manifest, or take `durable-ioring-events.man` from the crate:
   `durable_ioring_events::manifest()` is its text.
2. Compile it: `mc -um durable-ioring-events.man`. That makes a resource script and the binary form of the manifest.
3. Compile the resources: `rc durable-ioring-events.rc`.
4. Link them into a resource-only library: `link -dll -noentry -machine:x64 durable-ioring-events.res -out:durable-ioring-events.dll`
   (`-machine:arm64` on an ARM machine).
5. Install it, naming the library by its full path:
   `wevtutil im durable-ioring-events.man /rf:C:\path\durable-ioring-events.dll /mf:C:\path\durable-ioring-events.dll`.

Then a tool that decodes by manifest, such as `tracerpt`, shows each event by name. `wevtutil um durable-ioring-events.man`
uninstalls it. An event's fields are written in the order the manifest gives, with no padding, and a text field ends at its
first zero unit, so a zero in a message cuts it short for a reader.

See [DE-D-1](DESIGN-NOTES.md#de-d-1) for the name and the GUID, [DE-D-2](DESIGN-NOTES.md#de-d-2) for the events,
[DE-D-3](DESIGN-NOTES.md#de-d-3) for the manifest and [DE-D-4](DESIGN-NOTES.md#de-d-4) for the sink.
