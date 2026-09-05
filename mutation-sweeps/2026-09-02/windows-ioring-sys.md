# Mutation survivors -- windows-ioring-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 229
- survived: 2
- timeout: 6

**Partly addressed already** in commit(s) `9a9163c` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/batch.rs (1)

```
656:9: replace RegisteredBuffers<B>::is_empty -> bool with false
```

### src/ring.rs (1)

```
196:51: replace | with ^ in InjectedFailure::as_hresult
```

## Timed out

Not survivors. Read the README's note before treating these as gaps.

### src/batch.rs (4)

```
2012:9: replace Batch<'ring>::do_submit -> io::Result<u32> with Ok(0)
2012:9: replace Batch<'ring>::do_submit -> io::Result<u32> with Ok(1)
2037:9: replace <impl Drop for Batch<'_>>::drop with ()
2037:12: delete ! in <impl Drop for Batch<'_>>::drop
```

### src/ring.rs (2)

```
936:9: replace IoRing::drain_for_rundown -> io::Result<()> with Ok(())
969:9: replace IoRing::try_pop -> io::Result<Option<Completion>> with Ok(None)
```
