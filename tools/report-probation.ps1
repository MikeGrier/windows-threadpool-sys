# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Reports each probation test's runs, passes and current run of passes.

.DESCRIPTION
    Reads the record the long-haul workflow keeps on the `probation-results` branch (or a
    record file, with -Record) and prints it as a markdown table. A candidate is a test whose
    latest result is a pass and whose current run of passes has reached -Threshold: that is
    what promoting it from the `probation` module to `long_haul`, a change to the test, is
    decided on. The report decides nothing.

    The threshold is a working position and not a decision (DESIGN-NOTES.md,
    "probation-results"): how many passes are enough is for the first results to suggest.

.PARAMETER Record
    A record file to report, instead of asking the remote.

.PARAMETER Threshold
    The passes in a row that make a test a candidate.

.PARAMETER Branch
    The branch that holds the record.

.PARAMETER Remote
    The remote that holds it.

.PARAMETER RepositoryRoot
    The repository to ask, for its remote. Defaults to this script's.

.PARAMETER SummaryPath
    A file to append the report to as well, as the Actions step summary.

.OUTPUTS
    Exits 0 with the report; 1 when the remote could not be asked.
#>
[CmdletBinding()]
param(
    [string] $Record,
    [int] $Threshold = 5,
    [string] $Branch,
    [string] $Remote = 'origin',
    [string] $RepositoryRoot,
    [string] $SummaryPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'probation-common.ps1')

if (-not $Branch) { $Branch = $script:ProbationBranch }
if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
if ($Threshold -lt 1) { Write-Host "-Threshold is a number of passes in a row and must be at least 1; got $Threshold."; exit 2 }

if ($Record) {
    if (-not (Test-Path -LiteralPath $Record -PathType Leaf)) { Write-Host "No record at $Record."; exit 2 }
    $held = Read-ProbationRecord -Path $Record
}
else {
    $state = Get-ProbationBranch -RepositoryRoot $RepositoryRoot -Remote $Remote -Branch $Branch -File $script:ProbationFile
    if ($state.Error) { Write-Host $state.Error; exit 1 }
    $held = New-ProbationRecord
    if ($state.Present -and $null -ne $state.Text) { $held = ConvertFrom-ProbationRecordText $state.Text }
}

$lines = @(Format-ProbationReport -Record $held -Threshold $Threshold)
$lines | ForEach-Object { Write-Host $_ }
if ($SummaryPath) { [System.IO.File]::AppendAllText($SummaryPath, (($lines -join "`n") + "`n")) }
exit 0
