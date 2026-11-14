# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Reads the output of [probe-io-test-tail.ps1](probe-io-test-tail.ps1) and says which tests carry
    the duration tail and whether each slow execution overlaps a disk-latency stall.

.DESCRIPTION
    A test execution ends at the arrival time of its libtest `ok`/`failed` event and began
    `exec_time` before that. It is SLOW when `exec_time` is at least -SlowFactor times that test's
    median and at least -SlowFloorSeconds. The disk latency of a counter sample is the largest
    `Avg. Disk sec/Write` or `Avg. Disk sec/Read` over the drives (the `_Total` instance is an
    average and is left out); a sample is a STALL when that is at or above the probe directory's own
    -StallPercentile.

    For each slow execution the report gives the largest latency sampled within its interval
    (widened by one second each side, for the one-second sampling) and whether that is a stall. It
    also gives how many slow executions would overlap a stall by chance: for each one, the fraction
    of windows of its length over the whole counter series that contain a stall, summed. Overlaps far
    above that expectation say the slow executions coincide with disk stalls; overlaps near it say
    they do not.

    typeperf stamps samples in the machine's local time and records the bias in its header; times
    are converted to UTC with it before they are compared with the events.

.PARAMETER Path
    One or more probe output directories, or directories containing them (searched for
    `events.tsv`).
#>
#Requires -Version 7
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string[]] $Path,
    [double] $SlowFactor = 5,
    [double] $SlowFloorSeconds = 1,
    [double] $StallPercentile = 0.99,
    [int] $List = 40
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message)
    Write-Host $Message
}

function Get-Percentile {
    param([double[]] $Sorted, [double] $P)
    if ($Sorted.Count -eq 0) { return [double]::NaN }
    $at = [Math]::Ceiling($P * $Sorted.Count) - 1
    return $Sorted[[Math]::Max(0, [Math]::Min($Sorted.Count - 1, $at))]
}

# typeperf's CSV as (Utc, Latency, Queue, Cpu, Defender) per sample, in time order.
function Read-Counters {
    param([string] $File)
    $lines = @(Get-Content -LiteralPath $File)
    if ($lines.Count -lt 2) { return @() }
    $header = @($lines[0] | ConvertFrom-Csv -Header (1..1000) | Select-Object -First 1 |
            ForEach-Object { $_.PSObject.Properties.Value } | Where-Object { $null -ne $_ })
    if ($header[0] -notmatch '\((-?\d+)\)\s*$') { throw "no time-zone bias in the header of $File" }
    $bias = [int]$Matches[1]
    $latency = @(); $queue = @(); $cpu = -1; $defender = -1
    for ($c = 1; $c -lt $header.Count; $c++) {
        $name = $header[$c]
        if ($name -match '\\LogicalDisk\(_Total\)\\') { continue }
        if ($name -match '\\LogicalDisk\([^)]*\)\\Avg\. Disk sec/(Write|Read)$') { $latency += $c }
        elseif ($name -match '\\LogicalDisk\([^)]*\)\\Current Disk Queue Length$') { $queue += $c }
        elseif ($name -match '\\Processor\(_Total\)\\% Processor Time$') { $cpu = $c }
        elseif ($name -match '\\Process\(MsMpEng\)\\% Processor Time$') { $defender = $c }
    }
    $rows = foreach ($line in $lines | Select-Object -Skip 1) {
        $cells = @($line | ConvertFrom-Csv -Header (0..($header.Count - 1)) | ForEach-Object { $_.PSObject.Properties.Value })
        if ($cells.Count -lt $header.Count) { continue }
        $local = [DateTime]::ParseExact($cells[0], 'MM/dd/yyyy HH:mm:ss.fff', [Globalization.CultureInfo]::InvariantCulture)
        $num = { param($i) $v = 0.0; if ($i -ge 0 -and [double]::TryParse($cells[$i], [Globalization.NumberStyles]::Float, [Globalization.CultureInfo]::InvariantCulture, [ref]$v)) { $v } else { [double]::NaN } }
        $lat = @($latency | ForEach-Object { & $num $_ } | Where-Object { -not [double]::IsNaN($_) })
        $q = @($queue | ForEach-Object { & $num $_ } | Where-Object { -not [double]::IsNaN($_) })
        if ($lat.Count -eq 0) { continue }
        [pscustomobject]@{
            Utc      = [DateTime]::SpecifyKind($local.AddMinutes($bias), [DateTimeKind]::Utc)
            Latency  = ($lat | Measure-Object -Maximum).Maximum
            Queue    = if ($q.Count) { ($q | Measure-Object -Maximum).Maximum } else { [double]::NaN }
            Cpu      = & $num $cpu
            Defender = & $num $defender
        }
    }
    return @($rows)
}

# Every finished test execution in an events.tsv, with its interval in UTC.
function Read-Executions {
    param([string] $File, [string] $Source)
    foreach ($line in Get-Content -LiteralPath $File | Select-Object -Skip 1) {
        $at, $iteration, $binary, $json = $line -split "`t", 4
        # libtest's fixed shape, matched rather than parsed: a run holds hundreds of thousands of these.
        if ($json -notmatch '^\{ ?"type": ?"test", ?"name": ?"([^"]+)", ?"event": ?"(ok|failed)", ?"exec_time": ?([0-9.eE+-]+)') { continue }
        $name = $Matches[1]; $outcome = $Matches[2]; $seconds = [double]::Parse($Matches[3], [Globalization.CultureInfo]::InvariantCulture)
        $end = [DateTime]::Parse($at, [Globalization.CultureInfo]::InvariantCulture, [Globalization.DateTimeStyles]::AdjustToUniversal -bor [Globalization.DateTimeStyles]::AssumeUniversal)
        [pscustomobject]@{
            Source = $Source; Iteration = [int]$iteration; Binary = $binary; Test = $name; Outcome = $outcome
            Seconds = $seconds; Start = $end.AddSeconds(-$seconds); End = $end
        }
    }
}

$dirs = @(@(foreach ($p in $Path) { Get-ChildItem -LiteralPath $p -Recurse -Filter events.tsv | ForEach-Object DirectoryName }) | Sort-Object -Unique)
if ($dirs.Count -eq 0) { throw "No events.tsv under $($Path -join ', ')." }

$all = [System.Collections.Generic.List[object]]::new()
$launches = [System.Collections.Generic.List[object]]::new()
$slow = [System.Collections.Generic.List[object]]::new()
$expected = 0.0
foreach ($dir in $dirs) {
    $source = Split-Path -Leaf $dir
    $mode = if (Test-Path -LiteralPath (Join-Path $dir 'machine.json')) { (Get-Content -LiteralPath (Join-Path $dir 'machine.json') -Raw | ConvertFrom-Json).mode } else { '' }
    $runs = @(Read-Executions -File (Join-Path $dir 'events.tsv') -Source $source)
    foreach ($r in $runs) { $r | Add-Member Mode $mode; $all.Add($r) }
    foreach ($phase in Import-Csv -LiteralPath (Join-Path $dir 'phases.tsv') -Delimiter "`t") {
        $span = [DateTime]::Parse($phase.end, [Globalization.CultureInfo]::InvariantCulture) - [DateTime]::Parse($phase.start, [Globalization.CultureInfo]::InvariantCulture)
        $launches.Add([pscustomobject]@{ Mode = $mode; Phase = $phase.phase; Seconds = $span.TotalSeconds })
    }
    $samples = @(Read-Counters -File (Join-Path $dir 'counters.csv'))
    $lat = [double[]]@($samples.Latency | Sort-Object)
    $threshold = Get-Percentile $lat $StallPercentile
    $stall = @($samples | ForEach-Object { $_.Latency -ge $threshold })
    $medians = @{}
    foreach ($g in $runs | Group-Object Binary, Test) { $medians[$g.Name] = Get-Percentile ([double[]]@($g.Group.Seconds | Sort-Object)) 0.5 }
    foreach ($r in $runs) {
        $median = $medians["$($r.Binary), $($r.Test)"]
        if ($r.Seconds -lt $SlowFloorSeconds -or $r.Seconds -lt $SlowFactor * $median) { continue }
        $from = $r.Start.AddSeconds(-1); $to = $r.End.AddSeconds(1)
        $inside = @($samples | Where-Object { $_.Utc -ge $from -and $_.Utc -le $to })
        $peak = if ($inside.Count) { ($inside.Latency | Measure-Object -Maximum).Maximum } else { [double]::NaN }
        # The chance a window this long, placed anywhere in the series, holds a stall.
        $width = [Math]::Max(1, [int][Math]::Ceiling(($to - $from).TotalSeconds))
        $positions = [Math]::Max(1, $stall.Count - $width + 1)
        $hits = 0
        for ($s = 0; $s -lt $positions; $s++) {
            for ($k = $s; $k -lt [Math]::Min($stall.Count, $s + $width); $k++) { if ($stall[$k]) { $hits++; break } }
        }
        $expected += $hits / $positions
        $slow.Add([pscustomobject]@{
                Source = $source; Mode = $mode; Iteration = $r.Iteration; Binary = $r.Binary; Test = $r.Test; Seconds = $r.Seconds
                Median = $median; Samples = $inside.Count; PeakLatency = $peak; Stall = ($inside.Count -gt 0 -and $peak -ge $threshold)
                Threshold = $threshold; Queue = if ($inside.Count) { ($inside.Queue | Measure-Object -Maximum).Maximum } else { [double]::NaN }
                Defender = if ($inside.Count) { ($inside.Defender | Where-Object { -not [double]::IsNaN($_) } | Measure-Object -Maximum).Maximum } else { $null }
            })
    }
    Write-Report ('{0} [{1}]: {2} executions, {3} counter samples, stall threshold (p{4}) {5:n1} ms' -f $source, $mode, $runs.Count,
        $samples.Count, ($StallPercentile * 100), ($threshold * 1000))
}

Write-Report ''
Write-Report "Duration of each build and binary launch, per mode (s):"
foreach ($g in $launches | Group-Object Mode, Phase | Sort-Object Name) {
    $perLaunch = [double[]]@($g.Group.Seconds | Sort-Object)
    Write-Report ('  {0,-36} launches {1,5}  p50 {2,7:n2}  p99 {3,7:n2}  max {4,7:n2}' -f $g.Name, $perLaunch.Count,
        (Get-Percentile $perLaunch 0.5), (Get-Percentile $perLaunch 0.99), $perLaunch[-1])
}

Write-Report ''
Write-Report "Slow executions (>= ${SlowFactor}x the test's median and >= ${SlowFloorSeconds}s): $($slow.Count) of $($all.Count)"
foreach ($g in $slow | Group-Object Mode) { Write-Report "  $($g.Name): $($g.Count)" }
foreach ($g in $slow | Group-Object Binary, Test | Sort-Object Count -Descending | Select-Object -First 15) {
    Write-Report ('  {0,4}  {1}' -f $g.Count, $g.Name)
}
$overlapping = @($slow | Where-Object Stall).Count
$noSamples = @($slow | Where-Object { $_.Samples -eq 0 }).Count
Write-Report ''
Write-Report ('Overlapping a disk stall: {0} of {1}; by chance alone, about {2:n1}. With no counter sample in the window: {3}.' -f $overlapping,
    $slow.Count, $expected, $noSamples)
Write-Report ''
Write-Report "Slowest executions:"
foreach ($s in $slow | Sort-Object Seconds -Descending | Select-Object -First $List) {
    Write-Report ('  {0,7:n2}s (median {1:n2}s) {2,-5} peak {3,8:n1} ms queue {4,5:n1} defender {5,5:n0}%  [{6}] {7} :: {8} @{9}#{10}' -f $s.Seconds,
        $s.Median, $(if ($s.Stall) { 'STALL' } else { '' }), ($s.PeakLatency * 1000), $s.Queue, $s.Defender, $s.Mode, $s.Binary, $s.Test, $s.Source, $s.Iteration)
}
