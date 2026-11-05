# probation-results

This branch holds one generated file, `results.json`: for each test on probation, how many
runs it has been in, how many it passed, how many passes in a row it is on, and the last
run and commit it was seen at.

It is derived data, written by the `long-haul` workflow after each run of the probation
tier. Do not edit it by hand, and do not merge it into anything.

It shares no history with `main`. Its history may be squashed or deleted at any time, and
doing so only restarts the count of each test's runs.

To read it, run `tools/report-probation.ps1`. A test is promoted from the `probation`
module to `long_haul` by a change to the test, never by this file.

The reasoning is in DESIGN-NOTES.md on `main`, under "probation-results".
