# Mutation survivors -- windows-guard-alloc

Sweep of 2026-09-02. See [README.md](README.md) for the command, the
workspace-wide totals, and how to read a timeout.

- caught: 88
- survived: 22
- timeout: 0

**Partly addressed already** in commit(s) `c16845c, b791418` on branch
`mikegrier/deferred-namespace-ops`. The entries below are as the sweep
found them and have NOT been pruned -- re-run before treating any single
line as outstanding.

## Survived

### src/lib.rs (14)

```
106:5: replace seed_from_environment -> Option<u64> with Some(0)
106:5: replace seed_from_environment -> Option<u64> with Some(1)
106:5: replace seed_from_environment -> Option<u64> with None
124:16: replace == with != in seed_from_environment
124:21: replace || with && in seed_from_environment
153:5: replace seed -> u64 with 0
134:9: delete match arm [ZERO, LOWER_X | UPPER_X, rest @..] in seed_from_environment
153:5: replace seed -> u64 with 1
170:28: replace == with != in seed
232:9: replace GuardAlloc::announce_seed with ()
255:24: replace < with == in GuardAlloc::poison_check
255:24: replace < with <= in GuardAlloc::poison_check
316:14: replace > with == in data_offset
316:14: replace > with >= in data_offset
```

### src/witness.rs (5)

```
50:9: replace <impl std::fmt::Display for Breach>::fmt -> std::fmt::Result with Ok(Default::default())
82:9: replace Witness::ordinal -> u64 with 0
82:9: replace Witness::ordinal -> u64 with 1
97:18: replace < with <= in Witness::permit
158:27: replace match guard range.start <= last.end with false in merged
```

### src/poison.rs (3)

```
66:17: replace < with <= in mul_inverse
87:20: replace < with <= in unxor_shift_right
120:14: replace < with <= in identify
```
