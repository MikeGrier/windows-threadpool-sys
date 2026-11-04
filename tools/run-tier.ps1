# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs one test tier and records what each of its tests did.

.DESCRIPTION
    `.github/workflows/long-haul.yml` runs the long-haul tier and then the probation tier
    (DEVELOPMENT.md, "Test tiers"). Both go through here, so the command that selects a
    tier is written once: the library, binary, integration and example test targets, with
    `--ignored` and the tier's module filter. Doctests are left out; a doctest cannot sit in
    a `long_haul` or `probation` module.

    The tests run through libtest's JSON output, so what each one did is read from events
    and not from the text a person reads. That format is unstable, and is asked for with
    `RUSTC_BOOTSTRAP=1` as tools/check-test-tiers.ps1 asks for it; the build is made without
    it, and setting it for the run does not rebuild anything. cargo's output and the events
    go to one file by one redirect so that cargo's `Running` header, which says which
    executable the events that follow belong to, stays in order with them.

    Writes `outcomes.json` to the output directory: each test's key, its outcome (`pass` or
    `fail`) and, for a failure, what it printed. Prints the failures. Exits with cargo's own
    exit code, so a failing test fails the step; 2 when the outcomes could not be read from
    what the run wrote, which is a defect of this check and says nothing about the tests.

.PARAMETER Module
    The tier's module: `long_haul` or `probation`.

.PARAMETER OutputDirectory
    Where `build.json`, `capture.txt` and `outcomes.json` go.

.PARAMETER RepositoryRoot
    The workspace to run in. Defaults to this script's repository.

.PARAMETER CargoArgs
    The arguments selecting what cargo builds and runs, before `--`.

.EXAMPLE
    ./tools/run-tier.ps1 -Module probation -OutputDirectory .scratch/tiers/probation
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('long_haul', 'probation')][string] $Module,
    [Parameter(Mandatory = $true)][string] $OutputDirectory,
    [string] $RepositoryRoot,
    [string[]] $CargoArgs = @('--workspace', '--all-features', '--locked', '--no-fail-fast', '--lib', '--bins', '--tests', '--examples')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'probation-common.ps1')

if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$buildPath = Join-Path $OutputDirectory 'build.json'
$capturePath = Join-Path $OutputDirectory 'capture.txt'
$outcomesPath = Join-Path $OutputDirectory 'outcomes.json'
foreach ($stale in @($buildPath, $capturePath, $outcomesPath)) { Remove-Item -LiteralPath $stale -Force -ErrorAction SilentlyContinue }

Push-Location -LiteralPath $RepositoryRoot
try {
    # The build, for the map from each executable to the target it came from. Incremental
    # when the tests were built already, which they are in the workflow.
    $built = Invoke-NativeSplit { cargo test @CargoArgs --no-run --message-format=json }
    [System.IO.File]::WriteAllLines($buildPath, [string[]]@($built.Stdout), [System.Text.UTF8Encoding]::new($false))
    if ($built.ExitCode -ne 0) {
        Write-Host $built.Stderr
        Write-Host "cargo could not build the tests (exit $($built.ExitCode))."
        exit $built.ExitCode
    }

    # A batch file, so that the redirect is cmd's and both streams are written to one
    # handle, in the order they happen. A PowerShell `2>&1` reads them on separate threads.
    $runner = Join-Path $OutputDirectory 'run.cmd'
    $arguments = ($CargoArgs -join ' ') + " -- --ignored $Module`:: -Z unstable-options --format json"
    [System.IO.File]::WriteAllText($runner, "@echo off`r`ncargo test $arguments > `"$capturePath`" 2>&1`r`nexit /b %ERRORLEVEL%`r`n")
    $previous = $env:RUSTC_BOOTSTRAP
    $env:RUSTC_BOOTSTRAP = '1'
    try {
        & $runner
        $cargoExit = $LASTEXITCODE
    }
    finally {
        if ($null -eq $previous) { Remove-Item Env:\RUSTC_BOOTSTRAP -ErrorAction SilentlyContinue } else { $env:RUSTC_BOOTSTRAP = $previous }
    }
}
finally { Pop-Location }

try {
    $map = Get-ProbationTargetMap -BuildJsonPath $buildPath -RepositoryRoot $RepositoryRoot
    $read = ConvertFrom-ProbationCapture -CapturePath $capturePath -TargetMap $map
}
catch {
    Write-Host "The outcomes of the $Module tier could not be read from $capturePath`: $($_.Exception.Message)"
    exit 2
}

$passed = @($read.Outcomes | Where-Object { $_.Outcome -eq $script:ProbationPass })
$failed = @($read.Outcomes | Where-Object { $_.Outcome -eq $script:ProbationFail })
foreach ($outcome in $failed) {
    Write-Host "FAILED $($outcome.Key)"
    if ($outcome.Note) { Write-Host (($outcome.Note.TrimEnd() -split "`r?`n" | ForEach-Object { "    $_" }) -join "`n") }
}
foreach ($problem in $read.Problems) { Write-Host "unread: $problem" }
Write-Host "$Module tier: $($passed.Count) passed, $($failed.Count) failed."

$document = [ordered]@{
    version  = $script:ProbationRecordVersion
    module   = $Module
    outcomes = @($read.Outcomes | ForEach-Object { [ordered]@{ key = $_.Key; outcome = $_.Outcome } })
}
[System.IO.File]::WriteAllText($outcomesPath, (($document | ConvertTo-Json -Depth 5) -replace "`r`n", "`n") + "`n", [System.Text.UTF8Encoding]::new($false))
exit $cargoExit
