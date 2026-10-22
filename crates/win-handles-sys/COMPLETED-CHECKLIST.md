# Completed checklist: win-handles-sys

Append-only.

## Moved 2026-10-18 01:50:00 -04:00 -- the query that was queued with the crate

### <a id="wh-11"></a>WH-1.1 -- `GetHandleInformation` as a safe query. *(completed 2026-10-18 01:50:00 -04:00)*

`get_handle_information` and `HandleFlags`, built as `durable-ioring`'s `DI-14.8.4`.
What was measured is in [DESIGN-NOTES.md](DESIGN-NOTES.md) `WH-D-2`.

## Moved 2026-10-21 22:06:12 -04:00 -- the write half of the query

### <a id="wh-12"></a>WH-1.2 -- `SetHandleInformation`, the write half of the query, taking the same typed flags and a mask of which are changed. *(completed 2026-10-21 22:06:12 -04:00)*

The item as written:

**WH-1.2** -- `SetHandleInformation`, the write half of the query above, taking the same typed flags and
  a mask of which are changed. It is documented under the same Learn section, so it belongs here.

`set_handle_information(handle, mask, flags)`, with nothing refused ahead of the call, read from the page and measured on a real
handle. The reading, the measurements, and what the sweep does and cannot pin are [WH-D-3](DESIGN-NOTES.md#wh-d-3).
