# Copyright (c) 2026 Mike Grier. All rights reserved.
<#
.SYNOPSIS
    Builds and runs the standalone NUMA spikes, capturing their output.

.DESCRIPTION
    The spikes under crates/windows-ioring-sys/design-sessions/spikes are
    deliberately NOT workspace members: each is a single file written against
    windows-sys alone, so that what it measures is the operating system's
    behaviour and not ours. Their README documents the way to run one -- drop it
    into a scratch binary crate with a single dependency -- and this script is
    that procedure, automated.

    Running it in CI has a side effect worth having: it is the executable form
    of that README instruction, so the instruction cannot rot without turning
    the step red.

    ON A SINGLE-NODE MACHINE EVERY SPIKE HERE IS VACUOUS, and each says so in
    its own output rather than printing a confident zero. That is expected. The
    cost is a minute; the payoff is that if a multi-node runner ever appears,
    the answer is already in that build's log.

    This script never fails on a spike's result. It exits non-zero only if a
    spike fails to BUILD, which is a real defect in the instrument.

.PARAMETER Summary
    Optional path to append a rendered summary to, for $env:GITHUB_STEP_SUMMARY.

.PARAMETER OutputDirectory
    Where to write per-spike transcripts. Defaults to .scratch/numa-spikes.
#>
[CmdletBinding()]
param(
    [string] $Summary,
    [string] $OutputDirectory = (Join-Path $PSScriptRoot '..\.scratch\numa-spikes')
)

$ErrorActionPreference = 'Stop'

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$spikeDir = Join-Path $repoRoot 'crates\windows-ioring-sys\design-sessions\spikes'

# name -> the windows-sys features that spike's own doc comment asks for.
$spikes = @(
    @{
        Name     = 'file-handle-numa'
        File     = 'file-handle-numa-spike.rs'
        Features = @(
            '"Win32_Foundation"', '"Win32_Security"', '"Win32_Storage_FileSystem"',
            '"Win32_System_IO"', '"Win32_System_Ioctl"'
        )
        Asks     = 'Does a file handle name a NUMA node, and is it the volume''s or the file''s?'
    },
    @{
        Name     = 'thread-stack-numa'
        File     = 'thread-stack-numa-spike.rs'
        Features = @(
            '"Win32_Foundation"', '"Win32_Security"', '"Win32_System_Threading"',
            '"Win32_System_SystemInformation"', '"Win32_System_ProcessStatus"'
        )
        Asks     = 'Does creation-time affinity govern where a thread''s stack lives?'
    }
)

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$buildFailures = 0
$sections = New-Object System.Collections.Generic.List[string]

foreach ($spike in $spikes) {
    $source = Join-Path $spikeDir $spike.File
    if (-not (Test-Path $source)) {
        Write-Host "::warning::spike source missing: $source"
        $buildFailures++
        continue
    }

    $work = Join-Path ([System.IO.Path]::GetTempPath()) ("spike-" + $spike.Name + "-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path (Join-Path $work 'src') | Out-Null

    $features = $spike.Features -join ', '
    $manifest = @"
[package]
name = "spike_$($spike.Name -replace '-','_')"
version = "0.0.0"
edition = "2021"

[dependencies]
windows-sys = { version = "0.61.2", default-features = false, features = [$features] }

[workspace]
"@
    Set-Content -Path (Join-Path $work 'Cargo.toml') -Value $manifest -Encoding utf8
    Copy-Item $source (Join-Path $work 'src\main.rs') -Force

    Write-Host "=== building $($spike.Name) ==="
    Push-Location $work
    try {
        $build = & cargo build --quiet 2>&1
        $buildExit = $LASTEXITCODE
        if ($buildExit -ne 0) {
            # A build failure is a defect in the instrument, and is the one
            # thing here worth failing over.
            Write-Host "::error::spike $($spike.Name) failed to build"
            $build | ForEach-Object { Write-Host $_ }
            $buildFailures++
            $sections.Add("### $($spike.Name)`n`n**FAILED TO BUILD** -- the instrument is broken, not the machine.`n")
            continue
        }

        Write-Host "=== running $($spike.Name) ==="
        $output = & cargo run --quiet 2>&1 | Out-String
        Write-Host $output
    }
    finally {
        Pop-Location
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }

    $transcript = Join-Path $OutputDirectory "$($spike.Name).txt"
    Set-Content -Path $transcript -Value $output -Encoding utf8

    $vacuous = $output -match 'VACUOUS'
    $verdict = if ($vacuous) {
        'vacuous on this runner (single NUMA node) -- expected, and the spike said so itself'
    }
    else {
        '**NOT vacuous -- this runner has more than one NUMA node. Read the output.**'
    }

    $sections.Add(@"
### $($spike.Name)

$($spike.Asks)

$verdict

``````
$output
``````
"@)
}

if ($Summary) {
    $header = @"
## NUMA spike results

Observational. These never fail the build; a red step here means a spike failed
to **build**, which is a defect in the instrument rather than a finding about
the machine.

"@
    Add-Content -Path $Summary -Value ($header + ($sections -join "`n"))
}

if ($buildFailures -gt 0) {
    Write-Host "::error::$buildFailures spike(s) failed to build"
    exit 1
}
Write-Host "all spikes built and ran"
exit 0
