# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs the sabotage harness over a faux plan -- a planned mix of passes,
    failures and overruns -- with the plan's entries spread across parallel
    processes, so the sweep takes about as long as its slowest entry rather than
    the sum of them.

.DESCRIPTION
    A plan is an ordinary sabotage manifest whose entries each carry a
    `// faux: <directive>` line in their `replace` text; tools/faux/faux-cargo.cmd
    reads it back and behaves as planned. See "Faux runs" in README-sabotage.md.

    Most of a faux sweep's time is the runs that overrun: each costs its whole
    bound, one after another. The harness already splits a manifest with
    -Shard/-ShardCount, so this script starts one harness process per shard at
    once, each with its OWN output directory (a shard's working copy is not
    shareable), and prints their reports in shard order when they finish.

    The launcher is built once, here, and handed to every shard: building it in
    each would have them contend for one cargo target directory.

.PARAMETER Plan
    The manifest to run. Defaults to plan.json beside this script.

.PARAMETER Jobs
    How many shards to run at once. Defaults to the number of entries, capped at
    the processor count and 8. 1 runs the plan serially, in one process.

.PARAMETER TimeoutSeconds
    The harness's bound for a test run. Defaults to 3: short, because every
    planned overrun costs it, and long enough that a planned `sleep 1` finishes
    well inside it.

.PARAMETER Name
    Only entries whose name matches this wildcard.

.PARAMETER HarnessArguments
    Anything else to hand run-sabotage.ps1, e.g. '-TraceLaunches'.

.PARAMETER LauncherPath
    An already-built win-job-launcher.exe. Built from this checkout, into
    .scratch/faux, when not given.

.OUTPUTS
    Exits 0 if every shard did, otherwise the highest exit code any shard
    returned (1: a run did not do what its entry declared; 2: nothing was swept).
#>
[CmdletBinding()]
param(
    [string] $Plan,
    [int] $Jobs = 0,
    [int] $TimeoutSeconds = 3,
    [string] $Name = '*',
    [string[]] $HarnessArguments = @(),
    [string] $LauncherPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot '..\common.ps1')

# Not a parameter default: Windows PowerShell 5.1 leaves $PSScriptRoot empty
# there.
if (-not $Plan) { $Plan = Join-Path $PSScriptRoot 'plan.json' }

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'heading')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'heading' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
$harness = Join-Path $PSScriptRoot '..\run-sabotage.ps1'
$faux = Join-Path $PSScriptRoot 'faux-cargo.cmd'
$planPath = [System.IO.Path]::GetFullPath($Plan)
if (-not (Test-Path -LiteralPath $planPath)) {
    Write-Report "No plan at: $planPath" -Level bad
    exit 2
}

$entries = @((Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json).sabotages).Count
if ($Jobs -le 0) { $Jobs = [Math]::Min([Math]::Min($entries, 8), [Environment]::ProcessorCount) }
if ($Jobs -lt 1) { $Jobs = 1 }

$clock = [Diagnostics.Stopwatch]::StartNew()

# The launcher is built from THIS checkout, once, into a directory that outlives
# the run so the next one starts warm -- unless the caller already has one.
$scratch = Join-Path $repoRoot '.scratch\faux'
New-Item -ItemType Directory -Force -Path $scratch | Out-Null
if ($LauncherPath) {
    $launcher = [System.IO.Path]::GetFullPath($LauncherPath)
}
else {
    $launcher = Build-JobLauncher -TargetDirectory (Join-Path $scratch 'launcher-target')
}

# The plan runs in a repository of its own, holding nothing but the plan and its
# subject. The harness copies every tracked file of the repository it sweeps
# into a working copy per shard; for this checkout that is most of a second per
# shard, and for a plan whose runs take about a second it would dominate. The
# fixture's copy is two files.
$fixture = Join-Path ([System.IO.Path]::GetTempPath()) ('faux-plan-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $fixture | Out-Null
try {
    Copy-Item -LiteralPath $planPath -Destination (Join-Path $fixture 'sabotage.json')
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'subject.faux') -Destination (Join-Path $fixture 'subject.faux')
    [System.IO.File]::WriteAllText((Join-Path $fixture '.gitignore'), ".scratch/`n")
    New-Item -ItemType Directory -Force -Path (Join-Path $fixture '.scratch') | Out-Null
    $null = Invoke-Native { git -C $fixture init --quiet }
    $null = Invoke-Native { git -C $fixture add -A }

    $shell = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell' }
    $running = @()
    for ($i = 0; $i -lt $Jobs; $i++) {
        # Under .scratch, which git ignores, so the harness does not copy a log it
        # is itself being written to into its working copy.
        $log = Join-Path $fixture ".scratch\shard-$i.log"
        $arguments = @(
            '-NoProfile', '-File', $harness,
            '-Manifest', 'sabotage.json', '-CargoCommand', $faux,
            '-TimeoutSeconds', "$TimeoutSeconds", '-Name', $Name,
            '-OutputDirectory', (Join-Path $fixture ".scratch\shard-$i"),
            '-LauncherPath', $launcher
        )
        if ($Jobs -gt 1) { $arguments += @('-Shard', "$i", '-ShardCount', "$Jobs") }
        $arguments += $HarnessArguments
        $quoted = @($arguments | ForEach-Object { ConvertTo-NativeArgument ([string]$_) })
        $running += [pscustomobject]@{
            Index = $i; Log = $log
            Process = Start-Process -FilePath $shell -ArgumentList $quoted -WorkingDirectory $fixture `
                -PassThru -NoNewWindow -RedirectStandardOutput $log -RedirectStandardError "$log.err"
        }
    }

    # Touching the handle keeps the exit code readable on Windows PowerShell 5.1.
    foreach ($shard in $running) { $null = $shard.Process.Handle }

    $worst = 0
    foreach ($shard in $running) {
        $shard.Process.WaitForExit()
        $code = [int]$shard.Process.ExitCode
        if ($code -gt $worst) { $worst = $code }
        Write-Report ''
        $level = 'heading'
        if ($code -ne 0) { $level = 'bad' }
        Write-Report "=== shard $($shard.Index + 1) of $Jobs (exit $code) ===" -Level $level
        foreach ($path in @($shard.Log, "$($shard.Log).err")) {
            if (Test-Path -LiteralPath $path) {
                Get-Content -LiteralPath $path | ForEach-Object { Write-Report $_ }
            }
        }
    }
}
finally {
    Remove-Item -LiteralPath $fixture -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Report ''
$seconds = [Math]::Round($clock.Elapsed.TotalSeconds, 1)
if ($worst -eq 0) {
    Write-Report "Every planned run behaved as declared ($Jobs shard(s), ${seconds}s)." -Level good
}
else {
    Write-Report "A shard reported a problem (worst exit $worst; ${seconds}s)." -Level bad
}
exit $worst
