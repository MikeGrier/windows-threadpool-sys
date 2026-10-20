# Design notes: win-text-sys

## <a id="wt-d-1"></a>WT-D-1: the crate is named for its subject, text, and `CompareStringOrdinal` is the first function filed in it

**The name is the engineer's, 2026-10-18.** Asked which name the crate should take for the ordinal string
comparison, the engineer said "I lean towards win-text-sys for the crate name. locale implies that text handling
that was not specific to a locale would not belong", and then "win-text-sys is the name of the crate unless you
have a solid argument against it". None was found. The name was unclaimed on crates.io when it was checked that
day. The workspace's `win-` prefix rule applies, as the crate is a safe layer more or less directly over Win32
([DESIGN-NOTES.md](../../DESIGN-NOTES.md#new-crates-take-the-win-prefix)).

**The section.** By [the placement rule](../../DESIGN-NOTES.md#safe-wrappers-are-placed-by-the-learn-section-they-are-documented-under),
the snapshot [WIN32-LEARN-MAP-DATA.txt](../../WIN32-LEARN-MAP-DATA.txt) (2026-10-17) files `CompareStringOrdinal`
once, under *Desktop App Technologies > Desktop App User Interface > Internationalization for Windows Applications >
Globalization Services > National Language Support > National Language Support Reference > National Language Support
Functions*. That is this crate's section. The snapshot files a second area under the same Internationalization
heading, *Unicode and Character Sets*, which holds functions of text that is not tied to a locale; it is not this
crate's by the rule, and a wrapper from it is judged by the crate's subject: it belongs here if it is a function of
text, whichever section lists it. The name is the subject whether or not the text is particular to a locale, and
neither Microsoft's section title nor its header appears in a name or an identifier, as the rule requires.

**What a `-sys` crate here is.** It is memory-safe over the call and adds no policy: it passes the arguments as
given, reports what the call reports, and refuses ahead of the call only what the call's page makes
impossible to pass ([WT-D-2](#wt-d-2)).

**Scheduled work, and none beyond it.** The crate has no checklist of its own: its one function is
[M43.1](../../COMPLETED-CHECKLIST.md#m431) in the workspace plan, and the two sites that call `CompareStringOrdinal` themselves
move to it in `SAFE-4` of [windows-file-enumeration-sys' COMPLETED-CHECKLIST.md](../windows-file-enumeration-sys/COMPLETED-CHECKLIST.md#safe-4)
and `M17.1` of [windows-file-watcher's COMPLETED-CHECKLIST.md](../windows-file-watcher/COMPLETED-CHECKLIST.md#m171). The other functions
the section lists, such as `CompareStringEx` and `FindStringOrdinal`, are outside the item that made the crate.

## <a id="wt-d-2"></a>WT-D-2: `compare_string_ordinal` takes two slices and a case and gives an `Ordering`, with the call's failure and the crate's one refusal as distinct errors

Read 2026-10-20 from the call's page (`ms.date` 2018-12-05, `updated_at` 2024-02-22).

**The shape.** Two `&[u16]` and a `Case`, which is `bIgnoreCase`: `Sensitive` is `FALSE`, and `Insensitive` is `TRUE`,
which is 1, "the only numeric value that can be used to specify a true value for this boolean parameter that does not
result an invalid parameter error", so no other value can be passed and the type cannot make that invalid call. The
result is an `Ordering`, from the page's `CSTR_LESS_THAN`, `CSTR_EQUAL` and `CSTR_GREATER_THAN`.

**The count is always passed.** The page gives `-1` for a NUL-terminated string, and a slice is not one, so every
call passes the slice's length. A NUL in the data is then a unit and the comparison of `"a\0b"` and `"a\0c"` is of
three units each, where the `-1` form would compare the one-unit string `"a"` with itself. The same holds for an
unpaired surrogate, which is a unit.

**The one refusal ahead of the call, and what it rests on.** A slice of more than `i32::MAX` units is refused as
`CompareStringOrdinalError::TooLong`, naming the first operand when both are too long. The citation is the page's
syntax block, which types `cchCount1` and `cchCount2` as `int`, and its parameter text, "Length of the string indicated
by *lpString1*". It is on the arguments alone. It is a limit of the call's signature and not a condition the page calls
invalid, so it is this crate's refusal and is kept distinct from a failure the call reports, in an error of this
crate's own rather than `std::io::Error`: `ERROR_INVALID_PARAMETER`, which the page lists for the call, is
`ErrorKind::InvalidInput` in std, and a refusal given that kind could not be told from the call's failure. The
consumer decides what a string that long means for it; the two sites this was made for compare names, and each
chose a fallback of its own that this crate does not take on.

**Nothing else is checked ahead of the call.** The page's one failure, `ERROR_INVALID_PARAMETER` "Any of the
parameter values was invalid", is the call's to give, and it reaches the caller as `CompareStringOrdinalError::Call`
with the code as a `Win32Error`. The code is read immediately after the call. The match arm for a result the
page does not list, neither of the three nor zero, reports a failure with code zero, which says Windows gave none, as
`windows-file-enumeration-sys`' [D-24](../windows-file-enumeration-sys/DESIGN-NOTES.md#d-24) does for its own
unreachable state; the mapping is a function of the result alone and is tested in memory over values the call
does not return.

**What the tests are, and are not.** The call's failure is made by passing `bIgnoreCase` as 2, which the page calls an
invalid parameter, directly to the private call; the safe function cannot pass it. That the comparison is ordinal
and not linguistic (`"\u00df"` against `"ss"`, an `e` with a combining accent against the one-unit letter), and that
its order is that of the units and not of code points, are evidence of what was observed and not a contract this
crate states: the page says "binary equality" and gives no ordering rule, so the crate's documentation says the
same and no check ahead of the call depends on either.
