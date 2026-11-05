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
    what the run wrote (a line of it that was not an event counts, since a result lost that way
    would be recorded as a test not run), or when an argument could not be passed through cmd;
    both are defects of this check and say nothing about the tests.

.PARAMETER Module
    The tier's module: `long_haul` or `probation`.

.PARAMETER OutputDirectory
    Where `build.json`, `capture.txt` and `outcomes.json` go.

.PARAMETER RepositoryRoot
    The workspace to run in. Defaults to this script's repository.

.PARAMETER TestThreads
    How many tests each executable runs at once, `--test-threads`. One by default: a tier's tests are the
    ones whose scale or timing is the point, so each is run alone and not alongside the others, whose load
    would otherwise be part of what it measures. The flush-barrier stress instruments say so themselves
    (their module docs: `--test-threads 1` "is not optional decoration"), and one of their early defects
    was two of them colliding on a temp file under a parallel harness. Raising it is a choice for a tier
    whose tests are known to be independent.

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
    [ValidateRange(1, 1024)][int] $TestThreads = 1,
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

# One argument as cmd reads it in a batch file: quoted when it holds anything cmd or the C runtime
# would split or act on, with `%` doubled, which a batch file would otherwise expand. A double quote
# inside an argument has no spelling that both cmd and the runtime read the same way, so the caller refuses it.
function ConvertTo-CmdArgument {
    param([string] $Argument)
    $text = $Argument.Replace('%', '%%')
    if ($text -eq '' -or $text -match '[^A-Za-z0-9_\-.:\\/=+,@%]') {
        $text = $text -replace '(\\+)$', '$1$1'
        return '"' + $text + '"'
    }
    return $text
}

foreach ($argument in $CargoArgs) {
    if ($argument.Contains('"')) { Write-Host "A cargo argument with a double quote cannot be passed through cmd: $argument"; exit 2 }
}

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
    # Each argument is quoted for cmd, since the build above took them as an array and a path with a
    # space in it must reach this phase as one argument too. `--color never` because the headers
    # are read as plain text: with `CARGO_TERM_COLOR=always` set, as some CI hosts set it, cargo
    # wraps `Running` in escape codes and no executable would be recognised.
    $quoted = @(@($CargoArgs | ForEach-Object { ConvertTo-CmdArgument $_ }) + @('--color', 'never'))
    $arguments = ($quoted -join ' ') + " -- --ignored $Module`:: --test-threads $TestThreads -Z unstable-options --format json"
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
# An event line that could not be read is a result that was lost, and the record treats a test
# a run does not report as one that was not run. A partial `outcomes.json` would be recorded as
# the whole run, so none is written.
if ($read.Problems.Count -gt 0) {
    foreach ($problem in $read.Problems) { Write-Host "unread: $problem" }
    Write-Host "The outcomes of the $Module tier could not be read from $capturePath`: $($read.Problems.Count) line(s) were not events."
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
