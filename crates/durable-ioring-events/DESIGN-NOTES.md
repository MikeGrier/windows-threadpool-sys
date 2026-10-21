# Design notes: durable-ioring-events

## <a id="de-d-1"></a>DE-D-1: the provider is `durable-ioring-events`, in a crate of its own, and its GUID is fixed

**The name is the engineer's, 2026-10-20.** Asked where the delay events' provider should live and what it should be called,
the engineer answered: "win-dioring, I'd put it in its own crate" ([DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76)).
When the scope of that name was asked about, it read as broader than the dioring family, and the engineer said: "Change the
name of the event source to durable-ioring-events, replacing dashes with underscores as appropriate." So the provider is
named `durable-ioring-events`, and so is the crate; its library is `durable_ioring_events`, and the provider's name keeps its
dashes because it is a label in the manifest.

**The GUID is generated once, 2026-10-20**, with `[guid]::NewGuid()`, and is `{47067f6f-8f2b-4278-9b92-092dfb3385e6}`,
`PROVIDER_ID`. A session enables the provider by it, so changing it breaks every session that did, and a test pins it. It is
recorded here and in the code and nowhere is it derived.

**The scope is the dioring family.** The provider carries the delay events of [DI-D-26](../durable-ioring/DESIGN-NOTES.md#di-d-26)
that `durable-ioring` and `win-durable-ioring` report, and the inconsistency report, and nothing from another crate. The
provider is registered once for a process and shared by every sink that is alive, which is the assistant's working position
from the question of its scope and is built as that: it is not a ruling.

**Scheduled work.** None. The Windows driver's default sink is [DI-12.1.6](../durable-ioring/CHECKLIST.md).

## <a id="de-d-2"></a>DE-D-2: the events are one table, an event of durable-ioring is one event of the provider, and a flush answered is two

**The table is the one description of an event.** `EVENTS` holds, for each event, its id, name, the kind of event of
durable-ioring that becomes it, its level, its keyword and its fields with their types. The manifest is rendered from it
([DE-D-3](#de-d-3)), the sink encodes each event to its columns, and a test reads an event back from the bytes by it, so the
three cannot disagree about a field, which is the restatement drift the repository's rules name.

**Each event of durable-ioring is one event here, except a flush answered, which is two.** An inconsistency, a lock wait, a
flush issued and a seal waited are one each. A flush answered carries a file or a domain, and the two have different fields (a
file's key, a domain's name as bytes with a length), so each is an event of its own: `FlushAnsweredFile` and
`FlushAnsweredDomain`. They share a level and a keyword, which a test holds, so that a kind can be asked about by its first
event. This is the assistant's choice and is open to a ruling; the alternative is one event with a part kind and fields that
are empty for the other part.

**Fields.** Every event begins with the instance, the number it was minted with, so a reader can tell instances apart; a
lineage is a number; an epoch is text, built from the `Debug` form durable-ioring renders it in ([DI-D-77](../durable-ioring/DESIGN-NOTES.md#di-d-77)),
and a duration is nanoseconds as a 64-bit number, the largest if it does not fit. A flush's outcome is a four-byte boolean, which is
what a manifest's `Boolean` is. A domain's name is bytes the consumer chose, so it is a `Binary` with its length written
first, and a name too long for the length is cut to what the length can say so that the two agree.

**Level and keyword.** An inconsistency is an error and each delay is informational. The keywords are the categories a reader
filters by: locks, flushes, seals and inconsistencies, one bit each in the provider's forty-eight. Every event has a nonzero
level and keyword, as the page for the structure asks of every event, and is version 0, channel 0, opcode 0 and task 0.

**Text.** Text is written as UTF-16 with a terminator, as a manifest's `UnicodeString` is read. A zero in the text is written
whole and ends the string for a reader, so what follows it in that field is not shown. The one text that is arbitrary, an
inconsistency's detail, is the last field of its event, so no field after it is misread. An event larger than a session's buffer is dropped by the call and the sink
ignores that.

**The ids are pinned.** A reader decodes by an event's id, so they are 1 to 6 in this order and a test holds them.

**Scheduled work.** None.

## <a id="de-d-3"></a>DE-D-3: the manifest is rendered from the table and kept with the crate, and it was checked with the real tools

**Rendered, and kept.** `manifest()` renders the manifest from `EVENTS`, and `durable-ioring-events.man`, beside the crate's
`Cargo.toml`, is that text, held equal to it by a test; the ignored test `rewrite_the_manifest_file` brings the file up to
date. The crate has no build step that needs the Windows SDK, so nothing a consumer builds depends on it.

**What it says.** The provider's name and GUID, its keywords, each event with its id, version, level, keyword and template, and
each template with the types of its fields in order. The provider's resource and message file names are
`durable-ioring-events.dll`, which an installer overrides with the file it is given; the events have no localized messages, so a
tool shows a field's name and value and no sentence.

**Installing it is the reader's.** The provider records without it ([DI-D-76](../durable-ioring/DESIGN-NOTES.md#di-d-76), and
the page for `EventRegister`: "It is not necessary to install a manifest to write events or capture traces"). A reader who
wants names compiles it with the Windows SDK's `mc` and `rc`, links the resource-only library, and installs it with
`wevtutil im`, which the README gives step by step.

**Checked, 2026-10-20.** The steps in the README were run: `mc -um` accepted the manifest, `rc` and `link` made the library,
and `wevtutil im` installed it. A session for the provider recorded the events the example writes, and `tracerpt` named the
provider and showed each of the six events with its level and keyword by name, and every field by name with its value: the
instance, the lock's name and its wait, the epoch as text, a file's key, a flush's outcome as true or false, and a domain's
bytes with their length (`0x7373642D34` for `ssd-4`). `tracerpt` warned that "Some events do not match the schema", which was
about the two header events every trace carries and not the provider's. The manifest was uninstalled afterwards. This was done
by hand and is not a test: it needs the SDK and an elevated process, and what the tests hold is that the manifest is the table
and that the table is what is written ([DE-D-2](#de-d-2)).

**Scheduled work.** None.

## <a id="de-d-4"></a>DE-D-4: the sink shares one registration, wants a kind only when a session listens to it, and ignores a failed write

**One registration, shared.** `EtwDiagnostics` holds an `Arc` to a registration that every sink of the process shares while
any is alive: the first makes it, a later one finds it, and the last to be dropped unregisters it. The page asks a component
to register "one or two" providers, a process has a limit, and a library that is unloaded must give its registration back
("A DLL that unloads without freeing all of the provider handles that it registered may cause the process to crash"). A static
that is never dropped would do the first and not the last. The assistant chose this; it is open to a ruling.

**`wants` is a test, and `report` tests again.** `wants` asks `EventProviderEnabled` for the kind's first event's level and
keyword, so a session that is not listening costs the instance one branch and no text is built, as
[DI-D-51](../durable-ioring/DESIGN-NOTES.md#di-d-51) and [DI-D-26](../durable-ioring/DESIGN-NOTES.md#di-d-26) require. `report`
asks `EventEnabled` of the event itself before it builds anything, because `wants` was asked of a kind and an event's
descriptor is finer. A unit test counts how many times a field's text is formatted: none with no session, and once with one.

**A sink that cannot register wants nothing.** `new` carries on with no registration if `EventRegister` fails, as the page says
production code should; `is_registered` says whether it did, and `try_new` returns the failure for a caller that wants it.

**A failed write is ignored.** The page says the code "is primarily intended for use in debugging and diagnostic scenarios", and
a sink has no way to report a failure of its own.

**The tests.** Without a session, in memory: a sink registers, wants nothing, builds nothing, shares one registration and
gives it back. With one, `tests/session.rs`, which starts a real session, enables the provider in it, reports through the sink
and reads the events back, decoding them by the table: every event reaches a session as its values, a keyword filter and a
level filter are what the sink is asked about, a sink made before a session wants events once one listens, two sinks and four
threads record each event once, a zero in text is written whole, and an event too large for a session is dropped with no
failure. Starting a session needs privilege, so those tests are ignored by default and CI runs them with `--include-ignored`
([WE-D-3](../win-etw-sys/DESIGN-NOTES.md#we-d-3)). They were run elevated on 2026-10-20 and all nine pass.

**Scheduled work.** None beyond [DI-12.1.6](../durable-ioring/CHECKLIST.md), which makes this the Windows driver's default.
