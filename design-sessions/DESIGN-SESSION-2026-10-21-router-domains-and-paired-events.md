# Design session -- the router's domains, paired events, and how a database would use domains (2026-10-21)

**Summary.** Began as the engineer's question whether durable-ioring's routing provider (`DI-12.2`) needed
replanning, and widened. Decisions that resulted: [DI-D-80](../crates/durable-ioring/DESIGN-NOTES.md#di-d-80) (durations
come from paired begin and end events with correlation ids, and the router measures nothing) and
[DI-D-81](../crates/durable-ioring/DESIGN-NOTES.md#di-d-81) (a provider registered with the router serves exactly its own
`domains()`). Work queued: `DI-12.2.1` to `DI-12.2.4` and `DI-M19` and `DI-M20` in
[durable-ioring's CHECKLIST.md](../crates/durable-ioring/CHECKLIST.md), and `M39.3` in the root
[CHECKLIST.md](../CHECKLIST.md). The model of a database's storage below is the engineer's, refined in discussion; it is
a working model of how a consumer would use the crate, not a decision, and most of it describes the layer outside the
crate that maps storage onto flush domains.

## What the router reports, and where timing comes from

The assistant split `DI-12.2` into four steps and named two open questions, the first being what the router should
report. It offered: an inconsistency for a domain no provider owns; a per-provider timing event; or leaving it coarse. Asked
whether these were exclusive, the assistant said the first two were additive and the third the only exclusive choice, and
that "per-call timing" had two readings it had conflated: the time inside a provider's `make_durable` call, measured with
nothing stored; and dispatch to a provider's last domain answering, which needs the completions wrapped and adds little
over the core's per-domain timing.

The engineer ruled against both:

> I don't think that the core router should implement any of this timing infrastructure, it should come in via the
> diagnostics and tracing. ETW has the ability to pair begin/end events and then the analysis tools will compute times
> from those events. We can depend on other diagnostic sinks to do similar if we can simply establish (a) correlation IDs
> and (b) reliable begin/end pairings.

The assistant found the shipped delay events short of that in three ways: no correlation id; a flush issued once per seal
and answered once per part, so an answer had no begin of its own; and `EventWrite` taking the calling thread's activity id,
which a begin and an end on different threads do not share. On the ids the engineer said: "our correlation ids are scoped
to a process and so may be u64s generated from an atomic u64 using relaxed increments", and asked for the work to be
queued, which became `DI-M19`.

Then, to the four questions the assistant listed:

- Which pairs: "There should be event pairs for seals (which the diagnostic sink may trivially decline to do anything
  with) and for the more weighty actions like when flushes occur. For the portable ring, I assume that flushes are modeled
  as asynchronous operations by the provider (since they are with Linux iouring) so the begin/end pairing is important
  there." And later: "sinks may decline any pair, to be clear".
- Whether an end keeps `elapsed`: "I say no, and this is something we can choose to enhance in the sinks rather than
  pushing it into the router."
- The three candidate pairs, a seal, a flush of a file or a domain, and a lock wait: "those seem like the required ones".
- The 128-bit activity id: "true, perhaps we use the process id as the upper 64 bits and the u64 as the lower?" Held as a
  working position; the assistant noted that a process id reused within one trace restarts the counter.

Two follow-ups the assistant raised: a seal's span, answered "asked-for to durable-or-failed"; and where a lock wait's end
is written, since without `elapsed` the event's own stamp is the measure, and `DI-D-77` reported after the guard was
released. The engineer: "The lock wait end is written with the lock held. Any synchronization within the diagnostics
platform must not interfere with the synchronization in the durable-ioring." The assistant read that as a leaf-lock rule
on sinks -- a sink's locks may be taken under dioring's, never the other way, and no thread holds a sink's lock while
calling into an instance -- recorded as its statement of the rule and open to correction.

## What decides the domains a registered provider serves

The second open question. The assistant set out: (A) the provider's own `domains()`, one source of truth, but a provider is
registered for all it declares; or (B) domains named at registration, allowing a subset, at the cost of a second list that
can disagree with the provider's and route it domains it never said it serves. The engineer asked how things would work in
the real world.

## The real-world model

The engineer, thinking it through and asking to be checked:

> On Windows, the likely use will to be to have a limited set of folders if not files where a "durability deliberate"
> client will be operating. [...] I think that they will present the storage/storage-roots to a layer slightly above us
> (e.g. the actual database) and that layer will call into the durable-ioring and want to establish a ring with correct
> durability domains for the files and directories.

> The directory aspect is that I believe that some layer will need to map the directory to the volume / flush domain that
> files that would be created in it by name would be in.

> in practice, I think that what would happen is that there would be one or more directories on different physical disks
> for data files where the database may balance for i/o load or disk space, and a log file on another distinct physical
> drive optimized for log write patterns. The data files may be single files where the database manages pages within it or
> the database may decide to use multiple files for whatever reason.

What held, against the API: the layer above knows the roots and so mints the domain bytes, which is what
[API.md](../crates/durable-ioring/API.md) section 13 already expects of "a locality helper's job outside this crate", root
`M39`.

What the assistant refined:

- **A directory maps to a volume, not a physical disk.** Two volumes can share one disk, and so its cache and its failures;
  one volume can span several. A file's declared domains are its volume and every disk beneath it, since extra domains are
  safe and a missing one is the one way a declaration makes the contract false. `M39.2` already plans that traversal.
- **Predict by directory, confirm by handle.** A file created by name in a directory lands on the directory's volume,
  mount points included, so mapping a directory once is sound -- but its path is resolved again at each create, and a
  junction swapped between them sends the file elsewhere. Opening relative to a directory handle would close that, and is
  parked as `DI-15.3` for want of a documented call. The substitute: read the new file's volume from its own handle
  (`FileIdInfo` carries the volume serial number) and compare it with the prediction before the file is added. The engineer:
  "The volume id refinement is a good catch for that race condition." Queued as `M39.3`.
- **The log on its own drive is where domains pay, and through fate, not routing.** The log is its own lineage, and
  section 12's gates hold a data page for the log through its LSN. With nothing declared every file is one domain, so a
  failed flush on a data disk would make the log's writes suspect; declared apart, it does not. That needs no provider.
- **The built-in default is most of the story for a typical database.** A volume-wide flush needs administrative privilege,
  per the `FlushFileBuffers` page, and a log is one file, so per-file is per-volume for it. Providers, and the router, matter
  for remote shares, exotic devices and a privileged service.
- **The mapping layer is the natural builder of the router**, since it mints the bytes the providers must carry. That made
  (A) the natural shape: whatever builds a provider already has its domains. The engineer agreed: DI-D-81.

## Directory entries

Raised by the assistant and kept apart by agreement: a database that creates a data file at run time needs the creation
durable before it relies on it. Linux requires an `fsync` of the directory. What Windows documents -- whether flushing the
new file makes its entry durable, whether a directory handle may be flushed -- the assistant does not know, and the API does
not say. Queued as research and a discussion, `DI-M20`, deliberately coarse beyond that.

*Afterward, 2026-10-21:* `DI-M20` was withdrawn once the research was done
([DI-D-95](../crates/durable-ioring/DESIGN-NOTES.md#di-d-95)). The durable ioring never creates, renames or deletes a file,
and the engineer's directory was framing for root `M39.3`, which holds the mapping of a directory to its flush domains.
