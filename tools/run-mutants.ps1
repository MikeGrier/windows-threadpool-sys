# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs cargo-mutants with the settings this workspace needs, and without a
    crashing mutant wedging the run behind a modal dialog.

.DESCRIPTION
    Three things go wrong when cargo-mutants is invoked directly here, and this
    wrapper exists because each one has already cost a run.

    **A crashing mutant pops a modal dialog and stalls the entire run.** Mutation
    testing is expected to produce weird programs, and some of them are not
    merely wrong but memory-unsafe -- inverting the `ERROR_IO_PENDING` check in
    `arm_detailed_read` makes the thread pool cancel its accounting for an I/O
    the kernel is still going to complete, so the completion lands in a freed
    buffer. The crash is fine and expected. What is not fine is the operating
    system's response: a modal "Application Error" box offering to debug, which
    nothing in an automated run will ever click.

    Two dialogs can appear, and they have different switches:

    - The **WER "Application Error" box**, controlled by `DontShowUI` under
      HKCU. That is what this wrapper sets and restores.
    - The **JIT debugger prompt** ("Click on CANCEL to debug the program"),
      controlled by `AeDebug\Debugger` and `AeDebug\Auto` under HKLM. On this
      machine `vsjitdebugger.exe` is registered with `Auto` unset, which means
      prompt. `DontShowUI` suppressed it in practice -- a 144-mutant sweep with
      sixteen crashes ran to completion -- but the key is HKLM and would need
      elevation to change, so if a run ever stalls again with a debugger prompt,
      that is the knob to look at rather than this one.

    **Measured cost, which is the reason this is not optional.** With the dialog
    suppressed, a crashing mutant costs 7.9s and 10.5s (the two that crashed in
    the watcher.rs sweep) and is recorded as `CaughtMutant`. Without it the run
    does not merely slow down: it stops advancing entirely and never resumes,
    because `--timeout` bounds the *test*, not a process sitting on a message
    pump. The first watcher.rs attempt wedged after 33 mutants and had to be
    killed by hand.

    Deliberately **not** setting WER's `Disabled` as well: with `DontShowUI` on,
    zero crash dumps were collected during that sweep, so there is no report
    collection left to switch off and the extra registry write would be
    unmeasured complexity.

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

if ($previous -eq 1) {
    # Left set by a previous run that was killed before its `finally` could run
    # -- the one hole in the restore, since a hard kill runs no cleanup. Say so,
    # because silently treating it as the user's own preference would restore it
    # to 1 afterwards and leave the machine permanently changed by a crash.
    Write-Host "NOTE: DontShowUI was already 1. If a previous run was killed, clear it after:" -ForegroundColor Yellow
    Write-Host "  Remove-ItemProperty '$werKey' -Name DontShowUI" -ForegroundColor Yellow
}

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
    # Anything WER or the JIT debugger left holding a dead test process. With
    # the dialog suppressed these should not appear at all; killing them is
    # insurance against a stale one pinning a target file and failing the next
    # build with a confusing "access denied".
    foreach ($name in 'WerFault', 'WerFaultSecure', 'vsjitdebugger') {
        Get-Process -Name $name -ErrorAction SilentlyContinue |
            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    }

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
