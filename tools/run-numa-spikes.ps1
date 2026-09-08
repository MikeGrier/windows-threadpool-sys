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

    This script never fails on a spike's *result*. It exits non-zero when a
    spike fails to BUILD or fails to RUN, both of which are defects in the
    instrument rather than findings about the machine.

    That distinction is the whole contract, so the `numa-spikes` job carries no
    `continue-on-error`: this script is the only place that separates a finding
    from instrument rot, and a job-level flag would flatten the two back
    together, turning a spike that no longer compiles into a green check. The
    job had such a flag until 2026-09-04, which is exactly the defect the PR #56
    review caught. Do not add one back on the grounds that a spike's result
    should not block the build -- the result already does not.

.PARAMETER Summary
    Optional path to append a rendered summary to, for $env:GITHUB_STEP_SUMMARY.

.PARAMETER OutputDirectory
    Where to write per-spike transcripts. Defaults to .scratch/numa-spikes.
#>
[CmdletBinding()]
param(
    [string] $Summary,
    [string] $OutputDirectory
)

$ErrorActionPreference = 'Stop'

# Resolved here rather than as a param default: Windows PowerShell 5.1 does not
# populate $PSScriptRoot while evaluating a default on a [CmdletBinding()]
# script, so the default form fails outright under 5.1 while working under 7.
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $PSScriptRoot '..\.scratch\numa-spikes'
}

# The single output sink. Every message this tool emits goes through here, so
# the destination and the formatting stay separable from the call sites that
# produce the content -- the repository's one-output-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'warning', 'error')][string] $Level = 'info'
    )
    switch ($Level) {
        'warning' { Write-Host "::warning::$Message" }
        'error' { Write-Host "::error::$Message" }
        default { Write-Host $Message }
    }
}

# `Invoke-Native` and `ConvertTo-OutputLines`, which every capture below goes
# through. Dot-sourced rather than imported: a module copy of that guard does
# not reach the scriptblock it is handed, and silently fails on 5.1 alone. The
# full argument, and the measurement behind it, is in that file.
#
# What it costs THIS script, recorded here because the shape is specific to the
# spikes: `cargo --quiet` writes nothing to stderr on a clean build, so under
# 5.1 this ran to completion for as long as every spike was healthy. It threw
# only when cargo did write there -- a warning, or a failed compile -- which is
# exactly the case this script exists to report. The throw landed before
# `$buildExit` was assigned, so the broken-instrument branch never ran: no
# transcript, no summary, and the one artifact somebody downloads to diagnose a
# rotted spike was the one case that never produced it. Confirmed both ways
# against a deliberately uncompilable crate: unguarded, 5.1 threw and captured
# nothing; guarded, it returned exit 101 with all seven lines of diagnostic.
. (Join-Path $PSScriptRoot 'common.ps1')

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

# Made absolute once, here, because each spike is built and run from inside a
# temp crate directory. A relative -OutputDirectory resolved later would land
# under that temp directory and be deleted with it, so the transcripts would
# vanish exactly when someone went looking for them.
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$instrumentFailures = 0
$sections = New-Object System.Collections.Generic.List[string]

foreach ($spike in $spikes) {
    $source = Join-Path $spikeDir $spike.File
    if (-not (Test-Path $source)) {
        Write-Report "spike source missing: $source" -Level warning
        $instrumentFailures++
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

    $buildOutput = ''
    $output = ''
    $buildExit = 0
    $runExit = 0
    $built = $false

    Write-Report "=== building $($spike.Name) ==="
    Push-Location $work
    try {
        $build = Invoke-Native { cargo build --quiet }
        $buildExit = $LASTEXITCODE
        $buildOutput = ($build | Out-String)
        if ($buildExit -ne 0) {
            # A build failure is a defect in the instrument, and is one of the
            # two things here worth failing over.
            Write-Report "spike $($spike.Name) failed to build" -Level error
            # Echo the same text the transcript gets, so the log and the
            # artifact are two renderings of one capture rather than two
            # records of one failure that a reader has to reconcile.
            # `Invoke-Native` has already flattened the ErrorRecords `2>&1`
            # produces into plain strings, so neither carries the stray
            # `System.Management.Automation.RemoteException` this used to
            # splice into the middle of a compiler diagnostic.
            $buildOutput.TrimEnd() -split "`n" | ForEach-Object { Write-Report $_.TrimEnd() }
            $instrumentFailures++
            $sections.Add("### $($spike.Name)`n`n**FAILED TO BUILD** -- the instrument is broken, not the machine.`n")
        }
        else {
            $built = $true
            Write-Report "=== running $($spike.Name) ==="
            $output = Invoke-Native { cargo run --quiet } | Out-String
            $runExit = $LASTEXITCODE
            Write-Report $output
        }
    }
    finally {
        Pop-Location
        Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
    }

    # Written for every spike, including one that failed to build. This used to
    # sit after a `continue` in the branch above: the `finally` still ran, but
    # everything after the try/finally was skipped, so the broken instrument --
    # the only case anyone downloads this artifact to diagnose -- was the one
    # case that produced no transcript at all.
    #
    # The build side is recorded even on success, because `cargo build --quiet`
    # still emits warnings and those were previously captured into $build and
    # then discarded. A spike accumulating warnings is an instrument beginning
    # to rot, and that is worth seeing before it fails outright.
    $transcript = Join-Path $OutputDirectory "$($spike.Name).txt"
    $transcriptBody = @"
=== build (exit $buildExit) ===
$(if ($buildOutput.Trim()) { $buildOutput.TrimEnd() } else { '(no build output)' })

=== run ($(if ($built) { "exit $runExit" } else { 'not run -- build failed' })) ===
$(if ($output.Trim()) { $output.TrimEnd() } else { '(no run output)' })
"@
    Set-Content -Path $transcript -Value $transcriptBody -Encoding utf8

    if (-not $built) { continue }

    if ($runExit -ne 0) {
        # The other one. Vacuity is decided by searching the output for
        # `VACUOUS`, and a spike that crashed printed no such line -- so
        # without this the summary would announce "**NOT vacuous -- this runner
        # has more than one NUMA node**" on the strength of a stack trace, and
        # the script would still exit 0. That is the instrument breaking while
        # claiming a result about the machine.
        Write-Report "spike $($spike.Name) failed to run (exit $runExit)" -Level error
        $instrumentFailures++
        $verdict = "**FAILED TO RUN (exit $runExit)** -- the instrument is broken, so this says nothing about the machine."
    }
    # -cmatch, not -match, and the case is load-bearing. PowerShell's -match is
    # case-INSENSITIVE, and thread-stack-numa-spike.rs emits a structured line
    # carrying the field name `"vacuous":false` on every run -- so the
    # insensitive form matched that field name even on a multi-node machine,
    # making the `NOT vacuous` branch below unreachable and reporting the one
    # run that would finally answer the question as the boring expected case.
    # The uppercase sentinel is the spikes' documented verdict marker; the
    # lowercase JSON key is data. Only the former decides.
    #
    # This is the second instance of one class, at the other end of the same
    # interface: file-handle-numa-spike.rs used to print the sentinel
    # unconditionally, before any node was queried, which marked every run
    # vacuous for exactly the same reason. That was fixed in the spike and the
    # matching hazard here was never looked for. Anything that can make this
    # branch fire when the machine is not single-node silences the one signal
    # the job exists to raise, so treat a change to either side as a change to
    # both.
    elseif ($output -cmatch 'VACUOUS') {
        $verdict = 'vacuous on this runner (single NUMA node) -- expected, and the spike said so itself'
    }
    else {
        $verdict = '**NOT vacuous -- this runner has more than one NUMA node. Read the output.**'
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

Observational. A spike's *result* never fails the build; a red step here means a
spike failed to **build** or to **run**, which is a defect in the instrument
rather than a finding about the machine.

"@
    # Written through AppendAllText rather than Add-Content because the bytes
    # have to be identical on both shells. Add-Content's default encoding is the
    # ANSI code page on Windows PowerShell 5.1 and UTF-8 on 7, so an em dash
    # lands as 0x97 under 5.1 and as e2 80 94 under 7; GitHub reads
    # GITHUB_STEP_SUMMARY as UTF-8, so the 5.1 bytes render as mojibake. Adding
    # `-Encoding utf8` fixes that but introduces a second divergence: on 5.1
    # that spelling means UTF-8 *with* BOM, so a summary file that does not
    # already exist gains a BOM there and not on 7. This form has neither.
    $utf8NoBom = New-Object System.Text.UTF8Encoding $false
    [System.IO.File]::AppendAllText($Summary, ($header + ($sections -join "`n")), $utf8NoBom)
}

if ($instrumentFailures -gt 0) {
    Write-Report "$instrumentFailures spike(s) failed to build or run" -Level error
    exit 1
}
Write-Report 'all spikes built and ran'
exit 0
