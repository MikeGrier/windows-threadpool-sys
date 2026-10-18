# win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns.

Each registered file is opened once and the handle is kept. On every poll the crate reads each
file's current length through that handle, never by path and never from directory change
notifications, and reports the files whose length changed. The crate attaches no meaning to a
change and is never told what the caller consumed. It builds only on Windows.

## Reading the bytes

`reader::TailReader` goes on from the lengths to the bytes. It registers files as the watcher does,
and on each poll it reads what the files gained through an I/O ring, into buffers from a pool, and
delivers it on a queue the caller drains. Each data entry carries a clone of the value the file was
registered with, the range of the file its bytes are from, and the bytes. A shrink or a terminal entry
carries the value and what it reports instead, and a poll-done carries nothing and ends a poll. It is a
module of this crate, and the watcher on its own is unchanged.

## Choosing the sizes of the reading layer

The caller chooses every size the reading layer works to, in `reader::TailReaderOptions`, and
`validate` refuses a size that cannot work before anything is allocated. There is no ceiling that
exists to protect the defaults: a size well above them is accepted wherever the pool, the queue and
the ring can have it.

- `buffer_len` is the size of each buffer a read fills, from 1 byte to the most a ring read takes.
- `buffer_count` is how many buffers there are, at least 1. It also bounds the reads in flight, and
  the memory is `buffer_len` times it, allocated when the layer is made. Whether that memory is
  there is the allocator's answer at that point, not a refused size.
- `client_sq_capacity` is how many entries the client's queue holds, a power of two within the
  bounds the queue reports. Those bounds are what the queue can represent, not what a machine can
  hold: a capacity within them whose memory is not there is refused when the layer is made, as
  `CreateError::Allocation`, as the buffers' memory is.
- `ring_sq_size` and `ring_cq_size` are the I/O ring's two queue sizes. Each is a power of two, no
  larger than the ring reports for the machine, and `ring_sq_size` is at least
  `TailReaderOptions::MIN_RING_SQ_SIZE`. `ring_cq_size` is at least `ring_sq_size` times
  `TailReaderOptions::MIN_RING_CQ_PER_SQ`.

The ring would accept a size that is zero, not a power of two, or smaller than it needs, and give
the caller a ring of a size they did not ask for: it rounds a size up, replaces zero with a default
of its own, and raises a completion queue that is too small for its submission queue. The options
refuse each of those instead, so the ring a caller gets is the ring they asked for. A refusal is an
`OptionsError` that names the option, the value given and the `Cause`.

Reads in flight at once are the least of the pieces announced, `buffer_count`, and the completion
queue the ring gives.

Each size limits one thing, and the reader stops when it reaches any of three, going on again
without a further call when the stop is lifted:

- **Every buffer is held.** A `Chunk` holds its buffer until it drops, so a client that keeps
  `buffer_count` chunks keeps the reader from reading, with nothing in flight, until one drops. A
  poll's poll-done waits for the reads of its poll.
- **The ring is full.** `ring_cq_size` is the most reads the ring holds at once. Past it the reader
  waits for a completion, whatever the pool could lend.
- **The client's queue is full.** `client_sq_capacity` is the most entries waiting at once. Past it
  the reader holds the entry it could not queue and waits for the client to drain and call
  `ring_resume`; nothing is dropped.

A poll is in flight from `poll` until its poll-done has been queued for the caller, which is after every
read of it is delivered, and a `poll` before then fails with `PollError::InFlight`. That includes a poll
waiting on any of the three stops above, so a caller that never drains, or keeps every chunk, never
polls again; a caller that has read a poll-done may poll again at once.

The integration tests of this layer, [tests/reader_real_writers](tests/reader_real_writers), run
each of these over files that child processes write, with the defaults, with sizes well above
them, and with sizes small enough that every stop is reached all the time.

What was decided, and why: [DESIGN-NOTES.md](DESIGN-NOTES.md). The work, in order:
[CHECKLIST.md](CHECKLIST.md).
