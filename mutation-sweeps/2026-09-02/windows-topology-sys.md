# Mutation survivors -- windows-topology-sys

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 127
- survived: 5
- timeout: 0

**Partly addressed already** in commit(s) `e198b8a` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/domain.rs (4)

```
250:21: replace serde_impl::<impl Deserialize<'de> for AttributeValue>::deserialize::<impl Visitor<'de> for ValueVisitor>::expecting -> fmt::Result with Ok(Default::default())
297:9: replace serde_impl::as_bool -> Result<bool, E> with Ok(true)
313:13: delete match arm AttributeValue::SignedInteger(n) in serde_impl::as_u64
375:48: replace match guard map.len() == 1 with true in serde_impl::cache_kind_from_value
```

### src/processor_set.rs (1)

```
30:9: replace ProcessorSet::empty -> Self with Default::default()
```
