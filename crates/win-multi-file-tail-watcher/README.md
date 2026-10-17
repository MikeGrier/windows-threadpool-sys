# win-multi-file-tail-watcher

Reports length changes across a large population of files, on a beat the caller owns.

Each registered file is opened once and the handle is kept. On every poll the crate reads each
file's current length through that handle, never by path and never from directory change
notifications, and reports the files whose length changed. The crate attaches no meaning to a
change and is never told what the caller consumed. It builds only on Windows.

## Reading the bytes

`reader::TailReader` goes on from the lengths to the bytes. It registers files as the watcher does,
and on each poll it reads what the files gained through an I/O ring, into buffers from a pool, and
delivers it on a queue the caller drains. Each entry carries a clone of the value the file was
registered with, the range of the file its bytes are from, and the bytes. It is a module of this
crate, and the watcher on its own is unchanged.

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
  bounds the queue reports.
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

What was decided, and why: [DESIGN-NOTES.md](DESIGN-NOTES.md). The work, in order:
[CHECKLIST.md](CHECKLIST.md).
