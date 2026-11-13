# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    How long each sabotage manifest's full suite takes on CI, from the sweep jobs' logs.

.DESCRIPTION
    The harness's hang bound has to sit above the suite's duration on the machine that runs the
    sweep, and a developer's machine says nothing about GitHub's hosted runners. The sweep's own
    job logs do: every shard runs `run-sabotage.ps1 -TraceLaunches`, so each launch logs its
    duration through win-job-launcher (`the command exited with code C after Nms`, or `the bound of
    Nms was reached`, both worded in that crate's launch.rs), followed by the `.result` file it
    wrote. The logs outlive the transcripts, which are uploaded only for a shard that fails.

    One row per FULL-SUITE launch: each shard's baseline, and each entry whose verdict did not come
    from its narrowed run (`caught (its recorded tests failed ...)`), since a narrowed run is a few
    tests and says nothing about the suite. Build launches write `.build.result` and are left out.
    For an entry with two run launches (a narrowed run that was not credited, then the suite), the
    last is the suite.

    Baselines run under the build bound, not the hang bound, so their durations are never cut short;
    they are the sample to place a bound by. Entry launches killed at the bound are kept as `bound`
    rows: an entry that reaches it on every launch hangs, and one that reaches it only sometimes is
    the case a bound inside the runners' tail produces.

    Needs PowerShell 7 and an authenticated `gh` unless -Offline is given. Logs are cached under
    -LogDirectory, filtered to the lines this reads, and a job already there is not fetched again.

.PARAMETER Manifest
    Manifest paths, relative to the repository root. Each one's sweep jobs are found by its label
    (Get-SabotageManifestLabel in common.ps1, which the planner names them by), and its entries'
    declared `expect` is read from it.

.PARAMETER Since
    Fetch the shard jobs of every completed `sabotage sweeps` run created at or after this time.

.PARAMETER Offline
    Read only what -LogDirectory already holds.

.EXAMPLE
    ./tools/measure-sweep-durations.ps1 -Since 2026-11-04 -Csv .scratch/launches.csv
#>
#Requires -Version 7
[CmdletBinding()]
param(
    [string[]] $Manifest = @('crates/durable-ioring/sabotage.json', 'crates/win-durable-ioring/sabotage.json'),
    [datetime] $Since = (Get-Date).AddDays(-14),
    [string] $LogDirectory,
    [string] $Csv,
    [switch] $Offline,
    [int] $Throttle = 8
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')
if (-not $LogDirectory) { $LogDirectory = Join-Path $PSScriptRoot '../.scratch/sweep-durations/logs' }

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message)
    Write-Host $Message
}

# The lines of a shard log this script reads; everything else is dropped when the log is cached.
$script:KeptLine = 'win-job-launcher .*(exited with code|bound of \d+ms was reached|wrote .*\.result)|Hang bound: |\[\s*\d+/\d+\] '

# One row per full-suite launch in one shard's log. `Header` carries the job's identity; `Expect`
# maps an entry name to its declared expectation.
function ConvertFrom-SweepShardLog {
    param([string[]] $Lines, [hashtable] $Header, [hashtable] $Expect)
    $rows = @()
    $bound = $null
    $current = $null
    $launches = @()
    $pending = $null
    foreach ($raw in $Lines) {
        $line = $raw -replace '^\d{4}-\d\d-\d\dT[\d:.]+Z ', ''
        if ($line -match 'Hang bound: (\d+)s') { $bound = [int]$Matches[1]; continue }
        if ($line -match 'exited with code (-?\d+) after (\d+)ms') { $pending = @{ Code = $Matches[1]; Ms = [int]$Matches[2] }; continue }
        if ($line -match 'the bound of (\d+)ms was reached') { $pending = @{ Code = 'bound'; Ms = [int]$Matches[1] }; continue }
        if ($line -match 'wrote .*[\\/]([^\\/]+?)\.txt(\.build)?\.result\s*$') {
            $stem = $Matches[1]
            $isBuild = [bool]$Matches[2]
            if ($pending -and -not $isBuild) {
                if ($stem -eq 'baseline') {
                    $rows += [pscustomobject]($Header + @{ Kind = 'baseline'; Entry = ''; Expect = ''; Verdict = ''; Code = $pending.Code; Ms = $pending.Ms; Bound = '' })
                }
                else { $launches += $pending }
            }
            $pending = $null
            continue
        }
        if ($line -match '^\[\s*\d+/\d+\] (.+)$') {
            $text = $Matches[1]
            if ($current -and $text.Length -gt $current.Length -and $text.StartsWith($current)) {
                $verdict = $text.Substring($current.Length).Trim() -replace '\s*\(\d+s(, ~[^)]*)?\)$', ''
                if ($verdict -notmatch '^caught \(its recorded tests failed' -and $launches.Count -gt 0) {
                    $last = $launches[-1]
                    $rows += [pscustomobject]($Header + @{ Kind = 'entry'; Entry = $current; Expect = [string]$Expect[$current]
                            Verdict = $verdict; Code = $last.Code; Ms = $last.Ms; Bound = $bound })
                }
                $current = $null
            }
            else { $current = $text }
            $launches = @()
        }
    }
    return $rows
}

# Nearest-rank percentile of an ascending array.
function Get-Percentile {
    param([double[]] $Sorted, [double] $P)
    if ($Sorted.Count -eq 0) { return [double]::NaN }
    $at = [Math]::Ceiling($P * $Sorted.Count) - 1
    return $Sorted[[Math]::Max(0, [Math]::Min($Sorted.Count - 1, $at))]
}

$labels = @{}
$expects = @{}
foreach ($path in $Manifest) {
    $label = Get-SabotageManifestLabel $path
    $labels[$label] = $path
    $expects[$label] = @{}
    foreach ($entry in (Get-Content -LiteralPath $path -Raw | ConvertFrom-Json).sabotages) { $expects[$label][$entry.name] = $entry.expect }
}
New-Item -ItemType Directory -Force $LogDirectory | Out-Null

if (-not $Offline) {
    $repo = (gh repo view --json nameWithOwner -q .nameWithOwner)
    $runs = @(gh run list --workflow 'sabotage sweeps' -L 500 --json databaseId,createdAt,status `
            --jq '.[] | select(.status=="completed") | "\(.databaseId) \(.createdAt)"' | Where-Object { $_ -match '^\d+ \S+$' })
    $jobs = foreach ($line in $runs) {
        $id, $at = $line.Split(' ')
        if ([datetime]::Parse($at).ToUniversalTime() -lt $Since.ToUniversalTime()) { continue }
        foreach ($j in gh api --paginate "repos/$repo/actions/runs/$id/jobs?per_page=100" `
                --jq '.jobs[] | "\(.id)\t\(.name)\t\(.conclusion)\t\(.runner_name)"' 2>$null) {
            $f = $j.Split("`t")
            if ($f.Count -lt 4 -or $f[1] -notmatch '^sweep (\S+) \d+/\d+$' -or -not $labels.ContainsKey($Matches[1])) { continue }
            [pscustomobject]@{ Run = $id; RunAt = $at; Job = $f[0]; Name = $f[1]; Conclusion = $f[2]; Runner = $f[3] }
        }
    }
    Write-Report "Shard jobs since $($Since.ToString('u')): $(@($jobs).Count)"
    $kept = $script:KeptLine
    $missed = @($jobs | ForEach-Object -ThrottleLimit $Throttle -Parallel {
            $path = Join-Path $using:LogDirectory "$($_.Run)-$($_.Job).txt"
            if (Test-Path -LiteralPath $path) { return }
            $raw = gh api --allow-escape-sequences "repos/$using:repo/actions/jobs/$($_.Job)/logs" 2>$null
            if ($LASTEXITCODE -ne 0 -or -not $raw) { return "$($_.Run)/$($_.Job) $($_.Name)" }
            $lines = $raw | ForEach-Object { $_ -replace '\x1b\[[0-9;]*m', '' } | Where-Object { $_ -match $using:kept }
            $head = "# run=$($_.Run) runAt=$($_.RunAt) job=$($_.Job) name=$($_.Name) conclusion=$($_.Conclusion) runner=$($_.Runner)"
            Set-Content -LiteralPath $path -Value (@($head) + @($lines)) -Encoding utf8
        })
    foreach ($m in $missed) { Write-Report "  not fetched (re-run to retry): $m" }
}

$rows = @(foreach ($file in Get-ChildItem -LiteralPath $LogDirectory -Filter *.txt) {
        $lines = @(Get-Content -LiteralPath $file.FullName)
        if ($lines.Count -eq 0 -or $lines[0] -notmatch '^# run=(\d+) runAt=(\S+) job=(\d+) name=sweep (\S+) (\d+)/\d+ ') { continue }
        $header = @{ Run = $Matches[1]; RunAt = $Matches[2]; Job = $Matches[3]; Manifest = $Matches[4]; Shard = [int]$Matches[5] }
        if (-not $labels.ContainsKey($header.Manifest)) { continue }
        ConvertFrom-SweepShardLog -Lines ($lines | Select-Object -Skip 1) -Header $header -Expect $expects[$header.Manifest]
    })
if ($Csv) {
    $rows | Select-Object Manifest, Kind, Run, RunAt, Job, Shard, Entry, Expect, Verdict, Code, Ms, Bound |
        Export-Csv -LiteralPath $Csv -NoTypeInformation -Encoding utf8
}

$runCount = @($rows | ForEach-Object Run | Sort-Object -Unique).Count
Write-Report "Full-suite launches: $($rows.Count), from $runCount sweep run(s)."
$median = @{}
foreach ($label in $labels.Keys | Sort-Object) {
    $base = [double[]]@($rows | Where-Object { $_.Manifest -eq $label -and $_.Kind -eq 'baseline' } | ForEach-Object { $_.Ms / 1000.0 } | Sort-Object)
    $median[$label] = Get-Percentile $base 0.5
    Write-Report ''
    Write-Report "$label -- baselines (never cut short): n=$($base.Count)"
    if ($base.Count -gt 0) {
        Write-Report ('  p50 {0:n1}s  p90 {1:n1}s  p99 {2:n1}s  p99.9 {3:n1}s  max {4:n1}s' -f (Get-Percentile $base 0.5),
            (Get-Percentile $base 0.9), (Get-Percentile $base 0.99), (Get-Percentile $base 0.999), $base[-1])
    }
    $entries = @($rows | Where-Object { $_.Manifest -eq $label -and $_.Kind -eq 'entry' })
    $reached = @($entries | Group-Object Entry | Where-Object { @($_.Group | Where-Object Code -eq 'bound').Count -gt 0 })
    $every = @($reached | Where-Object { @($_.Group | Where-Object Code -ne 'bound').Count -eq 0 })
    Write-Report "  entries that reached the bound: $($reached.Count) (on every launch: $($every.Count))"
    foreach ($group in $reached | Where-Object { $every -notcontains $_ } | Sort-Object Name) {
        $hits = @($group.Group | Where-Object Code -eq 'bound')
        $others = ($group.Group | Where-Object Code -ne 'bound' | ForEach-Object { '{0:n1}' -f ($_.Ms / 1000.0) } | Sort-Object { [double]$_ }) -join ', '
        Write-Report ('    [{0}] {1}: bound on {2} of {3}; otherwise {4}s' -f $group.Group[0].Expect, $group.Name, $hits.Count, $group.Count, $others)
    }
}
$excess = [double[]]@($rows | Where-Object Kind -eq 'baseline' | ForEach-Object { $_.Ms / 1000.0 - $median[$_.Manifest] } | Sort-Object)
if ($excess.Count -gt 0) {
    $p999 = Get-Percentile $excess 0.999
    Write-Report ''
    Write-Report ('Baseline excess over each manifest''s median, pooled: n={0}  p99 {1:n1}s  p99.9 {2:n1}s  max {3:n1}s' -f $excess.Count,
        (Get-Percentile $excess 0.99), $p999, $excess[-1])
    foreach ($label in $labels.Keys | Sort-Object) {
        Write-Report ('  {0}: median + pooled p99.9 = {1:n1}s' -f $label, ($median[$label] + $p999))
    }
}
