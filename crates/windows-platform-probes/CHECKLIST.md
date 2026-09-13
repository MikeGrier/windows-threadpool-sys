# Checklist: windows-platform-probes

Design decisions are in [DESIGN-NOTES.md](DESIGN-NOTES.md). This crate's *creation* is tracked
separately, in the workspace [CHECKLIST-thread-ambient.md](../../CHECKLIST-thread-ambient.md) milestone
M27; that file is feature-scoped and is deleted when its feature completes, so durable follow-up work
for the crate belongs here instead.

## M3 -- Make the encoded row the contract, and stop checking the prose against it

Decided in [DESIGN-NOTES.md](DESIGN-NOTES.md) -> [The encoded row is the contract; the prose is
not](DESIGN-NOTES.md#d-encoded-row-is-the-contract), from the session in
[design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md](design-sessions/DESIGN-SESSION-2026-09-12-what-the-oracle-should-read.md).

The row is a machine contract mined across a fleet; the prose is for a reader. They carry different
obligations -- the row must be **correct**, enforced by machine; the prose must be **accurate and
readable**, enforced by review. Nothing is required to hold *between* them.

Re-checked against the code rather than against M2's account of it: the original defect was fixed,
and what it left behind was larger. The row published `not_compared`, `parse_incomplete` and
`enumeration_anomalies` as **counts**, where the prose printed each entry's text. A survey reading
`"parse_incomplete":1` could not tell *the probe detected a bug in itself* from *a core record
contradicted itself* from *this topology was not measured from a running machine*. **The row was
impoverished relative to the prose** -- the artifact that gets mined carried less than the artifact
that gets read. M3.1 has since closed that particular gap; the rest of the milestone is about which
artifact carries the contract, and stands whole.

**M2 completed with this decision**, and its ten open items were re-sequenced rather than reworked.
The milestone's own work -- the oracle, the binding, the real-host test, the derived fact set, the
partitioning discriminator and the shape corpus -- is done and archived. The leftovers had
accumulated under a heading none of them fit, and they split by whether M3 gates them: four are in
M4 below, six in M5. M2.18 is the exception, dissolved rather than moved.

- **M2.18 (typed banner) is dissolved into M3.3**, not carried over. It was the smallest instance
  of "should a report be a value a writer renders, or a string the renderer concatenates", and
  answering it alone would have typed one parameter while leaving the shape everywhere else.
- **M2.4** is re-scoped by M3.2. The exploration is still worth doing and its instrument is
  unchanged, but what it hunts for changes: invariants over `Coherence`, `BracketOutcome` and
  `Verdict` as VALUES, and facts the row fails to publish -- not correspondences between two
  renderings. Its closing sentence, "promote only what proves meaningful into the oracle from M2.1",
  now means the invariant set from M3.2. The open question attached to it -- whether this generalises
  past this crate -- survives unchanged and is arguably sharpened, since a data-level invariant is
  easier to share than a text reader.
- **M2.5** is gated by M3.1 and M3.3. Establishing that the middle of three discoveries agreed
  produces a new FACT, which M3.1 says must reach the row rather than only the banner; and M3.3
  changes how the banner is built. Written first, it would be written into machinery about to move.
- **M2.15** keeps its conclusion but loses its evidence. The five failures it cites were all
  `prose: "x86_64"` against `ndjson: "x86"` -- instances of exactly the correspondence M3.4 retires,
  so afterwards they would not occur and a re-run would look clean. The underlying point stands
  without them: CI builds `aarch64` and never tests it, and architecture is the one shape dimension
  a corpus cannot vary because it is fixed at compile time. Restate it on that basis when picked up.
- **M2.17** is re-scoped by M3.5: the dimensions worth crossing become the row's, and crossing prose
  shapes that are about to stop being checked would aim at the retiring half.
- **M2.7, M2.8, M2.9, M2.13, M2.14 and M2.16 are gated by nothing** and are in M5. M2.9 (a
  cross-host ratio called "the finding") and M2.14 (making two authoring rules bite) are if anything
  reinforced:
  under this decision prose accuracy is a review obligation rather than a machine-checked one, which
  puts more weight on both.

- [x] **M3.1** -- Publish each diagnostic as itself, not as a count.

  `not_compared`, `parse_incomplete` and `enumeration_anomalies` reach the row as
  `check.parse_incomplete.len()` and its two siblings, so the fact that a mining pass most needs --
  *which* condition occurred -- exists only in prose. Publish the entries, and give each a stable
  machine-readable discriminant rather than the human sentence, so a survey can group by condition
  without matching on English that is free to be reworded. The sentences stay in the prose, where
  rewording them is harmless.

  **The rule this establishes, which is the durable half:** a renderer may not tell a reader
  something the row cannot tell a survey. A cardinality is not a statement of the fact.

  **Done.** Three enums in `topology::diagnostic` -- 21 + 6 + 3 variants, one per condition -- each
  carrying its data, rendering its sentence through `Display`, and naming itself through `code()`.
  `CrossCheck`'s three `Vec<String>` became `Vec<Diagnostic>`, and the row publishes arrays of codes
  where it published `.len()`. The prose is byte-identical: the loops write `{entry}` and `Display`
  emits the same sentences.

  **The wire format changed**, deliberately and not additively: `"parse_incomplete":1` is now
  `"parse_incomplete":["partitioning_summary_missing"]`. The count is still available as the list's
  length, so nothing is lost, and publishing both would be a restatement that can drift. Same shape
  as the `efficiency_classes` correction that preceded it.

  Sabotage-verified, each mutation injected on its own line and reverted: renaming
  `PartitioningSummaryMissing`'s code reddens only
  `the_row_names_the_probes_own_bug_when_it_detects_one`; making the row keep only the first
  condition reddens the two list tests, through the bound oracle's count rule; mislabelling
  `TrailingBytes` reddens only `an_anomaly_reaches_the_row_as_its_kind`.

  **The substring-to-variant conversion cost two assertions their discrimination, found by review.**
  `c.contains("no online processors and processor groups")` became
  `matches!(c, MeasuredButCountsAbsent { .. })`, which holds when the entry names only ONE of the
  two -- exactly what the test forbids -- and the loop's labels stopped being asserted at all.
  Measured: with `absent` truncated to its first entry the whole suite stayed green at 249 passed.
  Both now assert the variant's `absent` payload, and both were observed to fail -- the truncation
  reddens the both-absent test, and swapping the two names reddens both. The general lesson is that
  converting an assertion from a substring to a variant DROPS whatever the substring discriminated
  inside the payload; the variant is the weaker claim unless the payload comes with it.

  **Which conditions are listed is deliberately not compared against the prose.** The code and the
  sentence come from one variant, so there is no second implementation to disagree through -- the
  correspondence holds by construction, which is stronger than a check. What remains checkable, and
  is checked, is that both renderings list the same NUMBER. Found while converting the accounting
  instrument: a mutation that swapped one code for another went unnoticed on the
  `verdict incomplete` shape, because the oracle reads the length. `corruptions` now APPENDS a code
  rather than substituting one, so the length always differs.

- [x] **M3.2** -- Assert the surviving correspondences as invariants on the observation, before
  rendering.

  Alarm-against-verdict, diagnostics-against-verdict and counters-against-verdict are the three
  oracle rules that survive the decision. They stop being comparisons of two rendered texts and
  become predicates over `Observation` and `CrossCheck` -- `SummaryMissing` implies the verdict is not
  `agree`, a non-empty `parse_incomplete` implies the verdict is not `agree`, and so on. No parser is
  involved, and the check runs whether or not anything was rendered.

  Each one must be sabotage-verified on arrival: delete the invariant, confirm the suite reddens,
  restore it. A predicate that cannot fail is the failure mode this crate keeps meeting.

  **Done, and the item's own framing was wrong in a way worth recording.** It named
  "diagnostics-against-verdict" and "counters-against-verdict" as rules to move. Two of those read
  `CrossCheck`'s lists -- and `verdict` is a pure function of those lists, so such a rule restates
  the definition, cannot fail for any input, and CANNOT CATCH A DELETED PUSH SITE: the deletion
  empties the list, the rule sees nothing, and the verdict is `agree` legitimately. Written that
  way first, with three tests that asserted acceptance under violation-sounding names.

  Every rule now reads the OBSERVATION. `blocking_states` names ten states that forbid an agreeing
  verdict, each with a push site in `cross_check` that it does not consult, plus the two counter
  rules. `check` takes the verdict rather than deriving it, so a test can supply the answer a
  broken `cross_check` would give -- otherwise every branch is reachable only by editing the source
  and a green run says nothing.

  Bound at `observe` (every observation MEASURED, rendered or not -- what this item asked for) and
  at `report` (every observation RENDERED, which on the test side is most of them, since the suite
  builds observations by hand). Not in `cross_check`, which would recurse.

  Sabotage: deleting the `PartitioningSummaryMissing` push reddens four tests, two of them new --
  the invariant's own accounting test, and a render test through `assert_holds` at the renderer
  binding. The invariant is not the sole detector for that push site; its value is the nine others,
  several of which have no dedicated test.

- [x] **M3.3** -- Emit the row from a typed value through one writer.

  > **-> PREREQUISITE: M3.4 lands first.** The reason is on M3.4: this item's nested per-entry data
  > makes `ndjson_list_len` silently miscount, so the parsers it would break should be gone before
  > the row changes shape rather than taught a shape they are about to lose.

  The row is built today by interpolating every value positionally into a `concat!` template.
  Two defect classes follow from that construction and both are closed by replacing it, not by
  checking it:

  **Injection.** Measured on PR #88: an `io::Error` containing `{` was selected as the report's
  machine-readable row, so the oracle checked the caller's text instead of the probe's. Caller text
  reaching the mined artifact is contamination of the contract.

  **Field order and labelling.** A field's name and its value are related only by counting
  positions, so a reordered argument or a miscounted placeholder yields mislabelled data that
  still parses, which nothing downstream can detect. Stated as the coupling rather than as a
  count of placeholders: that count was written twice and wrong twice within an hour.

  A typed row struct plus a single writer that escapes strings makes both unrepresentable. Write the
  writer here rather than adding a serialization dependency -- this crate has none and the row is
  one flat object.

  **Carry each diagnostic's DATA, which M3.1 left behind.** M3.1 publishes a condition's code but
  not the values its variant holds -- a survey learns `contradictory_cores` without learning that
  three cores contradicted themselves. The variants already carry those values, for `Display`; what
  stopped M3.1 publishing them is that the row is still a positional `concat!` template, where a
  nested per-entry object has to be hand-assembled. Once the row is typed this is a field like any
  other. Not deferred for want of a consumer -- the shape of the row is the blocker, and it is this
  item. This subsumes M2.18: the banner becomes a typed field like any other, and the
  question of who may construct one is answered by the row's constructor rather than separately.

  **Done.** `crate::row` holds a `Value` and a `Row` whose members are name-and-value pairs, with
  one writer that escapes strings. Both defect classes are now unrepresentable rather than
  detected: a name and its value move together or not at all, and a `Value::Text` cannot end the
  string it is in.

  Each diagnostic publishes its DATA through `published()`, so a survey learns
  `{"code":"contradictory_cores","count":3}` rather than the code alone -- what M3.1 had to leave
  behind because the row was a positional template. Anomalies carry `source` and `offset` too:
  the same kind at the same offset across a fleet is a different finding from the same kind
  scattered, and neither is visible from a count.

  `report_unmeasured` goes through the same writer, and that is the shape that most needed it --
  it is the only renderer that interpolates caller text, a failed discovery's `io::Error`. The
  error now reaches the row as a `discovery_error` field, so a survey can group failures by cause
  instead of parsing the prose sentence.

  The key-set check M3.4 deferred here now exists -- but NOT in the form M3.4 predicted, and the
  first attempt at it was vacuous. See the correction recorded under M3.7.

  **Two silent behaviour changes were caught by checking the old code rather than trusting the
  rewrite.** `PartitioningCache` has FIVE variants, not the four a rewrite naturally reaches for;
  and `SummaryMissing` publishes its level rather than `null` -- which matters precisely because
  that arm is the report telling a reader the probe has a bug, and WHICH level went unchecked is
  what they need.

  Sabotage-verified: removing the quote escape reddens three row tests, including the
  brace-injection one. The clean row is byte-identical to what the template produced, confirmed
  against a real `probe-topology` run.

- [x] **M3.4** -- Retire the prose-against-row correspondences and the parsers that serve only them.

  > **-> DO THIS BEFORE M3.3, and leave both IDs where they are.** M3.3 carries each diagnostic's
  > data, which turns the flat code arrays into arrays of OBJECTS -- and `ndjson_list_len` splits on
  > `,`, documented as safe for flat code arrays and nothing else. Pointed at
  > `[{"code":"contradictory_cores","cores":3}]` it counts members rather than entries and returns 2
  > for one entry. It does not fail; it silently answers wrong, and every prose-comparison rule then
  > compares that against the prose. Running M3.3 first therefore means teaching parsers a nested
  > shape and deleting them one item later, with a silent-wrong-answer window in between. The IDs
  > stay put because renumbering costs more than the mismatch, the same trade as M4/M5.
  >
  > Intended order for the rest of M3: **M3.2 -> M3.4 -> M3.3 -> M3.5**.

  > **-> CODE REVIEW RESUMES HERE.** Reviews are paused by the engineer's decision of 2026-09-12
  > until this crate no longer depends on prose as the oracle's subject, and this is the item that
  > ends that dependence. The reasoning: a large share of PR #88's fifteen fix commits were defects
  > in the prose-reading machinery -- the multibyte panic in `processors_in_banner`, `trim_matches`
  > collapsing `[[0]]` and `[0]`, `prose_field` selecting the wrong line -- and every one of them is
  > code this item deletes. Reviewing it closely is polishing something already scheduled for
  > demolition.
  >
  > Recorded with the honest counterweight, so the decision can be re-judged on evidence rather than
  > re-argued: of the six findings across the two reviews run on 2026-09-12, none was a defect in
  > the prose oracle. Two were documentation drift, one was a test that had lost its
  > discrimination, and the most valuable -- `disagreements` reaching the prose and not the row at
  > all -- was about the ROW being incomplete and survives this item untouched.

  Of 38 top-level functions in [src/report_oracle.rs](src/report_oracle.rs), ten are correspondence
  rules, four are comparison helpers, and **twenty-three exist only to extract values back out of
  rendered text**. With M3.2 and M3.3 landed, that extraction layer has no remaining consumer.

  What stays is a thin check that the row is **well-formed** -- it parses, it carries the expected
  key set, and it is the only such line in the report. That is not a correspondence; it is the
  writer's own output being checked, and the writer is the one place structure cannot check itself.

  Retire, do not merely stop calling. Dead extraction helpers left in place are a second grammar for
  a format that no longer has two readers.

  **Done, together with M3.5, because they cannot be separated.** The fact-accounting instrument is
  built entirely on `report_oracle::check` and the `Correspondence` variants, so deleting the
  correspondences leaves it measuring nothing and the suite red between the two items. Committed as
  one commit citing both IDs, per the checklist rule for coupled items, rather than split into a
  commit that does not pass.

  Measured: `report_oracle.rs` 79,394 -> 8,874 bytes, its tests 105,299 -> 5,598, the integration
  instrument 71,007 -> 25,689. All eight prose correspondences and all twenty-three extraction
  helpers are gone.

  What survives is the row's well-formedness: exactly one machine-readable line, brackets balanced
  (string-aware, because a failed discovery's `io::Error` is interpolated into a string value and an
  OS message is free to contain a bracket), and no repeated top-level key. That last one is the
  malformation that survives a consumer's parse and changes what it reads, since most JSON readers
  take the last.

  (The bracket check was weaker than this sentence implies -- it counted depth, so a trailing or
  misplaced separator passed. Strengthened in M3.7.)

  **The key-set check is deliberately NOT here.** Asserting it needs a list of expected keys, and a
  list written here is a census -- this component re-corrected the same census three times in one
  day. M3.3 makes the row a typed value, at which point the key set is derivable from the type
  rather than declared beside it. Moved there rather than approximated here.

  (**The second sentence is wrong, and M3.7 corrects it.** A key set is NOT derivable from a typed
  row: the type says "a row is a map of names to values", which is satisfied by every key set,
  including the one missing a field. The census this note was right to fear is a count; a schema is
  not one, and refusing to write it down bought nothing.)

- [x] **M3.5** -- Re-aim the shape corpus and the fact accounting at the row.

  **The instrument enumerates in one direction only, and the other direction is where M3.1's rule
  lives.** `ndjson_keys` reads the ROW's keys and requires each to be classified, so it asks "does
  anything read this key?" -- never "does the prose state a fact the row omits?". A fact with no key
  is outside the set of things it can have an opinion about.

  Measured, and this is how it was found rather than reasoned: `CrossCheck::disagreements` reached
  the prose as a listed entry per disagreement and reached the row as nothing at all. `cross_check`
  said `disagree` without saying WHICH counter did, which is the same shape as the defect the
  milestone came from. It survived 41 review rounds, a zero-survivor mutation sweep and the fact
  accounting, because every one of those instruments starts from what the row publishes. A review
  found it by reading the enum and asking who called `code()` -- the answer was nobody.

  So the accounting needs a second enumeration, from the PROSE's facts to the row's keys, or the
  rule "a renderer may not tell a reader something the row cannot tell a survey" has no instrument
  behind it and holds only as long as someone remembers it.

  **Done, with M3.4, and the second enumeration exists.** The instrument no longer asks "which prose
  facts does the oracle read" -- there are none. It asks, for every state `topology::invariant` knows
  forbids agreement, whether the row publishes a condition for it; and it holds the row's published
  conditions against what the cross-check found, across the corpus.

  (As first written this said the second rule held the row against a count of PROSE lines, which it
  did at the time. The follow-up commit that removed the last prose parsing replaced that with the
  comparison against the cross-check -- recorded further down this same item, so the item disagreed
  with itself. Found by a review.)

  Sabotage-verified against the defect that motivated it: dropping `disagreements` from the row --
  the omission that survived 41 review rounds, a zero-survivor mutation sweep and the old accounting
  -- now reddens the rule that holds the row against the cross-check. That rule was named
  `the_row_lists_a_condition_for_every_diagnostic_the_prose_lists` when this evidence was recorded
  and is `the_row_lists_exactly_the_conditions_the_cross_check_found` now; the sabotage was re-run
  against the current name. Recorded evidence that cannot be re-run as written is evidence nobody
  will re-run.

  **One asymmetry, found by the instrument rather than reasoned.** Counting all four lists against
  prose lines failed: the prose folds every anomaly into ONE
  `windows-topology-sys recorded N enumeration anomal...` sentence while the row lists one code per
  anomaly, so three anomalies read as two dropped entries. `enumeration_anomalies` counts on its own
  axis and is checked against the OBSERVATION -- one published code per anomaly recorded -- which is
  the artifact the row owes fidelity to. Checking it against the number inside that sentence would
  be the prose-reading this milestone retired.

  Both publication rules carry a corpus guard, because both skip a shape in no blocking state and a
  drifted all-healthy corpus would leave them green while checking nothing.

  **The last prose parsing in the matrix is gone.** M3.5 left one site: a rule that filtered
  rendered lines by prefix, counted them, and compared that number against the row -- the only place
  left where the test matrix obtained structured data by reading sentences. It had a unit-test twin
  in `src/tests.rs` that the first sweep missed and a second, wider sweep found.

  Both are replaced by the same claim against `cross_check`: the row's codes must EQUAL the
  cross-check's, in order. Strictly stronger -- a count catches only a dropped entry, this catches a
  drop, a reorder and a substitution -- and it never reads a sentence. It also covers all three
  lists, which the prose count could not: under INCOMPLETE the renderer gives `not_compared` and
  `parse_incomplete` the same bare `- ` prefix, so only their total was recoverable from prose.

  **The ordering half was vacuous, and the guard is what found it.** Reversing the row's
  `parse_incomplete` order reddened nothing: every corpus shape varied one dimension, so each landed
  at most one entry per list, and a one-element list has no order to get wrong. A first version of
  the guard summed the three lists and passed while the sabotage still did nothing -- one entry in
  each of two lists is two conditions and no order. Corrected to measure the largest SINGLE list,
  and a `several conditions at once, in one list` shape added. The reorder now reddens.

  What remains that touches rendered text at all: selecting the row line, and asserting positional
  containment -- the banner is the first line, the banner is one line, there is exactly one row.
  None reads prose for its content.


  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs) enumerates
  the facts a report publishes and measures, by mutation, which are read. The instrument is sound and
  the target changes: enumerate the row's fields, and require each to be read by an invariant or
  explicitly classified as unread. Its corpus of shapes keeps its purpose -- it exists to defeat the
  imagination-driven fixture, which the decision does not change.

  The prose half becomes a rendering test: the renderer emits what it is supposed to emit, judged on
  its own terms rather than against the row.

- [x] **M3.6** -- Split [DESIGN-NOTES.md](DESIGN-NOTES.md) into Tier 1 and Tier 2.

  Measured: 88 KiB, which is **XL** on the repository's byte scale, and the default posture at XL is
  to split unless the module is indivisible. It is not -- it carries current decisions and a large
  volume of how-we-got-here reasoning, which is exactly the Tier 1 / Tier 2 fracture the repository
  instructions describe.

  Move the rationale to `DESIGN-RATIONALE.md`, cross-referenced by decision anchor, leaving Tier 1
  stating what was decided and what forced it. The decision added by this milestone is written to be
  split that way already, so it is the worked example rather than the hard case.

  **Done, and the result is still XL -- say so rather than imply otherwise.** 25,942 bytes moved;
  DESIGN-NOTES.md went 92,467 -> 67,807, which is over the 64 KiB threshold still. The split was
  made at the one unambiguous Tier 2 fracture rather than trimmed to hit a number.

  The fracture: the correspondence-oracle investigation. It is Tier 2 on both tests -- a record of
  how a decision was reached rather than a statement of one, AND a decision since superseded by
  [#d-encoded-row-is-the-contract](DESIGN-NOTES.md#d-encoded-row-is-the-contract). Moving it also
  resolved latent drift: it cites `Correspondence`, the fact-accounting instrument and the prose
  rules, none of which survived M3.4 and M3.5. As history those sentences are accurate; as Tier 1
  they described deleted code.

  Pure relocation, verified byte-for-byte against the pre-split file (439 lines, identical). The
  two moved anchors are kept in Tier 1 beside a pointer, so existing links land somewhere that says
  where the content went, and every in-repo reference was repointed at the content.

> **-> OPEN QUESTION for the engineer:** the remaining bulk of DESIGN-NOTES.md is neither current
> decisions nor rationale -- it is FINDINGS, measurements about Windows that are this crate's actual
> product (the completion-port fork, the thread-agnosticism probe, the x64 comparison, the long-path
> pair, the topology cross-check). They do not belong in a rationale file, and filing them as
> decisions is what keeps Tier 1 XL. Whether they want a tier of their own is a structural choice
> about this component's documentation scheme, so it is raised rather than taken.

- [x] **M3.7** -- Make four instruments as strong as their names claim.

  A review of the completed M3 found no wrong behaviour and four weak instruments -- tests and
  guards whose names assert a property they could not actually fail to satisfy. That is the
  recurring defect class of this whole branch, so the four are recorded with what each one was
  measured to miss.

  **1. The row's key set was unenforced.** M3.3's note above claimed the check "is derived:
  `Row::keys` reads the value, and a test asserts the reader and the writer agree". Both halves
  read the same `Row`, so the test says only that the writer is self-consistent. Measured: deleting
  `.with("packages", ...)` from the renderer left the ENTIRE suite green -- a field silently
  vanishes from every downstream survey and nothing objects. Fixed by declaring
  `MEASURED_ROW_KEYS` / `UNMEASURED_ROW_KEYS` as the contract the renderer is held to. This is not
  the census M3.4 feared: a count is derivable from the thing it counts, so restating it is drift
  waiting to happen; a schema is NOT derivable from the row, which is exactly why writing it down
  buys something.

  **2. The well-formedness oracle accepted invalid JSON.** `balanced()` counted bracket depth, so
  `{"a":1,}` (trailing separator) and `{"a":1]` (mismatched closer) both passed -- and a consumer
  would reject both. Replaced by `malformation()`, a typed delimiter stack that also checks
  separator placement. Sabotage-verified: making the writer emit a leading separator produces a
  balanced but invalid row, which the old check passed and the new one reddens.

  **3. A sabotage asserted only that it had sabotaged.** The publication-accounting sabotage stripped
  a condition from the report and then asserted the condition was absent -- which is a fact about
  the string edit, not about the rule. It would have passed with the rule deleted. The rule is now
  `publication_holds(observation, text)`, and the sabotage asserts it REJECTS the stripped report
  and ACCEPTS the original.

  **4. A completeness guard compared a table against itself.** `blocking_states` returned strings,
  and the guard that checked every blocking state was described derived both sides from that one
  table. Introduced a `BlockingState` enum with `ALL`, so the guard holds the table against the
  type's variants and a new state that nobody describes fails to build past it.

  (**The last clause was false, and M3.8 corrects it.** `ALL` was a hand-written array; nothing
  tied it to the enum.)

- [x] **M3.8** -- Make `BlockingState::ALL` exhaustive by construction rather than by assertion.

  The same defect as M3.7's fourth finding, one level up, and introduced by the fix for it. The
  doc on `ALL` claimed "an exhaustive list the compiler checks: adding a variant without adding it
  there fails to build". That is not what the compiler checks. The `match` in `described()` is
  exhaustive-checked, which is what made the claim look right -- but it forces a new variant to
  acquire an ARM, never an ENTRY in a separate array.

  Measured, not read: a new variant plus the `described()` arm the match demands compiled cleanly
  and left all ten invariant tests green, reached by none of them. `ALL` is the list the
  completeness guard iterates, so a variant missing from it is a blocking state nothing tests --
  which is the exact failure the guard was added to prevent, reintroduced by the shape of its fix.

  The reverse loop in the guard is not a substitute. It catches a state `blocking_states` produces
  and `ALL` omits, but only once some perturbation reaches it -- and a state with no perturbation
  entry is precisely what the test exists to catch, so it is circular in the case that matters.

  Fixed by declaring the enum, `ALL` and `described()` from one list through a macro, so a variant
  that is not in the list does not exist. The claim is now true rather than deleted.

  Sabotage-verified in both directions: the original sabotage is now inexpressible (there is no
  second place to omit the variant from), and its reachable equivalent -- a new state in the list
  with no perturbation entry -- reddens `every_blocking_state_has_a_perturbation`, where before
  the whole suite stayed green.

  **Three rounds on one guard: strings, then a hand-written `ALL`, then generation.** Each fix
  moved the census somewhere harder to see rather than removing it. Worth stating because the
  reviewer's finding was not a new defect -- it was the same defect wearing the previous fix.

## M4 -- Carried over from M2: the items M3 gates

These were written under M2 and are blocked on M3 above: each one targets the prose-against-row
machinery that M3 retires or relocates, so doing them first means doing them twice. M3's preamble
says which M3 item gates each of them.

**The IDs keep their M2 numbers deliberately.** [COMPLETED-CHECKLIST.md](COMPLETED-CHECKLIST.md) is
append-only and its entries are immutable, and two archived entries already cite M2.4 and M2.14 --
so renumbering would leave dangling references in a file that may not be edited to repair them.
Stable IDs cost a mismatch between an item number and its milestone; renumbering would cost
correctness in the archive.

- [ ] **M2.4** -- Explore, with the sparse matrix as the instrument, whether the same correspondence
  failures exist for `Coherence`, `BracketOutcome` and `Verdict`, and in the sibling probes' renderers.
  Expect the matrix to be mostly empty; that is the expected shape and not a sign the exercise failed.
  **Record the vacuous results as well as the findings** -- "X and Y were examined and need not
  correspond" is what stops the next person re-exploring the same cells, and is the half that normally
  evaporates. Promote only what proves meaningful into the oracle from M2.1.

> **-> OPEN QUESTION for the engineer:** M2.4 may show this generalises past this crate, in which case
> the oracle belongs somewhere shared and the question becomes a repository-wide convention rather than
> a probe-crate one. That is a design decision, not a mechanical follow-on, and is deliberately left
> unanswered here.

- [ ] **M2.5** -- Make the banner describe the read the body describes. A probe run performs
  **three** independent `MachineMemoryTopology::discover()` calls: `Fingerprint::discover()` for the
  banner, `measure()`'s own discovery for the body, and `Fingerprint::discover()` again. `attribution`
  compares only the two endpoints, so equal endpoints print an unqualified banner without establishing
  that the middle read agreed with them.

  **The uncovered window is narrow, and worth stating precisely so it is not over- or under-sold.**
  `measure()` brackets its counters around its own discovery, so a processor, group or NUMA change
  during the middle read is already caught as `BracketOutcome::Changed`. What no counter reaches is
  cache and efficiency-class structure. So the reachable case is a run where the cache structure
  differs between the endpoint reads and the middle read while the processor, group and NUMA counts
  stay identical -- near-impossible on real hardware, since caches do not change without processors
  changing, but reachable on a hypervisor returning inconsistent `GetLogicalProcessorInformationEx`
  results, which is exactly the population this probe exists to survey.

  Prefer **construction over comparison**: return the measured topology from `measure()` (as a sibling
  function, so the six existing `measure()` callers are untouched) and build the banner with the
  already-public `Fingerprint::from_topology`. The banner then describes the body's read *by
  construction* and the contradiction becomes unrepresentable, rather than detected by a third
  comparison that is itself new prose able to drift. The endpoint reads still earn their place: they
  catch structural change across the wider window that the counter bracket cannot see.

  **This belongs to M2 rather than beside it:** "the banner describes the measured read" is a
  correspondence invariant, so it should be expressed in the M2.1 oracle and checked on every rendered
  report, not asserted once in a single test.

  Reviewer disagreement is recorded deliberately, because it is evidence about the instrument rather
  than noise: across two rounds one reader raised this twice while two others cleared it, one of them
  explicitly after being pointed at the question. Nothing in the suite decides it either way, which is
  itself the argument for the oracle.

- [ ] **M2.15** -- Run the probe suite on a second architecture in CI.

  A reviewer asked whether the suite was portable and it was not: three renderer fixtures and the
  shape corpus' banner builder each hard-coded `x86_64` while the row they are compared against
  publishes `std::env::consts::ARCH`. Measured on `i686-pc-windows-msvc`: five failures, every one
  `prose: "x86_64"` against `ndjson: "x86"`. CI BUILDS `aarch64` and never TESTS it, so a
  build-and-clippy matrix cannot see this class at all.

  Architecture is the one shape dimension the M2.12 corpus cannot vary, because it is fixed at
  compile time rather than chosen per report -- so the corpus that exists precisely to defeat shape
  blindness is blind here by construction, and only a second test target can close it. Add one
  (`i686-pc-windows-msvc` runs natively on the existing runners; `aarch64` would need its own).

- [ ] **M2.17** -- Cross the corpus dimensions instead of varying one at a time.

  [tests/a_real_report_agrees_with_itself.rs](tests/a_real_report_agrees_with_itself.rs)'s `shapes()`
  builds each shape by taking `base()` and changing ONE thing. That makes every shape easy to read and
  is why the corpus found what it found -- but it means any renderer branch selected by TWO
  dimensions at once is unreachable by construction, and the corpus cannot report the gap because it
  does not know the branch exists.

  Measured: `CrossCheck` tags a `parse_incomplete` entry `- {caveat}` under `INCOMPLETE` but
  `(parse incomplete) {caveat}` under `DISAGREE`. Anomalies appeared only in an agreeing-counter
  shape and disagreements only in a zero-anomaly shape, so the second spelling was never rendered,
  and the anomaly-count rule was silently unread on every disagreeing report -- while the comment
  above it said it was read for every verdict. One hand-written crossed shape closed it, and the
  accounting test went red under sabotage only once that shape existed.

  Enumerate the dimensions the renderer actually branches on (verdict, bracket, coherence, the
  partitioning arm, presence of each diagnostic list) and generate the cross product, or a pairwise
  covering set if the full product is too slow. The corpus already asserts self-consistency and runs
  the fact accounting per shape, so nothing new has to be written to check them -- only to produce
  them. Until this lands, a shape that needs two dimensions must be added by hand, which is exactly
  the imagination-driven process M2.12 exists to replace.


## M5 -- Carried over from M2: unblocked hygiene

**Nothing gates these.** They are grouped last by priority, not by dependency -- none of them touches
the report pipeline, so any of them may be pulled forward ahead of M3 or M4 at any time. They were
discovered during M2 and parked there under a heading none of them fit.

IDs keep their M2 numbers, for the reason given under M4.

- [ ] **M2.7** -- Decide whether the other nine probe steps in CI should carry `if: '!cancelled()'`,
  and apply or record the decision.

  **Measured 2026-09-09:** twelve probe steps in [ci.yml](../../.github/workflows/ci.yml), of which
  three are guarded -- topology, and the doorbell/request pair added with this note. The other nine
  (`error mode`, `handle state`, `worker context`, `pool growth`, `device map`, `IoRing`,
  `completion port`, and both halves of the long-path pair) are skipped whenever an earlier step in
  the job fails, because Actions defaults to `if: success()`.

  The argument for guarding is already written at the topology step and is not specific to it: a
  probe step exists to emit diagnostics, so skipping it on failure suppresses it in exactly the run
  that wanted it. **The long-path pair is the sharpest case** -- its own comment says either half
  alone "says nothing", since the finding is the difference between two executables, so a partial
  run of that pair is worse than useless.

  **It is queued rather than done because there is a real tradeoff, and it is an operational call.**
  `!cancelled()` also runs the step when the *build* failed, where `cargo run` cannot compile and
  the step turns from skipped (grey) into failed (red). That trades quieter broken-build output for
  better broken-test output. The topology step already took that trade; whether all twelve should is
  a judgement about how the CI log is read, not something to settle by consistency alone.

- [ ] **M2.8** -- Carry the OS error in the remaining Win32 assertion messages.

  `last_os_error()` (or a raw `GetLastError`) is in the messages in `doorbell_cost`, `request_cost`
  and `handle_state`, and missing from four sites in probes this peel did not touch:
  `completion_port.rs:224` and `:234` ("create a completion port"), `ioring.rs:320` ("create the
  probe pipe"), and `pool_growth.rs:62` ("create the gate event"). Each says what was being attempted
  and not why it failed, which is the whole of what a CI log can offer someone who cannot rerun under
  a debugger.

  Two rules worth carrying over, both learned the expensive way in this peel. Read the error
  **immediately after the single call whose failure is reported** -- a code attached to a condition
  spanning two calls belongs to whichever ran last, not whichever failed, and can print "The
  operation completed successfully" under a message saying something failed. And attach it only to a
  condition that is genuinely an OS failure: a call that returned a size rather than an error should
  not carry one, since `GetLastError` says nothing about it.

- [ ] **M2.9** -- Stop `request_cost` calling a cross-host ratio "the finding".

  [src/request_cost.rs](src/request_cost.rs) ends its module doc with "Absolute values are
  host-specific; the **ratios against the doorbell and the atomic** are the finding." The ratios that
  [src/bin/request_cost.rs](src/bin/request_cost.rs) actually prints divide THIS host's measurement by
  `DOORBELL_NS_REFERENCE` / `ATOMIC_NS_REFERENCE`, which are constants measured on the Snapdragon X2
  development machine. A ratio with this host's numerator and another host's denominator is neither a
  same-host ratio nor a portable finding, and the emitted report says as much two lines later:
  "re-read that probe on this host before trusting them". So the module doc promotes to "the finding"
  exactly the number its own output tells the reader not to trust.

  **Pre-existing, and deliberately not fixed in the `GetFullPathNameW` peel (PR #86) that found it.**
  It arrived in `ae1e39f`, is already on `main`, and is outside that branch's diff; folding it in
  would have put an unrelated behavioural change into a documentation peel that had already run to
  nineteen review rounds.

  The fix is a decision, not a sweep, which is why this is queued rather than taken: either compute
  both figures on the same host and run (the probe would have to measure the doorbell itself, or read
  a companion artifact), or keep the fixed references and demote them in the prose from "the finding"
  to a labelled cross-host comparison. The first is more useful and more work; the second is honest
  and cheap. Same defect class as PR #86's subject -- a claim stated more strongly than the evidence
  supports -- so whichever is chosen, the wording has to end up matching what the numbers can carry.

- [ ] **M2.13** -- Lint the completed-checklist archive mechanically in CI.

  Three bookkeeping defects reached review on this branch, and all three are decidable by a script: a
  `###` heading concatenated onto the previous line so it did not parse as a heading at all, three
  more headings with no blank line above them, an archived item body left `- [ ]` after being checked
  off, and a pre-existing entry's heading rewritten -- which the file forbids in its own second line.

  Assert, for every `COMPLETED-CHECKLIST.md`: every `###` heading is preceded by a blank line; no
  `- [ ]` remains; and the file has ZERO deleted lines against the merge base. The last one is the
  append-only invariant, and it is the one a human reviewer is least likely to notice.


- [ ] **M2.14** -- Make the two authoring rules this branch earned actually bite. Re-planned
  2026-09-13; see the rationale below before implementing either sub-step.

  **As originally written this item said "write two authoring rules into the repository
  instructions". Measurement says that would have been worse than useless.** The two rules it
  proposed -- *state the invariant, not the census*, and *a new test is not done until it has been
  observed to fail* -- ALREADY EXIST, in
  [.github/copilot-instructions.md](../../.github/copilot-instructions.md) under CONTRACT INTEGRITY
  rule 1 ("Prefer a derived fact to a restated one", and beneath it "verify the binding by
  sabotage: change the definition and confirm the consumer's BEHAVIOR changes"). Writing them again
  would add a second copy of a rule, which is the exact defect that section forbids and the exact
  mechanism -- restatement drift -- it exists to prevent.

  **And statement is demonstrably not the gap.** Both rules were in force on 2026-09-12 and both
  were violated: `09da7e9` claims "Sabotage-verified, EACH against the instrument it was meant to
  strengthen" and then names two sabotages for four fixes. The one that got none is the completeness
  guard, which a review found broken an hour later (M3.8). A rule cited in the commit that breaks it
  will not be repaired by a third copy of itself.

  **This repository has already solved this problem once, and not with a rule.** The
  [ci.yml](../../.github/workflows/ci.yml) `sabotage-harness` job records that the harness "accumulated
  fixes over eleven review rounds and thirteen of the later defects were introduced by earlier fixes,
  because every verification was a one-off command that was then discarded and nothing re-checked an
  earlier guarantee." That is M3.8's story verbatim. The answer then was a CI ratchet.

- [x] **M2.14.1** -- Give `windows-platform-probes` a sabotage manifest, so "observed to fail" is a
  recorded artifact rather than a habit.

  **Done.** [sabotage.json](sabotage.json), 7 entries, swept green: six `caught`, one `survives`,
  all behaving as declared. Not wired into CI, matching the two sibling manifests and the
  `sabotage-harness` job's own note that a sweep "rebuilds a crate per entry and is deliberately an
  occasional instrument".

  **The control is the entry that matters most here.** It rewords a prose line to carry the same
  fact and must SURVIVE, which turns this component's central decision -- the row is the machine
  contract, the prose is for a reader -- from a sentence into a measurement. If it is ever reported
  as caught, a test has started reading the prose again and that test is the defect.

  **The harness found a defect in the manifest that the authoring script missed, which is the
  lesson.** The entry for the escape-aware key reader anchored on `'\\' => escaped = true,`; the
  script checked uniqueness by whole-LINE equality and found one match, while the harness matches by
  SUBSTRING and found two -- the same arm appears in `malformation` at a deeper indent, and the
  shallower line is a substring of the deeper one. The script's check was a second, weaker
  implementation of the harness's rule, which is precisely the defect class this manifest exists to
  catch. The anchor was widened to the function signature; the harness remains the only authority on
  uniqueness.

  `tools/run-sabotage.ps1` exists, has its own tests, and runs in CI;
  [windows-placement-probe](../windows-placement-probe/sabotage.json) (9 entries) and
  [windows-waitable-queues](../windows-waitable-queues/sabotage.json) (39 entries) each carry a
  `sabotage.json`. **This crate carries none**, so every sabotage run while building M3 was ad-hoc
  PowerShell, discarded on the spot -- which is why a `git checkout` destroyed uncommitted work
  twice and a `.Replace` pattern silently matched two sites once. The manifest format's `find` must
  match EXACTLY ONCE, which is precisely the guard that hand-running lacks.

  Eight commits on this branch record their sabotages in the message, so the first pass is
  transcription rather than invention: the defect, the file and the test expected to redden are
  already written down.

  Include at least one `expect: "survives"` control. A manifest of nothing but `caught` cannot
  distinguish a suite that is watching from a suite that fails on any edit.

- [ ] **M2.14.2** -- Add to CONTRACT INTEGRITY rule 1 the one thing this branch learned that it does
  NOT already say, and a pointer to the mechanism. A pointer, not a restatement.

  The genuinely new fact: **when a fix claims to have removed a weakness, sabotage the claim rather
  than the symptom.** Rule 1 tells an author to prefer a derived fact over a restated one; it does
  not warn that an author may believe they derived one when they only MOVED the census. That is
  exactly what happened three times on a single guard -- strings, then a hand-written `ALL`, then
  generation -- each fix relocating the census somewhere harder to see while its commit message
  claimed the class was closed. The wording that would have caught it is about the CLAIM, and rule 1
  currently has no sentence about claims.

  > **-> DEPENDS ON M2.14.1:** the pointer has nothing to point at until the manifest exists.

- [ ] **M2.16** -- Repair the garbled `Report` doc comment, and drop the two counts that have already
  rotted beside it.

  [src/report.rs](src/report.rs) opens its `Report` sink doc with a dangling fragment -- "A [`Report`]
  a renderer can `writeln!` into directly." followed by a blank line and then "is arithmetic. Every
  renderer writes through ..." -- so a sentence was lost in an edit, and "moves only 18 renderer
  signatures." is followed by a bare repeat of the word "signatures." Introduced 2026-09-09 by
  `b5594860` and `3827dc32`, both already on main; found while sweeping a count defect on the report
  -oracle branch, where the file was out of scope to touch.

  Both surviving numbers in that passage are censuses that have since drifted. It claims **332
  `writeln!` sites**; measured now, 354. [DESIGN-NOTES.md](DESIGN-NOTES.md) restates the same 332,
  so the two must be fixed together or they drift apart again. Replace them with the invariant the
  passage is actually arguing -- that `String` already implements `fmt::Write`, so every existing
  write site stands untouched and only the renderer signatures move -- which is what makes the point
  and cannot rot. This is the same defect class as CONTRACT INTEGRITY rule 1 in
  [.github/copilot-instructions.md](../../.github/copilot-instructions.md), which M2.14 exists to
  make bite.
