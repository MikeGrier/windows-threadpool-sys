// Copyright (c) Mike Grier.

//! Correspondences a rendered report must satisfy *between* its parts.
//!
//! # Why this exists rather than more per-part tests
//!
//! A pull-request review found [`crate::topology_report`] printing
//! `BUG IN THIS PROBE ... Nothing below about cache partitioning can be
//! trusted` while the verdict two paragraphs below read `=> agree`. Twenty-eight
//! rounds of per-artifact review and a zero-surviving-mutant `cargo-mutants` run
//! had both passed over it, because **every function involved was correct on its
//! own terms** and the defect lived in the relation between two of them.
//!
//! That is the shape no per-part instrument can see. A test asserts one
//! function's output; a mutant perturbs one function's behaviour; a reviewer
//! reads one artifact and finds it locally true. A contradiction between two
//! locally-true parts is invisible to all three.
//!
//! # It reads the artifact, not the state that produced it
//!
//! Every check here works on the **rendered text** -- the thing a reader and a
//! log-mining pass actually receive. Checking internal state instead would miss
//! precisely the defect class this exists for: the state was consistent in the
//! case above, and the two renderings of it were not.
//!
//! # What it deliberately does not do
//!
//! It does not re-derive what the renderer should have printed. A second
//! implementation of the rendering rules would be a check of the copy rather
//! than of the contract, and would drift from the original the moment either
//! moved. Each rule below relates **two things already visible in the report**,
//! so the oracle has no opinion of its own to go stale.
//!
//! Over-constraining is the same defect as under-specifying, so a report that
//! omits a fact is not a violation -- absence is checked only where the report
//! itself makes a claim that requires the other part to agree.
//!
//! Seeded with three correlations, each of which is known to be real **because
//! it was violated**. It is not a speculative list to extend by imagination: a
//! fourth is added when a fourth contradiction is found.

/// A correspondence between two parts of a report that did not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Correspondence {
    /// The prose raised an alarm while the verdict said everything matched.
    ///
    /// The original defect. An alarm is a statement that some part of the
    /// report cannot be trusted; a verdict of `agree` is a statement that every
    /// check made was made and matched. Both can be locally true and they
    /// cannot both describe the same run.
    AlarmWithAgreeingVerdict {
        /// The alarm line found in the prose.
        alarm: String,
        /// Where the agreeing verdict was found: `prose` or `ndjson`.
        verdict_source: &'static str,
    },
    /// One fact rendered twice, with the two renderings disagreeing.
    ///
    /// A report carries its findings for a human in prose and for a mining pass
    /// in NDJSON. A consumer that reconciles the two cannot, and neither
    /// rendering is self-evidently the wrong one.
    ProseAndNdjsonDisagree {
        /// What the fact is called, for the reader of the failure.
        fact: &'static str,
        /// As the prose rendered it.
        prose: String,
        /// As the NDJSON rendered it.
        ndjson: String,
    },
    /// A hardware claim was stated without its caveat while the report's own
    /// evidence says the parse was in doubt.
    ///
    /// The renderer's rule is that every hardware conclusion is gated on the
    /// parse being whole. A claim printed bare, in a report that elsewhere
    /// reports doubt, is that rule with an exception -- and a rule with an
    /// exception is not a rule.
    UncaveatedClaimUnderDoubt {
        /// The claim that was stated bare.
        claim: &'static str,
        /// The report's own visible evidence of doubt.
        evidence: String,
    },
    /// The banner names one machine and the body describes another.
    ///
    /// The banner is the line a reader uses to decide whether two runs are
    /// comparable at all, so a banner describing a different machine from the
    /// body under it invalidates every comparison drawn from the report --
    /// while each half stays locally correct, which is what let this survive
    /// review.
    BannerDisagreesWithBody {
        /// The processor count the banner named.
        banner: String,
        /// The processor count the body reported.
        body: String,
    },
}

/// Every correspondence `report` violates, in the order they were checked.
///
/// An empty result means every correlation this oracle knows about held. It
/// does **not** mean the report is correct: an oracle is a floor, not a
/// specification.
#[must_use]
pub fn check(report: &str) -> Vec<Correspondence> {
    let mut found = Vec::new();
    let ndjson = ndjson_line(report);

    check_alarm_against_verdict(report, ndjson, &mut found);
    check_prose_against_ndjson(report, ndjson, &mut found);
    check_claims_against_doubt(report, ndjson, &mut found);
    check_structured_pairs(report, ndjson, &mut found);
    check_counters_against_verdict(report, ndjson, &mut found);
    check_banner_against_body(report, ndjson, &mut found);

    found
}

/// The banner names the machine the body describes.
///
/// A run makes three discoveries of the host -- one before the measurement,
/// `measure`'s own, and one after -- and the banner used to be built from an
/// endpoint, so it could name a different topology from the body beneath it
/// with nothing in the report saying so.
///
/// **This rule survives the fix that made that unrepresentable, and is not
/// redundant with it.** `measure_observed` now builds the banner from the body's
/// own topology, so the two cannot come from different reads; but they are still
/// two independent *derivations* from that one topology --
/// `Fingerprint::from_topology` and `observe`, each with its own filter for
/// which processors count. Those have already disagreed once, when
/// `from_topology` summed core-domain membership and printed `0p` for a machine
/// about to be measured on four processors. Construction closes the read gap;
/// this closes the derivation gap, on every report rather than in one test.
///
/// Reads the FIRST banner line only. When the endpoint readings disagree the
/// banner carries a second line naming the other reading, which is a reading the
/// body deliberately does not describe -- comparing it here would report a
/// contradiction the renderer went to some trouble to state honestly.
fn check_banner_against_body(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let (Some(banner), Some(ndjson)) = (
        report.lines().find(|line| line.starts_with("host:")),
        ndjson,
    ) else {
        return;
    };
    // Absent on a report whose discovery failed: the banner reads `UNKNOWN` and
    // `report_unmeasured` emits no processor count, so there is nothing to
    // relate and no violation to claim.
    let (Some(banner_count), Some(body_count)) = (
        processors_in_banner(banner),
        ndjson_field(ndjson, "processors"),
    ) else {
        return;
    };

    if banner_count != body_count {
        found.push(Correspondence::BannerDisagreesWithBody {
            banner: banner_count.to_owned(),
            body: body_count.to_owned(),
        });
    }
}

/// The processor count a banner line names, as it was rendered.
///
/// The fingerprint renders as `<arch> <N>p/<M>c smt<S>`, optionally behind a
/// `!!taint!! ` prefix, so the count is the digits immediately before `p/`.
/// Returned as text rather than parsed, so a malformed count is reported as the
/// mismatch it is instead of being silently discarded by a failed parse.
fn processors_in_banner(banner: &str) -> Option<&str> {
    let before = &banner[..banner.find("p/")?];
    let start = before
        .rfind(|c: char| !c.is_ascii_digit())
        .map_or(0, |i| i + 1);
    let digits = &before[start..];
    (!digits.is_empty()).then_some(digits)
}

/// [`check`], as an assertion, for tests that render a report.
///
/// # Panics
///
/// Panics listing every correspondence the report violated.
pub fn assert_corresponds(report: &str) {
    let violations = check(report);
    assert!(
        violations.is_empty(),
        "the report's parts contradict each other: {violations:#?}\n\n\
         --- the report ---\n{report}"
    );
}

/// The report's machine-readable line, if it has one.
///
/// A report is prose with at most one NDJSON line in it. `report_unmeasured`
/// emits a much shorter object than `report`, so every field read below is
/// optional by construction.
fn ndjson_line(report: &str) -> Option<&str> {
    report.lines().find(|line| line.starts_with('{'))
}

/// The raw text of one field of a flat JSON object.
///
/// Hand-written rather than pulled from a JSON crate because this crate has no
/// such dependency and the object is emitted a few lines away in this same
/// crate: flat, unnested, and machine-generated. It returns the value's source
/// text -- quotes stripped for a string, otherwise verbatim -- so a caller
/// compares renderings rather than parsed values, which is the point.
fn ndjson_field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("\"{key}\":");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];

    let value = if let Some(stripped) = rest.strip_prefix('"') {
        let end = stripped.find('"')?;
        &stripped[..end]
    } else if rest.starts_with('[') || rest.starts_with('{') {
        // Balanced, not first-closer. `caches` is an array OF objects and
        // `policies` is an object, so stopping at the first `]` or `}` would
        // truncate both -- returning `[{"level":1,"domains":8` for a three-level
        // machine, which then compares unequal against anything and reports a
        // contradiction that is the reader's own parse.
        let end = balanced_end(rest)?;
        &rest[1..end]
    } else {
        let end = rest.find([',', '}']).unwrap_or(rest.len());
        &rest[..end]
    };

    Some(value.trim())
}

/// The index of the bracket closing the one `text` opens with.
fn balanced_end(text: &str) -> Option<usize> {
    let mut depth = 0_i32;
    for (index, character) in text.char_indices() {
        match character {
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The text after `label` on the line that begins with it.
fn prose_field<'a>(report: &'a str, label: &str) -> Option<&'a str> {
    report
        .lines()
        .find(|line| line.starts_with(label))
        .map(|line| line[label.len()..].trim())
}

/// Alarms the prose can raise. Each is a statement that part of the report is
/// not to be trusted.
const ALARMS: &[&str] = &["BUG IN THIS PROBE"];

fn check_alarm_against_verdict(
    report: &str,
    ndjson: Option<&str>,
    found: &mut Vec<Correspondence>,
) {
    let Some(alarm) = report
        .lines()
        .find(|line| ALARMS.iter().any(|marker| line.contains(marker)))
    else {
        return;
    };

    if report.contains("=> agree") {
        found.push(Correspondence::AlarmWithAgreeingVerdict {
            alarm: alarm.trim().to_owned(),
            verdict_source: "prose",
        });
    }

    if ndjson.and_then(|line| ndjson_field(line, "cross_check")) == Some("agree") {
        found.push(Correspondence::AlarmWithAgreeingVerdict {
            alarm: alarm.trim().to_owned(),
            verdict_source: "ndjson",
        });
    }
}

/// Facts this report renders twice: the prose label, the NDJSON key, and the
/// name to use when they disagree.
///
/// Counts only. A prose line reads `processors (online) : 16` and the NDJSON
/// `"processors":16`, so the comparison is of the rendered values with the
/// prose label removed.
const DOUBLE_RENDERED: &[(&str, &str, &str)] = &[
    ("processors (online) : ", "processors", "online processors"),
    ("processor groups    : ", "groups", "processor groups"),
    ("packages            : ", "packages", "packages"),
    ("physical cores      : ", "cores", "physical cores"),
];

fn check_prose_against_ndjson(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    for (label, key, fact) in DOUBLE_RENDERED {
        let (Some(prose), Some(json)) = (prose_field(report, label), ndjson_field(ndjson, key))
        else {
            continue;
        };
        if prose != json {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact,
                prose: prose.to_owned(),
                ndjson: json.to_owned(),
            });
        }
    }

    // The efficiency classes, whose two renderings differ in punctuation and so
    // cannot be compared as text. This pair is here because it was wrong: the
    // NDJSON once emitted the class COUNT under a plural name, so a
    // single-class host printed `"efficiency_classes":1` beside a prose
    // `efficiency classes: [0]` -- the same fact, in one report, in two
    // renderings a consumer cannot reconcile.
    if let (Some(prose), Some(json)) = (
        prose_field(report, "  efficiency classes: "),
        ndjson_field(ndjson, "efficiency_classes"),
    ) {
        let prose_classes = normalise_list(prose);
        let json_classes = normalise_list(json);
        if prose_classes != json_classes {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact: "efficiency classes",
                prose: prose_classes,
                ndjson: json_classes,
            });
        }
    }

    // The verdict, which the prose states as a sentence and the NDJSON as a
    // token.
    let prose_verdict = if report.contains("=> agree") {
        Some("agree")
    } else if report.contains("=> DISAGREE") {
        Some("disagree")
    } else if report.contains("=> INCOMPLETE") {
        Some("incomplete")
    } else {
        None
    };

    if let (Some(prose), Some(json)) = (prose_verdict, ndjson_field(ndjson, "cross_check"))
        && prose != json
    {
        found.push(Correspondence::ProseAndNdjsonDisagree {
            fact: "cross-check verdict",
            prose: prose.to_owned(),
            ndjson: json.to_owned(),
        });
    }
}

/// The facts whose two renderings differ in shape rather than punctuation.
///
/// Found by the M2.4 matrix rather than by a defect. The four counts already
/// checked above were the ones a reviewer had happened to look at; walking every
/// NDJSON field against the prose showed these carrying the same fact twice as
/// well, with nothing comparing them.
fn check_structured_pairs(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    // `NUMA domains        : 1 (0 with no processors)` against two fields.
    if let Some(prose) = prose_field(report, "NUMA domains        : ") {
        let total = prose.split_whitespace().next().unwrap_or_default();
        let without = prose
            .split_once('(')
            .and_then(|(_, rest)| rest.split_whitespace().next())
            .unwrap_or_default();

        compare(
            found,
            "NUMA domains",
            total,
            ndjson_field(ndjson, "numa_domains"),
        );
        compare(
            found,
            "NUMA domains without processors",
            without,
            ndjson_field(ndjson, "numa_domains_without_processors"),
        );
    }

    // `outermost cache that partitions the processors it covers: L2 (8 domains)`
    // against the level the NDJSON names. This pair is the one the original
    // defect lived next to: the prose can name a level the machine-readable
    // line does not.
    if let Some(prose) = prose_field(
        report,
        "outermost cache that partitions the processors it covers: ",
    ) {
        let level = prose
            .trim_start_matches('L')
            .split_whitespace()
            .next()
            .unwrap_or_default();
        compare(
            found,
            "outermost partitioning cache level",
            level,
            ndjson_field(ndjson, "outermost_partitioning_cache_level"),
        );
    }

    // The policy table against the `policies` object. A policy's domain count is
    // what the whole report is for, so two renderings of it disagreeing would
    // mislead exactly the reader who came for the answer.
    if let Some(policies) = ndjson_field(ndjson, "policies") {
        for (name, count) in policy_rows(report) {
            let key = format!("\"{name}\":");
            let json = policies
                .find(&key)
                .map(|at| &policies[at + key.len()..])
                .map(|rest| {
                    let end = rest.find(',').unwrap_or(rest.len());
                    rest[..end].trim()
                });
            compare(found, "policy domain count", &count, json);
        }
    }

    // The cache table against the `caches` array, level by level.
    if let Some(caches) = ndjson_field(ndjson, "caches") {
        for (level, domains) in cache_rows(report) {
            let key = format!("\"level\":{level},\"domains\":");
            let json = caches
                .find(&key)
                .map(|at| &caches[at + key.len()..])
                .map(|rest| {
                    let end = rest.find([',', '}']).unwrap_or(rest.len());
                    rest[..end].trim()
                });
            compare(found, "cache domain count", &domains, json);
        }
    }
}

/// Push a disagreement when both renderings are present and differ.
fn compare(found: &mut Vec<Correspondence>, fact: &'static str, prose: &str, json: Option<&str>) {
    let Some(json) = json else {
        return;
    };
    if prose != json {
        found.push(Correspondence::ProseAndNdjsonDisagree {
            fact,
            prose: prose.to_owned(),
            ndjson: json.to_owned(),
        });
    }
}

/// `(policy name, domain count)` for each row of the policy table.
fn policy_rows(report: &str) -> Vec<(String, String)> {
    report
        .lines()
        .skip_while(|line| !line.starts_with("domains each policy would produce:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some((parts.next()?.to_owned(), parts.next()?.to_owned()))
        })
        .collect()
}

/// `(level, domain count)` for each row of the cache table.
fn cache_rows(report: &str) -> Vec<(String, String)> {
    report
        .lines()
        .skip_while(|line| !line.starts_with("caches:"))
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let level = parts.next()?.strip_prefix('L')?.to_owned();
            Some((level, parts.next()?.to_owned()))
        })
        .collect()
}

/// The independently-read Win32 counters against the verdict drawn from them.
///
/// A different shape from the rules above, and the one closest to what this
/// probe is *for*. The prose prints each counter beside the enumerated value it
/// was read to check; the whole point of the run is that a mismatch is a
/// finding. So a counter that disagrees with the enumeration while the verdict
/// reads `agree` is the original defect in its purest form -- the report
/// showing its own contradicting evidence directly above a verdict denying it.
fn check_counters_against_verdict(
    report: &str,
    ndjson: Option<&str>,
    found: &mut Vec<Correspondence>,
) {
    let Some(ndjson) = ndjson else {
        return;
    };
    if ndjson_field(ndjson, "cross_check") != Some("agree") {
        return;
    }

    // Only the two counters that are a direct count of an enumerated quantity.
    // `GetNumaHighestNodeNumber` is deliberately absent: it reports the largest
    // node NUMBER, which the report itself says is not a count, so comparing it
    // against `numa_domains` would manufacture a disagreement on any machine
    // with sparse node numbering.
    for (label, key, fact) in [
        (
            "  GetActiveProcessorCount     : ",
            "processors",
            "active processor count against the enumeration",
        ),
        (
            "  GetActiveProcessorGroupCount: ",
            "groups",
            "active group count against the enumeration",
        ),
    ] {
        let (Some(counter), Some(enumerated)) =
            (prose_field(report, label), ndjson_field(ndjson, key))
        else {
            continue;
        };
        if counter != enumerated {
            found.push(Correspondence::ProseAndNdjsonDisagree {
                fact,
                prose: counter.to_owned(),
                ndjson: enumerated.to_owned(),
            });
        }
    }
}

/// A list of numbers as a comparable string, whichever way it was punctuated.
fn normalise_list(rendered: &str) -> String {
    rendered
        .trim_matches(['[', ']'])
        .split(',')
        .map(str::trim)
        .filter(|piece| !piece.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

/// Hardware claims the prose can make, each with the caveat that must accompany
/// it when the parse is in doubt.
const GATED_CLAIMS: &[(&str, &str, &str)] = &[(
    "(heterogeneous: an I/O thread left unconstrained can land on an",
    "This run did not establish that the parse is whole",
    "heterogeneity",
)];

fn check_claims_against_doubt(report: &str, ndjson: Option<&str>, found: &mut Vec<Correspondence>) {
    let Some(ndjson) = ndjson else {
        return;
    };

    // The report's own visible evidence that its parse was in doubt.
    //
    // These two are exactly what `CrossCheck::parse_in_doubt` is defined as --
    // a non-empty `parse_incomplete` or a non-empty `disagreements`, the latter
    // being what makes the verdict `disagree`. That correspondence is the point
    // rather than a coincidence: if the definition changes and this does not,
    // the sabotage check in M2.2 is what should notice.
    let mut evidence = Vec::new();
    if let Some(count) = ndjson_field(ndjson, "parse_incomplete")
        && count != "0"
    {
        evidence.push(format!("parse_incomplete={count}"));
    }
    if ndjson_field(ndjson, "cross_check") == Some("disagree") {
        evidence.push("cross_check=disagree".to_owned());
    }

    if evidence.is_empty() {
        return;
    }

    for (claim, caveat, name) in GATED_CLAIMS {
        if report.contains(claim) && !report.contains(caveat) {
            found.push(Correspondence::UncaveatedClaimUnderDoubt {
                claim: name,
                evidence: evidence.join(", "),
            });
        }
    }
}

#[cfg(test)]
mod tests;
