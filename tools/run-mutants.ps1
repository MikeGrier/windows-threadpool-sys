# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs cargo-mutants with the settings this workspace needs, and without a
    crashing mutant wedging the run behind a modal dialog.

.DESCRIPTION
    Three things go wrong when cargo-mutants is invoked directly here, and this
    wrapper exists because each one has already cost a run.

    **A crashing mutant pops a Windows Error Reporting dialog.** Some mutants do
    not merely fail a test, they produce genuine memory unsafety -- inverting
    the `ERROR_IO_PENDING` check in `arm_detailed_read` makes the thread pool
    cancel its accounting for an I/O the kernel is still going to complete, so
    the completion lands in a freed buffer. The process dies at address zero,
    WER shows a modal "Application Error" box, and the run stops dead waiting
    for a human to click OK. `--timeout` does not save it: the process is alive,
    blocked on a dialog, and the crash never reaches the log as a signature that
    a scan would find. This wrapper sets `DontShowUI` for the duration and puts
    it back afterwards, so a crash is just a non-zero exit code and cargo-mutants
    records it as caught.

    **Features go to cargo-mutants itself, not after `--`, and never both.**
    cargo-mutants mutates source, so it happily mutates a module behind a
    feature that is off -- the mutation lands in code that is never compiled,
    the suite passes trivially, and the result is recorded as `missed`.
    Measured here: 57 of 61 survivors in one crate and 147 of 247 in another
    were this and nothing else. Passing it on *both* sides does not work at all:
    cargo-mutants forwards its trailing arguments onto the same `cargo test`, so
    the flag lands twice on one command line and cargo refuses it outright
    (`the argument '--all-features' cannot be used multiple times`), killing the
    run in the baseline. Passing it only *after* `--` reaches the test run but
    not the `--no-run` build, so the first build is thrown away and rebuilt with
    features on, and the build/test split cargo-mutants reports times a
    configuration it never tested.

    **`-j 2`, not more.** This workspace has timing-sensitive tests; under heavy
    parallel load one can fail for want of a CPU rather than because it detected
    the mutant, which cargo-mutants records as a *false* caught.

.PARAMETER Package
    Crate to mutate. Defaults to the file-watcher, the crate this was built for.

.PARAMETER File
    Repository-relative source file to scope to. Strongly recommended: a
    whole-crate sweep here takes hours, where one file takes about fifteen
    minutes and gives a result you can act on and re-verify the same day.

.PARAMETER Jobs
    Parallel jobs. See above before raising it.

.PARAMETER TimeoutSeconds
    Per-mutant test timeout.

.PARAMETER OutputDirectory
    Where to write `mutants.out`. Defaults under `.scratch/`, so a run never
    overwrites a previous run's results in the repository root -- which has
    already lost one analysis.

.EXAMPLE
    .\tools\run-mutants.ps1 -File crates/windows-file-watcher/src/watcher.rs
#>
[CmdletBinding()]
param(
    [string] $Package = 'windows-file-watcher',
    [string] $File,
    [int] $Jobs = 2,
    [int] $TimeoutSeconds = 120,
    [string] $OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repo = (git rev-parse --show-toplevel).Replace('/', '\')
if (-not $OutputDirectory) {
    $leaf = if ($File) { [System.IO.Path]::GetFileNameWithoutExtension($File) } else { $Package }
    $OutputDirectory = Join-Path $repo ".scratch\mutants-$leaf"
}

$werKey = 'HKCU:\Software\Microsoft\Windows\Windows Error Reporting'
$hadKey = Test-Path $werKey
# `.GetValue(name, $null)` rather than `Get-ItemProperty -Name`: under
# `Set-StrictMode -Version Latest` the latter throws when the property is absent
# (which is the default state of this one), so the wrapper died before it could
# do anything -- reading the "is it already set?" question was itself the bug.
$previous = if ($hadKey) { (Get-Item $werKey).GetValue('DontShowUI', $null) } else { $null }

try {
    if (-not $hadKey) { New-Item -Path $werKey -Force | Out-Null }
    Set-ItemProperty -Path $werKey -Name DontShowUI -Value 1 -Type DWord
    Write-Host "WER dialogs suppressed for this run (DontShowUI=1)." -ForegroundColor Cyan

    $argv = @('mutants', '-p', $Package, '-j', $Jobs, '--timeout', $TimeoutSeconds,
        '--output', $OutputDirectory, '--all-features')
    if ($File) { $argv += @('--file', $File) }

    Write-Host "cargo $($argv -join ' ')" -ForegroundColor DarkGray
    & cargo @argv
    $code = $LASTEXITCODE
}
finally {
    if ($null -ne $previous) {
        Set-ItemProperty -Path $werKey -Name DontShowUI -Value $previous -Type DWord
    }
    else {
        Remove-ItemProperty -Path $werKey -Name DontShowUI -ErrorAction SilentlyContinue
        if (-not $hadKey) { Remove-Item -Path $werKey -ErrorAction SilentlyContinue }
    }
    Write-Host "WER dialog setting restored." -ForegroundColor Cyan
}

$out = Join-Path $OutputDirectory 'mutants.out'
foreach ($name in 'caught', 'missed', 'timeout', 'unviable') {
    $path = Join-Path $out "$name.txt"
    $count = if (Test-Path $path) { (Get-Content $path | Measure-Object -Line).Lines } else { 0 }
    "{0,-9} {1}" -f $name, $count
}
Write-Host "results: $out" -ForegroundColor DarkGray

# cargo-mutants exits non-zero when anything survived, which is the normal
# outcome of an investigative run rather than a failure of this script.
exit $code
