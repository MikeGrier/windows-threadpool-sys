# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Times every test in win-durable-ioring's file-I/O test binaries, run after run, beside the
    machine's disk and processor counters. The CI half of CHECKLIST.md's M34.28.

.DESCRIPTION
    The sabotage sweeps' full-suite durations on GitHub's hosted runners have a tail that sits in
    three test binaries: the `win_durable_ioring` unit tests, `overlapped_evidence`, and the
    `blocking_push` example. A whole-launch duration cannot say whether a slow launch waited on the
    disk or on a timed wait that masks a lost wakeup. This records, for every test execution, when it
    ended and how long it ran (libtest's JSON report, with `exec_time`), and samples per-second
    disk-latency, disk-queue and processor counters with `typeperf` across the same interval, so
    [report-io-test-tail.ps1](report-io-test-tail.ps1) can say which tests are slow and whether each
    slow execution overlaps high disk latency.

    libtest's JSON format and `--report-time` are unstable options, accepted when `RUSTC_BOOTSTRAP=1`
    is set in the TEST BINARY's environment at run time. It is set for those launches only, after the
    build: nothing compiled differs from what the sweep runs.

    -Mode rebuild touches the crate's lib.rs and rebuilds the binaries before every iteration, as a
    sweep entry does before each run, so build output is being written back around the tests. -Mode
    steady builds once. Comparing the two says whether the tail follows the rebuild.

    Writes to -OutputDirectory: `events.tsv` (UTC arrival time, iteration, binary, libtest JSON
    line), `phases.tsv` (UTC start and end of each build and each binary launch, with exit code),
    `counters.csv` (typeperf's own CSV, local time with the bias in its header), and `machine.json`.
#>
#Requires -Version 7
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateRange(1, 10000)][int] $Iterations,
    [Parameter(Mandatory = $true)][string] $OutputDirectory,
    [ValidateSet('steady', 'rebuild')][string] $Mode = 'steady'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param([Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message)
    Write-Host $Message
}

function Get-UtcStamp { [DateTime]::UtcNow.ToString('o') }

$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$crate = Join-Path $root 'crates/win-durable-ioring'
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$out = (Resolve-Path $OutputDirectory).Path
$events = Join-Path $out 'events.tsv'
$phases = Join-Path $out 'phases.tsv'
$counters = Join-Path $out 'counters.csv'
Set-Content -LiteralPath $events -Value "at`titeration`tbinary`tline" -Encoding utf8
Set-Content -LiteralPath $phases -Value "iteration`tphase`tstart`tend`tcode" -Encoding utf8

$cargoArgs = @('test', '-p', 'win-durable-ioring', '--lib', '--test', 'overlapped_evidence', '--example', 'blocking_push', '--no-run', '--message-format=json')

# The three binaries, from cargo's own record of what it built.
function Invoke-Build {
    param([string] $Iteration)
    $start = Get-UtcStamp
    $json = & cargo @cargoArgs 2>$null
    $code = $LASTEXITCODE
    Add-Content -LiteralPath $phases -Value "$Iteration`tbuild`t$start`t$(Get-UtcStamp)`t$code"
    if ($code -ne 0) { throw "cargo $($cargoArgs -join ' ') exited $code" }
    $built = @($json | ForEach-Object { $_ | ConvertFrom-Json } |
            Where-Object { $_.reason -eq 'compiler-artifact' -and $_.profile.test -and $_.executable } |
            ForEach-Object { [pscustomobject]@{ Name = $_.target.name; Path = $_.executable } })
    if ($built.Count -ne 3) { throw "expected 3 test binaries, cargo reported $($built.Count): $($built.Name -join ', ')" }
    return $built
}

$counterPaths = @(
    '\LogicalDisk(*)\Avg. Disk sec/Write', '\LogicalDisk(*)\Avg. Disk sec/Read',
    '\LogicalDisk(*)\Current Disk Queue Length', '\Processor(_Total)\% Processor Time',
    '\Process(MsMpEng)\% Processor Time')
$machine = [ordered]@{
    runner = $env:RUNNER_NAME; mode = $Mode; iterations = $Iterations; temp = [System.IO.Path]::GetTempPath()
    processors = [Environment]::ProcessorCount; os = [Environment]::OSVersion.VersionString; startedUtc = Get-UtcStamp
    drives = @(Get-PSDrive -PSProvider FileSystem | ForEach-Object { [ordered]@{ name = $_.Name; usedBytes = $_.Used; freeBytes = $_.Free } })
}

$binaries = Invoke-Build -Iteration 0
$sampler = Start-Process typeperf -PassThru -WindowStyle Hidden -ArgumentList (@($counterPaths | ForEach-Object { "`"$_`"" }) +
    @('-si', '1', '-f', 'CSV', '-o', "`"$counters`"", '-y'))
try {
    Start-Sleep -Seconds 2
    for ($i = 1; $i -le $Iterations; $i++) {
        if ($Mode -eq 'rebuild') {
            (Get-Item (Join-Path $crate 'src/lib.rs')).LastWriteTimeUtc = [DateTime]::UtcNow
            $binaries = Invoke-Build -Iteration $i
        }
        foreach ($binary in $binaries) {
            $start = Get-UtcStamp
            $env:RUSTC_BOOTSTRAP = '1'
            # Buffered, and flushed once the launch ends, so the probe's own writes do not land among the tests'.
            $writer = [System.IO.StreamWriter]::new($events, $true, [System.Text.UTF8Encoding]::new($false), 1MB)
            try {
                Push-Location $crate
                & $binary.Path -Z unstable-options --format json --report-time 2>&1 | ForEach-Object {
                    $writer.WriteLine("$(Get-UtcStamp)`t$i`t$($binary.Name)`t$_")
                }
                $code = $LASTEXITCODE
            }
            finally {
                $writer.Dispose()
                Pop-Location
                Remove-Item Env:RUSTC_BOOTSTRAP -ErrorAction SilentlyContinue
            }
            Add-Content -LiteralPath $phases -Value "$i`t$($binary.Name)`t$start`t$(Get-UtcStamp)`t$code"
        }
        if ($i % 10 -eq 0 -or $i -eq $Iterations) { Write-Report "iteration $i of $Iterations ($Mode)" }
    }
    Start-Sleep -Seconds 2
}
finally {
    Stop-Process -Id $sampler.Id -ErrorAction SilentlyContinue
    $machine.endedUtc = Get-UtcStamp
    $machine | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $out 'machine.json') -Encoding utf8
}
Write-Report "Wrote $out"
