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

## Moved 2026-10-21 22:19:12 -04:00 -- the comparison of two handles

### <a id="wh-13"></a>WH-1.3 -- `CompareObjectHandles`: whether two handles name the same kernel object, as `compare_object_handles`, and this crate's tests no longer call it raw. *(completed 2026-10-21 22:19:12 -04:00)*

The item as written:

**WH-1.3** -- `CompareObjectHandles`: whether two handles name the same kernel object. It is listed in the
  same section, and the tests of this crate already call it raw (`same_object` in `duplicate/tests.rs`), so a
  safe form lets them stop.

`compare_object_handles(first, second) -> bool`, read from the page and measured on real handles. The duplicate's tests, which
called the raw function through a helper of their own, call it now. Why the answer is a `bool` and not a `Result`, and what was
measured, are [WH-D-4](DESIGN-NOTES.md#wh-d-4).
