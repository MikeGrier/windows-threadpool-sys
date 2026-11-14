# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [report-io-test-tail.ps1](report-io-test-tail.ps1). Exits 0 only if every case
    passes.

.DESCRIPTION
    The report's one claim is whether a slow test execution overlapped a disk stall, which rests on
    joining libtest's UTC event times to typeperf's local-time samples. Each case writes a probe
    directory with a known stall and a known slow execution and asserts the verdict, in both
    directions: a slow execution over a stall overlaps; one away from it, or under only the
    `_Total` average, does not; a fast execution is not slow at all. The stall in every case is
    reachable only through the header's time-zone bias, so a join that ignores it overlaps nothing.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1. The script itself needs PowerShell 7.
#>
#Requires -Version 7
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Script = Join-Path $PSScriptRoot 'report-io-test-tail.ps1'
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

function Assert-Reports {
    param([string] $Output, [string] $Want)
    if ($Output -notmatch [regex]::Escape($Want)) { throw "report lacks '$Want':`n$Output" }
}

# UTC 21:00:00 is local 16:00:00 at the bias of 300 minutes the counters' header carries.
$script:Utc0 = [DateTime]::new(2026, 11, 13, 21, 0, 0, [DateTimeKind]::Utc)
$script:Bias = 300

# A probe directory: 100 one-second samples, every latency 1 ms except the drive `Stalls` (second ->
# seconds of latency) and the `_Total` column's `TotalStalls`; and test executions, each a
# (test, end second, exec seconds).
function Invoke-Report {
    param([hashtable] $Stalls = @{}, [hashtable] $TotalStalls = @{}, [object[]] $Executions)
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) "report-io-test-tail-$PID-$([guid]::NewGuid().ToString('N'))"
    try {
        New-Item -ItemType Directory -Force $dir | Out-Null
        $csv = @('"(PDH-CSV 4.0) (Eastern Standard Time)(300)","\\H\LogicalDisk(C:)\Avg. Disk sec/Write","\\H\LogicalDisk(_Total)\Avg. Disk sec/Write","\\H\LogicalDisk(C:)\Current Disk Queue Length","\\H\Processor(_Total)\% Processor Time"')
        for ($s = 0; $s -lt 100; $s++) {
            $local = $script:Utc0.AddSeconds($s).AddMinutes(-$script:Bias).ToString('MM/dd/yyyy HH:mm:ss.fff', [Globalization.CultureInfo]::InvariantCulture)
            $drive = if ($Stalls.ContainsKey($s)) { $Stalls[$s] } else { 0.001 }
            $total = if ($TotalStalls.ContainsKey($s)) { $TotalStalls[$s] } else { 0.001 }
            $csv += "`"$local`",`"$drive`",`"$total`",`"0`",`"5`""
        }
        Set-Content -LiteralPath (Join-Path $dir 'counters.csv') -Value $csv
        $events = @("at`titeration`tbinary`tline")
        $phases = @("iteration`tphase`tstart`tend`tcode")
        $i = 0
        foreach ($e in $Executions) {
            $i++
            $end = $script:Utc0.AddSeconds($e[1])
            $events += "$($end.ToString('o'))`t$i`tbin`t{ `"type`": `"test`", `"name`": `"$($e[0])`", `"event`": `"ok`", `"exec_time`": $($e[2]) }"
            $phases += "$i`tbin`t$($end.AddSeconds(-$e[2]).ToString('o'))`t$($end.ToString('o'))`t0"
        }
        Set-Content -LiteralPath (Join-Path $dir 'events.tsv') -Value $events
        Set-Content -LiteralPath (Join-Path $dir 'phases.tsv') -Value $phases
        '{ "mode": "steady" }' | Set-Content -LiteralPath (Join-Path $dir 'machine.json')
        return ((& $script:Script -Path $dir -StallPercentile 0.995 6>&1) -join "`n")
    }
    finally { Remove-Item -Recurse -Force -LiteralPath $dir -ErrorAction SilentlyContinue }
}

# Five quick executions of `t`, so its median is 0.01 s.
$script:Quick = @(1..5 | ForEach-Object { , @('t', (2 * $_), 0.01) })

Write-Report 'test-report-io-test-tail:'

Test-Case 'a slow execution over a stall sample overlaps it' {
    $out = Invoke-Report -Stalls @{ 50 = 0.5 } -Executions ($script:Quick + , @('t', 52, 4.0))
    Assert-Reports $out 'Slow executions (>= 5x the test''s median and >= 1s): 1 of 6'
    Assert-Reports $out 'Overlapping a disk stall: 1 of 1'
    Assert-Reports $out 'With no counter sample in the window: 0.'
}

Test-Case 'a slow execution away from the stall does not overlap it' {
    $out = Invoke-Report -Stalls @{ 50 = 0.5 } -Executions ($script:Quick + , @('t', 82, 4.0))
    Assert-Reports $out 'Overlapping a disk stall: 0 of 1'
}

Test-Case 'a stall only in the _Total average is not a stall' {
    $out = Invoke-Report -Stalls @{ 90 = 0.5 } -TotalStalls @{ 50 = 0.5 } -Executions ($script:Quick + , @('t', 52, 4.0))
    Assert-Reports $out 'Overlapping a disk stall: 0 of 1'
}

Test-Case 'a fast execution over a stall is not slow' {
    $out = Invoke-Report -Stalls @{ 50 = 0.5 } -Executions ($script:Quick + , @('t', 50, 0.02))
    Assert-Reports $out 'Slow executions (>= 5x the test''s median and >= 1s): 0 of 6'
}

Test-Case 'an execution over the floor but under the factor is not slow' {
    $steady = @(1..5 | ForEach-Object { , @('u', (2 * $_ + 2), 2.0) })
    $out = Invoke-Report -Stalls @{ 50 = 0.5 } -Executions ($steady + , @('u', 52, 4.0))
    Assert-Reports $out 'Slow executions (>= 5x the test''s median and >= 1s): 0 of 6'
}

if ($script:Failures -gt 0) {
    Write-Report "test-report-io-test-tail: $script:Failures case(s) failed." -Level bad
    exit 1
}
Write-Report 'test-report-io-test-tail: all cases passed.' -Level good
