# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Decides whether the long-haul workflow has anything to do.

.DESCRIPTION
    `.github/workflows/long-haul.yml` runs the long-haul and probation test tiers daily, and only when `main` has
    moved since the last run that completed. A run on an unchanged `main` would repeat the last one's result at
    the cost of a workspace build and every long test, so the schedule asks this first.

    The rule:

      - A run the engineer asked for (`workflow_dispatch`), or any trigger but the schedule, always runs.
      - A scheduled run does not run when the most recent earlier run of the workflow that completed -- succeeded,
        failed or timed out -- was on the commit being run now. It runs when there is no such run, or when that
        run was on another commit.
      - A cancelled run did not complete, and a run that was only the gate skipping its work has the commit it
        skipped on, so it counts: the schedule would otherwise run again the next day on the same commit.
      - When the earlier runs cannot be listed the answer is to run, with a warning. The tier exists to run, and
        a failure to ask is not evidence that nothing changed.

    The runs come from `gh run list`, or from `-RunsPath` for a test: a JSON array of objects with
    `databaseId`, `headSha`, `conclusion` and `createdAt`.

.PARAMETER EventName
    The triggering event, `github.event_name`.

.PARAMETER Sha
    The commit this run is on, `github.sha`.

.PARAMETER CurrentRunId
    This run's id, `github.run_id`, so it is never counted as an earlier one.

.PARAMETER Workflow
    The workflow file to list the earlier runs of.

.PARAMETER Branch
    The branch the earlier runs are listed for.

.PARAMETER RunsPath
    A JSON file of earlier runs to use instead of asking GitHub.

.PARAMETER GitHubOutput
    The Actions step-output file. Receives `run=true|false` and `reason=<text>`.

.EXAMPLE
    ./tools/plan-long-haul.ps1 -EventName schedule -Sha $sha -CurrentRunId 1 -RunsPath runs.json
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $EventName,
    [Parameter(Mandatory = $true)][string] $Sha,
    [string] $CurrentRunId = '',
    [string] $Workflow = 'long-haul.yml',
    [string] $Branch = 'main',
    [string] $RunsPath,
    [string] $GitHubOutput
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

# What counts as having completed: it ran to a result. A cancelled run, a skipped one and a run that never
# started say nothing about the commit.
$completedConclusions = @('success', 'failure', 'timed_out')

function Get-LongHaulPlan {
    param([string] $EventName, [string] $Sha, [string] $CurrentRunId, [object[]] $Runs)
    if ($EventName -ne 'schedule') {
        return [pscustomobject]@{ Run = $true; Reason = "a $EventName run is always taken" }
    }
    $earlier = @($Runs |
            Where-Object { "$($_.databaseId)" -ne $CurrentRunId -and $completedConclusions -contains [string]$_.conclusion } |
            Sort-Object { [datetime]$_.createdAt } -Descending)
    if ($earlier.Count -eq 0) {
        return [pscustomobject]@{ Run = $true; Reason = 'no earlier run completed' }
    }
    $last = $earlier[0]
    if ([string]$last.headSha -eq $Sha) {
        return [pscustomobject]@{ Run = $false; Reason = "main has not moved since run $($last.databaseId), which completed ($($last.conclusion)) on $Sha" }
    }
    return [pscustomobject]@{ Run = $true; Reason = "main moved from $($last.headSha) (run $($last.databaseId)) to $Sha" }
}

$runs = @()
$plan = $null
if ($EventName -eq 'schedule') {
    $json = $null
    if ($RunsPath) {
        if (-not (Test-Path -LiteralPath $RunsPath -PathType Leaf)) { Write-Host "No runs file at $RunsPath."; exit 2 }
        $json = [System.IO.File]::ReadAllText($RunsPath)
    }
    else {
        $asked = Invoke-NativeSplit { gh run list --workflow $Workflow --branch $Branch --status completed --limit 30 --json databaseId,headSha,conclusion,createdAt }
        if ($asked.ExitCode -ne 0) {
            Write-Host "::warning::could not list earlier runs of $Workflow (gh exited $($asked.ExitCode): $($asked.Stderr)); running"
            $plan = [pscustomobject]@{ Run = $true; Reason = 'the earlier runs could not be listed' }
        }
        else { $json = ($asked.Stdout -join "`n") }
    }
    # Windows PowerShell 5.1 hands back a parsed array as ONE object, so it is enumerated to get the runs.
    if ($null -ne $json -and $json.Trim().Length -gt 0) {
        $parsed = $json | ConvertFrom-Json
        $runs = @($parsed | ForEach-Object { $_ } | Where-Object { $null -ne $_ })
    }
}

if ($null -eq $plan) {
    $plan = Get-LongHaulPlan -EventName $EventName -Sha $Sha -CurrentRunId $CurrentRunId -Runs $runs
}

$verdict = if ($plan.Run) { 'run' } else { 'skip' }
Write-Host "long-haul: $verdict -- $($plan.Reason)"
if ($GitHubOutput) {
    [System.IO.File]::AppendAllText($GitHubOutput, "run=$($plan.Run.ToString().ToLowerInvariant())`nreason=$($plan.Reason)`n")
}
exit 0
