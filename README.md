# sabotage-records

This branch holds one generated file, `catchers.json`: for each sabotage entry,
which tests caught it, when, and at which commit of `main`'s history.

It is derived data, written by the `sabotage sweeps` workflow after a sweep and
read by `tools/run-sabotage.ps1 -Since` to run fewer entries. Do not edit it by
hand, and do not merge it into anything.

It shares no history with `main`. Its history may be squashed or deleted at any
time: a record is only ever a hint, and a missing one makes a scoped sweep run
more entries, never fewer.

To use it in a clone, run `tools/fetch-sabotage-catchers.ps1`. An entry is used
only if the commit it names is in the clone's history, so a record from a commit
the clone does not have changes nothing.

The reasoning is in DESIGN-NOTES.md on `main`, under "sabotage-records".
