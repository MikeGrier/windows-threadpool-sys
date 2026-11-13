# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [measure-sweep-durations.ps1](measure-sweep-durations.ps1). Exits 0 only if every
    case passes.

.DESCRIPTION
    The script places a hang bound by the durations it reads, so a row it keeps that is not a
    full-suite launch, or one it drops that is, moves the bound. Each case drives the real script
    offline over fixture logs written in the launcher's wording and asserts the rows it exports, in
    both directions: build launches and credited narrowed runs are left out; baselines, full-suite
    entry launches, launches killed at the bound, and the suite that follows an uncredited narrowed
    run are kept.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell 5.1 ships Pester 3,
    whose syntax differs incompatibly from Pester 5. The script itself needs PowerShell 7.
#>
#Requires -Version 7
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Script = Join-Path $PSScriptRoot 'measure-sweep-durations.ps1'
$script:Failures = 0

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Test-Case {
    param([string] $Name, [scriptblock] $Body)
    try {
        & $Body
        Write-Report "  PASS  $Name" -Level good
    }
    catch {
        $script:Failures++
        Write-Report "  FAIL  $Name" -Level bad
        Write-Report "        $($_.Exception.Message)"
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $What)
    if ("$Expected" -ne "$Actual") { throw "$What`: expected '$Expected', got '$Actual'" }
}

# A launch as win-job-launcher traces it, then the result file run-sabotage.ps1 had it write.
function Get-Launch {
    param([string] $Stem, [int] $Ms, [string] $Code = '0', [switch] $Build)
    $end = if ($Code -eq 'bound') { "the bound of ${Ms}ms was reached; job: 8 active of 42 process(es)" }
    else { "the command exited with code $Code after ${Ms}ms; job: 0 active of 59 process(es)" }
    $suffix = if ($Build) { '.txt.build.result' } else { '.txt.result' }
    @(
        '2026-11-13T21:38:00.0000000Z win-job-launcher +0.000s: created a kill-on-close job'
        "2026-11-13T21:38:01.0000000Z win-job-launcher +1.000s: $end"
        "2026-11-13T21:38:01.0000000Z win-job-launcher +1.000s: wrote D:\a\repo\.scratch\sabotage\$Stem$suffix"
    )
}

# Runs the script over one fixture log of `Lines` for a manifest whose entries declare `Expect`,
# and returns the exported rows and what it reported.
function Invoke-Measure {
    param([string[]] $Lines, [hashtable] $Expect = @{}, [string] $JobName = 'sweep fixture 3/7')
    $root = Join-Path ([System.IO.Path]::GetTempPath()) "measure-sweep-durations-$PID-$([guid]::NewGuid().ToString('N'))"
    try {
        $crate = Join-Path $root 'crates/fixture'
        New-Item -ItemType Directory -Force $crate, (Join-Path $root 'logs') | Out-Null
        $entries = @($Expect.Keys | ForEach-Object { @{ name = $_; expect = $Expect[$_]; file = 'src/lib.rs'; find = @('a'); replace = @('b') } })
        @{ package = 'fixture'; sabotages = $entries } | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $crate 'sabotage.json')
        $head = "# run=100 runAt=2026-11-13T21:36:35Z job=200 name=$JobName conclusion=failure runner=GitHub Actions 1"
        Set-Content -LiteralPath (Join-Path $root 'logs/100-200.txt') -Value (@($head) + $Lines)
        $csv = Join-Path $root 'rows.csv'
        $output = & $script:Script -Offline -Manifest (Join-Path $crate 'sabotage.json') -LogDirectory (Join-Path $root 'logs') -Csv $csv 6>&1
        if (-not (Test-Path -LiteralPath $csv)) { throw "no CSV written; output: $($output -join ' | ')" }
        return [pscustomobject]@{ Rows = @(Import-Csv -LiteralPath $csv); Output = ($output -join "`n") }
    }
    finally { Remove-Item -Recurse -Force -LiteralPath $root -ErrorAction SilentlyContinue }
}

Write-Report 'test-measure-sweep-durations:'

Test-Case 'a baseline is kept and its build launch is not' {
    $rows = (Invoke-Measure -Lines @(
        (Get-Launch -Stem baseline -Ms 24030 -Build)
        (Get-Launch -Stem baseline -Ms 6950)
        '2026-11-13T21:38:02.0000000Z Baseline is green in 8s. Hang bound: 24s (3x the 8s baseline, floor 15s).'
    )).Rows
    Assert-Equal 1 $rows.Count 'rows'
    Assert-Equal 'baseline' $rows[0].Kind 'kind'
    Assert-Equal 6950 $rows[0].Ms 'ms'
    Assert-Equal 3 $rows[0].Shard 'shard'
}

Test-Case 'an entry caught by its narrowed run is left out' {
    $rows = (Invoke-Measure -Expect @{ 'E1: x' = 'caught' } -Lines @(
        '2026-11-13T21:38:02.0000000Z Baseline is green in 8s. Hang bound: 24s (3x the 8s baseline, floor 15s).'
        '[  1/1] E1: x'
        (Get-Launch -Stem E1-x -Ms 22000 -Build)
        (Get-Launch -Stem E1-x -Ms 3172 -Code 101)
        '[  1/1] E1: x caught (its recorded tests failed, exit 101) (25s, ~4m left)'
    )).Rows
    Assert-Equal 0 $rows.Count 'rows'
}

Test-Case 'an entry that survives is kept with its suite duration and declared expectation' {
    $rows = (Invoke-Measure -Expect @{ 'E2: y' = 'survives' } -Lines @(
        '2026-11-13T21:38:02.0000000Z Baseline is green in 8s. Hang bound: 24s (3x the 8s baseline, floor 15s).'
        '[  1/1] E2: y'
        (Get-Launch -Stem E2-y -Ms 21000 -Build)
        (Get-Launch -Stem E2-y -Ms 7700)
        '[  1/1] E2: y survived (NOT caught) (29s)'
    )).Rows
    Assert-Equal 1 $rows.Count 'rows'
    Assert-Equal 'entry' $rows[0].Kind 'kind'
    Assert-Equal 7700 $rows[0].Ms 'ms'
    Assert-Equal 'survives' $rows[0].Expect 'expect'
    Assert-Equal 'survived (NOT caught)' $rows[0].Verdict 'verdict'
    Assert-Equal 24 $rows[0].Bound 'bound'
}

Test-Case 'a launch killed at the bound is kept as a bound row' {
    $rows = (Invoke-Measure -Expect @{ 'E3: z' = 'survives' } -Lines @(
        '2026-11-13T21:38:02.0000000Z Baseline is green in 8s. Hang bound: 24s (3x the 8s baseline, floor 15s).'
        '[  1/1] E3: z'
        (Get-Launch -Stem E3-z -Ms 21835 -Build)
        (Get-Launch -Stem E3-z -Ms 24000 -Code bound)
        '[  1/1] E3: z caught (tests HUNG past 24s) (46s, ~1m left)'
    )).Rows
    Assert-Equal 1 $rows.Count 'rows'
    Assert-Equal 'bound' $rows[0].Code 'code'
    Assert-Equal 24000 $rows[0].Ms 'ms'
    Assert-Equal 'caught (tests HUNG past 24s)' $rows[0].Verdict 'verdict'
}

Test-Case 'after an uncredited narrowed run, the suite that follows is the row' {
    $rows = (Invoke-Measure -Expect @{ 'E4: w' = 'caught' } -Lines @(
        '2026-11-13T21:38:02.0000000Z Baseline is green in 8s. Hang bound: 24s (3x the 8s baseline, floor 15s).'
        '[  1/1] E4: w'
        (Get-Launch -Stem E4-w -Ms 21000 -Build)
        (Get-Launch -Stem E4-w -Ms 3000)
        (Get-Launch -Stem E4-w -Ms 16396 -Code 101)
        '[  1/1] E4: w caught (suite failed, exit 101) (46s, ~1m left)'
    )).Rows
    Assert-Equal 1 $rows.Count 'rows'
    Assert-Equal 16396 $rows[0].Ms 'ms'
    Assert-Equal '101' $rows[0].Code 'code'
}

Test-Case 'a log for a manifest not asked about is ignored' {
    $rows = (Invoke-Measure -JobName 'sweep other-crate 3/7' -Lines @((Get-Launch -Stem baseline -Ms 6950))).Rows
    Assert-Equal 0 $rows.Count 'rows'
}

Test-Case 'the reported percentiles are nearest-rank over the baselines' {
    # 0.1 s to 100.1 s in 0.1 s steps. 1,001 samples, so p x n is fractional and a floored rank
    # gives a different sample: ceiling(500.5) = 501st is 50.1 s, ceiling(999.999) = 1,000th is 100.0 s.
    $result = Invoke-Measure -Lines @(1..1001 | ForEach-Object { Get-Launch -Stem baseline -Ms ($_ * 100) })
    Assert-Equal 1001 $result.Rows.Count 'rows'
    foreach ($want in 'p50 50.1s', 'p99.9 100.0s', 'max 100.1s', 'median + pooled p99.9 = 100.0s') {
        if ($result.Output -notmatch [regex]::Escape($want)) { throw "report lacks '$want':`n$($result.Output)" }
    }
}

if ($script:Failures -gt 0) {
    Write-Report "test-measure-sweep-durations: $script:Failures case(s) failed." -Level bad
    exit 1
}
Write-Report 'test-measure-sweep-durations: all cases passed.' -Level good
