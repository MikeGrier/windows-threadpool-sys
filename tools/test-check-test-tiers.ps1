# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-test-tiers.ps1](check-test-tiers.ps1).

.DESCRIPTION
    The check guards the convention that lets a CI run select the tests that are too slow for every push
    (`long_haul::`) or not yet trusted (`probation::`) by name, while the ignore reason says which tier a test
    is in. A defect in it is quiet in the worst way: a test the run never selects is a test nothing runs, and
    the build stays green.

    Cases are BIDIRECTIONAL. A check that flagged every ignored test would pass "an unmarked test in the module
    is refused" and a check that flagged nothing would pass "an ordinary ignored test is accepted"; both are
    pinned, as is each lookalike spelling against the near-miss names that must NOT be flagged.

    The rules are driven through a recorded listing (`-ListingPath`), which needs no build. The end-to-end cases
    build a one-crate fixture workspace and ask its real test executable, because the JSON libtest writes is
    what the check reads, and it is not always valid JSON (a reason that names a path leaves its backslashes
    unescaped).

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$checker = Join-Path $PSScriptRoot 'check-test-tiers.ps1'
if (-not (Test-Path -LiteralPath $checker)) { throw "$checker not found beside this script" }

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $Message)
    if ($Expected -ne $Actual) { throw "$Message (expected '$Expected', got '$Actual')" }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $Message)
    if ($Text -notmatch $Pattern) { throw "$Message (no match for /$Pattern/ in: $Text)" }
}

function Assert-NoMatch {
    param([string] $Pattern, [string] $Text, [string] $Message)
    if ($Text -match $Pattern) { throw "$Message (unexpected /$Pattern/ in: $Text)" }
}

$script:roots = New-Object System.Collections.Generic.List[string]

function New-Root {
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ('test-tiers-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    $script:roots.Add($path) | Out-Null
    return $path
}

function Remove-Roots {
    foreach ($path in $script:roots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
    $script:roots.Clear()
}

# The messages are captured as strings and matched, so they must not wrap: on Windows PowerShell 5.1
# `Out-String` wraps captured host output at the host's width, which on a CI runner is narrow.
$captureWidth = 4096

function Invoke-Checker {
    param([hashtable] $Arguments)
    $text = & $checker @Arguments 6>&1 | Out-String -Width $captureWidth
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $text }
}

# A recorded listing: each test is @(name, ignored, reason).
function Invoke-OnListing {
    param([object[]] $Tests)
    $path = Join-Path (New-Root) 'listing.ndjson'
    $lines = foreach ($t in $Tests) {
        ConvertTo-Json -Compress -InputObject ([ordered]@{ binary = 'fixture'; name = $t[0]; ignore = [bool]$t[1]; ignore_message = [string]$t[2] })
    }
    [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n"))
    return Invoke-Checker @{ ListingPath = $path }
}

function Test-Flagged {
    param([object[]] $Tests, [string] $Pattern, [string] $Message)
    $r = Invoke-OnListing $Tests
    Assert-Equal 1 $r.Exit "$Message -- $($r.Output)"
    Assert-Match $Pattern $r.Output $Message
}

function Test-Accepted {
    param([object[]] $Tests, [string] $Message)
    $r = Invoke-OnListing $Tests
    Assert-Equal 0 $r.Exit "$Message -- $($r.Output)"
    Assert-Match 'Test tiers consistent' $r.Output $Message
}

# A one-crate workspace whose test executable is real. `Source` is the crate's lib.rs.
function New-Fixture {
    param([string] $Source, [string] $ExtraManifest = '')
    $root = Join-Path (New-Root) 'fixture'
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'src') | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $root 'Cargo.toml'), "[package]`nname = `"tiers-fixture`"`nversion = `"0.0.0`"`nedition = `"2021`"`n$ExtraManifest")
    [System.IO.File]::WriteAllText((Join-Path $root 'src\lib.rs'), $Source)
    Push-Location -LiteralPath $root
    try { & cargo generate-lockfile 2>&1 | Out-Null } finally { Pop-Location }
    return $root
}

$cases = @(
    @{
        Name = 'ordinary-tests-and-an-ordinarily-ignored-test-are-accepted'
        Run  = {
            Test-Accepted @(
                @('a', $false, ''),
                @('m::b', $false, ''),
                @('m::slow_for_another_reason', $true, 'needs the administrative share, which a CI runner has'),
                @('bench::timing', $true, 'run deliberately: a timing run')
            ) 'nothing is tiered'
        }
    },
    @{
        Name = 'a-long-haul-test-in-its-module-with-a-cost-is-accepted-and-counted'
        Run  = {
            $r = Invoke-OnListing @(
                @('long_haul::big_roster', $true, 'long-haul: about 3s'),
                @('outer::long_haul::nested::deep', $true, 'long-haul: about 9s'))
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'long-haul 2, probation 0' $r.Output 'the tier counts what is marked'
        }
    },
    @{
        Name = 'a-probation-test-in-its-module-with-a-cost-is-accepted-and-counted'
        Run  = {
            $r = Invoke-OnListing @(, @('probation::stress', $true, 'probation: never run in CI yet'))
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'long-haul 0, probation 1' $r.Output 'the tier counts what is marked'
        }
    },
    @{
        Name = 'a-marked-test-outside-its-module-is-refused'
        Run  = {
            Test-Flagged @(, @('tests::heavy', $true, 'long-haul: about 3s')) 'not in a module named long_haul' 'nothing would select it'
            Test-Flagged @(, @('plain_test', $true, 'probation: new')) 'not in a module named probation' 'a bare test has no module at all'
        }
    },
    @{
        Name = 'the-module-must-be-an-exact-segment-of-the-parents-and-not-the-test-itself'
        Run  = {
            Test-Flagged @(, @('outer::long_haul', $true, 'long-haul: x')) 'not in a module named long_haul' 'the test`s own name is not a module, and the filter `long_haul::` does not select it'
            Test-Flagged @(, @('not_long_haul::t', $true, 'long-haul: x')) 'not in a module named long_haul' 'a longer module name is not the module'
        }
    },
    @{
        Name = 'the-module-name-is-matched-exactly-as-the-filter-matches-it'
        Run  = {
            Test-Flagged @(, @('Long_Haul::t', $true, 'long-haul: x')) 'not in a module named long_haul' 'libtest`s filter is case-sensitive, so a differently cased module is never selected'
            Test-Flagged @(, @('PROBATION::t', $true, 'probation: x')) 'not in a module named probation' 'and so is the other tier'
        }
    },
    @{
        Name = 'a-marker-with-no-cost-after-it-is-refused'
        Run  = {
            Test-Flagged @(, @('long_haul::t', $true, 'long-haul:')) 'says nothing after' 'the bare marker'
            Test-Flagged @(, @('probation::t', $true, 'probation:    ')) 'says nothing after' 'the marker and whitespace'
        }
    },
    @{
        Name = 'an-unmarked-test-in-a-tier-module-is-refused-whether-or-not-it-is-ignored'
        Run  = {
            Test-Flagged @(, @('long_haul::t', $true, 'needs an admin share')) 'ignored for another reason' 'ignored for a reason that is not its own'
            Test-Flagged @(, @('long_haul::t', $false, '')) 'is not ignored' 'it would run on every push'
            Test-Flagged @(, @('probation::t', $true, '')) 'ignored for another reason' 'ignored with no reason at all'
        }
    },
    @{
        Name = 'a-path-that-only-contains-the-filter-is-refused-as-an-accidental-selection'
        Run  = {
            Test-Flagged @(, @('not_long_haul::t', $false, '')) 'would select it by accident' 'the filter matches a longer module name'
            Test-Flagged @(, @('a_probation::t', $true, 'needs admin')) 'would select it by accident' 'and so does the other tier'
        }
    },
    @{
        Name = 'a-reason-that-reads-like-a-marker-but-is-not-spelled-as-one-is-refused'
        Run  = {
            foreach ($reason in @('Long-haul: x', 'long haul: x', 'long_haul: x', 'longhaul: x', 'LONG-HAUL x', 'Probation: x', 'on probation')) {
                Test-Flagged @(, @('some::test', $true, $reason)) 'reads like a' "the reason `"$reason`" is a misspelt marker"
            }
        }
    },
    @{
        Name = 'names-that-merely-mention-a-tier-are-not-flagged'
        Run  = {
            Test-Accepted @(
                @('a_user_on_probation', $false, ''),
                @('long_haul_is_documented', $false, ''),
                @('m::probation_is_not_a_module_here', $false, ''),
                @('m::t', $true, 'needs a long-lived server; run deliberately')
            ) 'only an ignore reason or a module segment is a marker'
        }
    },
    @{
        Name = 'every-problem-is-reported-and-counted'
        Run  = {
            $r = Invoke-OnListing @(
                @('long_haul::a', $false, ''),
                @('tests::b', $true, 'probation: x'),
                @('fine', $false, ''))
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match '2 test tier problem' $r.Output 'both are counted'
            Assert-Match 'long_haul::a' $r.Output 'the first is named'
            Assert-Match 'tests::b' $r.Output 'the second is named'
            Assert-NoMatch '\bfine\b' $r.Output 'a conforming test is not mentioned'
        }
    },
    @{
        Name = 'a-listing-that-is-missing-is-a-census-that-could-not-be-taken'
        Run  = {
            $r = Invoke-Checker @{ ListingPath = (Join-Path (New-Root) 'absent.ndjson') }
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Match 'No listing' $r.Output 'it says what is missing'
        }
    },
    @{
        Name = 'a-real-workspace-with-tiered-tests-is-accepted-including-a-reason-that-names-a-path'
        Run  = {
            $root = New-Fixture @'
#[test] fn ordinary() {}
#[test]
#[ignore = r"needs the administrative share \\localhost\C$, which a CI runner has"]
fn names_a_path() {}
mod long_haul {
    #[test]
    #[ignore = "long-haul: about 3s"]
    fn heavy() {}
    #[test]
    #[ignore = r"long-haul: reads \\localhost\C$, about 3s"]
    fn heavy_and_names_a_path() {}
}
mod probation {
    #[test]
    #[ignore = "probation: never run in CI yet"]
    fn untried() {}
}
'@
            $r = Invoke-Checker @{ RepositoryRoot = $root }
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match '5 tests' $r.Output 'it saw all five, including the two whose lines are not valid JSON'
            Assert-Match 'long-haul 2, probation 1' $r.Output 'and read the marked one that names a path as marked, ignored and costed'
        }
    },
    @{
        Name = 'a-real-workspace-with-an-unmarked-test-in-a-tier-module-is-refused'
        Run  = {
            $root = New-Fixture @'
mod long_haul {
    #[test]
    fn heavy_but_not_ignored() {}
}
'@
            $r = Invoke-Checker @{ RepositoryRoot = $root }
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match 'long_haul::heavy_but_not_ignored' $r.Output 'it names the test'
            Assert-Match 'is not ignored' $r.Output 'and why'
        }
    },
    @{
        Name = 'a-target-with-a-custom-harness-is-a-census-that-could-not-be-taken'
        Run  = {
            $root = New-Fixture '#[test] fn t() {}' "`n[[test]]`nname = `"custom`"`npath = `"src/lib.rs`"`nharness = false`n"
            $r = Invoke-Checker @{ RepositoryRoot = $root }
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Match 'harness = false' $r.Output 'it names the declaration'
        }
    },
    @{
        Name = 'tests-that-do-not-build-are-a-census-that-could-not-be-taken'
        Run  = {
            $root = New-Fixture 'this is not rust'
            $r = Invoke-Checker @{ RepositoryRoot = $root }
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Match 'could not build' $r.Output 'it says the build failed'
        }
    }
)

$selected = $cases | Where-Object { $_.Name -like $Name }
if (-not $selected) {
    Write-Host "No cases matched '$Name'." -ForegroundColor Red
    exit 1
}

$failures = 0
foreach ($case in $selected) {
    try {
        & $case.Run
        Write-Host "  PASS  $($case.Name)" -ForegroundColor Green
    }
    catch {
        $failures++
        Write-Host "  FAIL  $($case.Name)" -ForegroundColor Red
        Write-Host "        $($_.Exception.Message)" -ForegroundColor Red
    }
    finally {
        Remove-Roots
    }
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "test-check-test-tiers: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-check-test-tiers: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
