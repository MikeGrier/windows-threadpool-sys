# Copyright (c) Mike Grier
#
# tools/check-compile-fail-doctests.ps1 -- every compile_fail doctest RUSTDOC
# runs is pinned, and is one check-compile-fail-codes.ps1 recognises.
#
# WHY. check-compile-fail-codes.ps1 finds compile_fail fences with a line
# pattern, which only approximates rustdoc's parser. Four PR #113 review rounds
# each found a spelling it missed -- fences nested in containers, tilde fences,
# block doc comments, raw-string doc attributes -- and every miss was an
# unpinned doctest CI would have passed. This check takes the set from rustdoc
# instead. A doctest run names each one as
#
#     test <path> - <item> (line N) - compile fail ... ok
#
# whatever spelling produced it, including Markdown pulled in by include_str!.
#
# What rustdoc does NOT give is the info string, and its line N is the fence
# line for `///` and Markdown but the attribute's first line for a
# `doc = "..."` attribute (both measured). So each reported doctest claims the
# first compile_fail fence at or after line N in its file that no other doctest
# has claimed, and then:
#
#   - nothing to claim        FAIL  a doc built by concat! or a macro, whose
#                                   info string cannot be read from source;
#   - the guard does not
#     recognise the fence     FAIL  a blind spot in the fast guard;
#   - the guard calls it
#     unpinned                FAIL  the doctest passes on any compile error.
#
# "Recognised" and "pinned" are the guard's own answers, asked through its
# -ListFences mode, so they are defined in exactly one place.
#
# Input is the plain-text output of `cargo test --doc` runs. CI runs three
# feature configurations because no single one reaches every compile_fail
# doctest; one listed in several runs is checked once.
#
#   ./tools/check-compile-fail-doctests.ps1 -Transcript a.txt, b.txt, c.txt
#   ./tools/check-compile-fail-doctests.ps1 -Transcript t.txt -Root DIR -ScanRoot DIR
#
# Exit 0 when every doctest passes, 1 on any failure, 2 on a configuration
# error -- including transcripts that name no compile_fail doctest at all,
# which would otherwise pass by checking nothing.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string[]]$Transcript,
    # What rustdoc's paths are relative to: the workspace root.
    [string]$Root,
    # What the guard scans; defaults to crates/ under -Root, as in CI.
    [string]$ScanRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'note')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'note' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Exit-Config {
    param([string] $Message)
    Write-Report "CONFIG ERROR: $Message" -Level bad
    exit 2
}

if (-not $Root) { $Root = Split-Path -Parent $PSScriptRoot }
if (-not (Test-Path -LiteralPath $Root)) { Exit-Config "root not found: $Root" }
$Root = (Resolve-Path -LiteralPath $Root).Path
if (-not $ScanRoot) { $ScanRoot = Join-Path $Root 'crates' }
foreach ($path in $Transcript) {
    if (-not (Test-Path -LiteralPath $path)) { Exit-Config "transcript not found: $path" }
}

# --- What rustdoc ran --------------------------------------------------------

# `- compile fail` is rustdoc's own label for the kind; `- compile` alone is a
# no_run doctest and must not match.
$entryPattern = [regex]'^test (?<path>.+?\.(?:rs|md)) - .*\(line (?<line>\d+)\) - compile fail\b'

# Keyed by the doctest's name. A doctest appears once per configuration that
# compiles it, so the count kept is the largest any single run reports, not
# the sum: two doctests can share a name (two fences in one attribute), and
# that is visible only within a run.
$doctests = @{}
foreach ($path in $Transcript) {
    $seen = @{}
    foreach ($line in [System.IO.File]::ReadAllLines((Resolve-Path -LiteralPath $path).Path)) {
        $match = $entryPattern.Match($line)
        if (-not $match.Success) { continue }
        $name = $line.Substring(5, $line.IndexOf(' - compile fail') - 5)
        if (-not $seen.ContainsKey($name)) {
            $seen[$name] = [pscustomobject]@{
                Name  = $name
                File  = [System.IO.Path]::GetFullPath((Join-Path $Root $match.Groups['path'].Value))
                Line  = [int]$match.Groups['line'].Value
                Count = 0
            }
        }
        $seen[$name].Count++
    }
    foreach ($entry in $seen.Values) {
        if (-not $doctests.ContainsKey($entry.Name) -or $doctests[$entry.Name].Count -lt $entry.Count) {
            $doctests[$entry.Name] = $entry
        }
    }
}

$total = 0
foreach ($entry in $doctests.Values) { $total += $entry.Count }
if ($total -eq 0) {
    Exit-Config ('the transcripts name no compile_fail doctest. Either they are not ' +
        '`cargo test --doc` output, or rustdoc ran none -- and a check that checked ' +
        'nothing must not pass.')
}

# --- What the guard recognises -----------------------------------------------

$guardScript = Join-Path $PSScriptRoot 'check-compile-fail-codes.ps1'
$records = @(& $guardScript -Root $ScanRoot -ListFences)
if ($LASTEXITCODE -ne 0) { Exit-Config "check-compile-fail-codes.ps1 -ListFences exited $LASTEXITCODE" }

# file<TAB>line -> the pinned verdict of each recognised fence on that line, in
# order; a one-line cfg_attr can carry more than one.
$recognised = @{}
foreach ($record in $records) {
    $fields = "$record".Split("`t")
    $key = "$($fields[0].ToLowerInvariant())`t$($fields[1])"
    if (-not $recognised.ContainsKey($key)) {
        $recognised[$key] = New-Object System.Collections.Generic.List[bool]
    }
    $recognised[$key].Add($fields[2] -eq 'True') | Out-Null
}

# --- Claim each doctest's fence ----------------------------------------------

# Deliberately looser than the guard: any line holding a fence opener followed
# by `compile_fail`. It never decides anything on its own -- rustdoc says a
# doctest is there, and the guard says whether it is recognised and pinned.
$looseFence = [regex]'(?:`{3,}|~{3,})[^`\r\n]*compile_fail'

$sources = @{}
$claimed = @{}
$failures = New-Object System.Collections.Generic.List[string]

$ordered = $doctests.Values | Sort-Object File, Line
foreach ($entry in $ordered) {
    if (-not $sources.ContainsKey($entry.File)) {
        if (-not (Test-Path -LiteralPath $entry.File)) {
            $failures.Add("$($entry.Name)`n      rustdoc names a file that does not exist: $($entry.File)") | Out-Null
            continue
        }
        $sources[$entry.File] = [System.IO.File]::ReadAllLines($entry.File)
    }
    $lines = $sources[$entry.File]

    for ($k = 0; $k -lt $entry.Count; $k++) {
        $found = 0
        for ($i = $entry.Line; $i -le $lines.Length; $i++) {
            $here = "$($entry.File.ToLowerInvariant())`t$i"
            $onLine = $looseFence.Matches($lines[$i - 1]).Count
            $taken = if ($claimed.ContainsKey($here)) { $claimed[$here] } else { 0 }
            if ($onLine -gt $taken) {
                $claimed[$here] = $taken + 1
                $found = $i
                break
            }
        }
        $where = "$($entry.Name)"
        if ($found -eq 0) {
            $failures.Add("$where`n      no compile_fail fence at or after line $($entry.Line) to read its info " +
                'string from -- a doc built by concat! or a macro cannot be checked from source') | Out-Null
            continue
        }
        $key = "$($entry.File.ToLowerInvariant())`t$found"
        $index = $claimed[$key] - 1
        if (-not $recognised.ContainsKey($key) -or $recognised[$key].Count -le $index) {
            $failures.Add("$where`n      the fence on line $found is one check-compile-fail-codes.ps1 does " +
                'not recognise: a blind spot in that guard, so fix its pattern and add a fixture') | Out-Null
            continue
        }
        if (-not $recognised[$key][$index]) {
            $failures.Add("$where`n      the fence on line $found names no error code, so the example passes " +
                'on ANY compile error') | Out-Null
        }
    }
}

if ($failures.Count -eq 0) {
    Write-Report ("Every compile_fail doctest rustdoc ran ($total) is pinned and recognised by " +
        'check-compile-fail-codes.ps1.') -Level good
    exit 0
}

Write-Report ''
Write-Report "$($failures.Count) of $total compile_fail doctests rustdoc ran fail this check:" -Level bad
foreach ($failure in $failures) { Write-Report "  $failure" }
Write-Report ''
exit 1
