# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Applies a probation run's outcomes to the record on the `probation-results` branch.

.DESCRIPTION
    The record keeps each probation test's runs, passes and current run of passes across
    runs (probation-common.ps1). It is derived data, so it lives on a branch of its own that
    shares no history with `main`, the same arrangement as `sabotage-records`; the decision
    is in DESIGN-NOTES.md, "probation-results".

    The branch's record has the run applied and is pushed as a fast-forward. A run the
    record already has as its latest is not applied again, so a retry from a fresh fetch
    cannot count it twice, and a run that selected no test while the record holds none
    commits nothing. A blob, a tree and a commit are written with git plumbing, in an index
    of its own, so no working tree, index or checked-out branch is read or changed. A push the
    remote rejects is retried from a fresh fetch. The push is never forced.

.PARAMETER Outcomes
    The `outcomes.json` that run-tier.ps1 wrote.

.PARAMETER RunId
    Names the run: applying it twice is applying it once. A re-run of a workflow's jobs is a
    new run of the tests, so it needs a new id.

.PARAMETER Commit
    The commit of `main`'s history the run tested.

.PARAMETER At
    When the run was, as text. Defaults to now, in UTC.

.PARAMETER Branch
    The branch that holds the record.

.PARAMETER Remote
    The remote that holds it.

.PARAMETER RepositoryRoot
    The repository to publish from, for its remote. Defaults to this script's.

.PARAMETER Attempts
    How many times to fetch, apply and push before giving up.

.PARAMETER RetryDelaySeconds
    How long to wait between attempts.

.PARAMETER AuthorName
    The name on the commit.

.PARAMETER AuthorEmail
    The email on the commit.

.PARAMETER BeforePush
    A script block run once between applying and pushing, on the first attempt. A test seam:
    it is how the tests make another publisher win the race, through the code a real race
    goes through.

.OUTPUTS
    Exits 0 when the branch holds the run, or already did; 1 when it could not be made to;
    2 when it was not given what it needs.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Outcomes,
    [Parameter(Mandatory = $true)][string] $RunId,
    [Parameter(Mandatory = $true)][string] $Commit,
    [string] $At,
    [string] $Branch,
    [string] $Remote = 'origin',
    [string] $RepositoryRoot,
    [int] $Attempts = 5,
    [int] $RetryDelaySeconds = 5,
    [string] $AuthorName = 'github-actions[bot]',
    [string] $AuthorEmail = '41898282+github-actions[bot]@users.noreply.github.com',
    [scriptblock] $BeforePush
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'probation-common.ps1')

if (-not $Branch) { $Branch = $script:ProbationBranch }
if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
if (-not $At) { $At = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ') }
if ($Attempts -lt 1) { Write-Host "-Attempts must be at least 1; got $Attempts."; exit 2 }
if (-not (Test-Path -LiteralPath $Outcomes -PathType Leaf)) { Write-Host "No outcomes to publish at $Outcomes."; exit 2 }

$document = [System.IO.File]::ReadAllText($Outcomes) | ConvertFrom-Json
if ($document.version -ne $script:ProbationRecordVersion) {
    Write-Host "$Outcomes is version $($document.version); this reads version $($script:ProbationRecordVersion)."
    exit 2
}
$incoming = @($document.outcomes | ForEach-Object { if ($null -ne $_) { [pscustomobject]@{ Key = [string]$_.key; Outcome = [string]$_.outcome; Note = '' } } })

# Seven-bit ASCII, and no link: it is read on the branch, where a relative link to a file on
# `main` resolves to nothing.
$readme = @'
# probation-results

This branch holds one generated file, `results.json`: for each test on probation, how many
runs it has been in, how many it passed, how many passes in a row it is on, and the last
run and commit it was seen at.

It is derived data, written by the `long-haul` workflow after each run of the probation
tier. Do not edit it by hand, and do not merge it into anything.

It shares no history with `main`. Its history may be squashed or deleted at any time, and
doing so only restarts the count of each test's runs.

To read it, run `tools/report-probation.ps1`. A test is promoted from the `probation`
module to `long_haul` by a change to the test, never by this file.

The reasoning is in DESIGN-NOTES.md on `main`, under "probation-results".
'@ -replace "`r`n", "`n"

$workDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('publish-probation-results-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $workDirectory | Out-Null

# Writes `Text` to the object database and returns its id.
function Write-Blob {
    param([string] $Text)
    $path = Join-Path $workDirectory ([guid]::NewGuid().ToString('N') + '.blob')
    [System.IO.File]::WriteAllText($path, $Text, [System.Text.UTF8Encoding]::new($false))
    $id = @(Invoke-NativeStdout { git -C $RepositoryRoot hash-object -w --no-filters -- $path }) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $id) { throw 'git could not write a blob.' }
    return ([string]$id).Trim()
}

# One attempt. Returns $null when the branch holds the run, or why this attempt did not get
# it there.
function Publish-Once {
    param([bool] $First)
    $state = Get-ProbationBranch -RepositoryRoot $RepositoryRoot -Remote $Remote -Branch $Branch -File $script:ProbationFile
    if ($state.Error) { return $state.Error }

    $record = New-ProbationRecord
    if ($state.Present -and $null -ne $state.Text) { $record = ConvertFrom-ProbationRecordText $state.Text }
    if ($record.latestRun -eq $RunId) {
        Write-Host "The $Branch branch on $Remote already holds run $RunId; nothing to publish."
        return $null
    }
    if ($incoming.Count -eq 0 -and $record.tests.Count -eq 0) {
        Write-Host "Nothing to publish to $Branch on ${Remote}: the run selected no test and no test is on record."
        return $null
    }
    $record = Add-ProbationRun -Record $record -Outcomes $incoming -RunId $RunId -Commit $Commit -At $At
    $recordBlob = Write-Blob (ConvertTo-ProbationRecordText $record)
    $readmeBlob = Write-Blob ($readme + "`n")

    # The tree is built in an index of its own and read out with `write-tree`, and not piped
    # to `mktree`: PowerShell ends every line it pipes to a native command with CRLF, which
    # `mktree` takes as part of the name. `GIT_INDEX_FILE` keeps the repository's own index
    # out of it.
    $previousIndex = $env:GIT_INDEX_FILE
    $tree = $null
    try {
        $env:GIT_INDEX_FILE = Join-Path $workDirectory ('index-' + [guid]::NewGuid().ToString('N'))
        foreach ($item in @(@($readmeBlob, 'README.md'), @($recordBlob, $script:ProbationFile))) {
            $spec = "100644,$($item[0]),$($item[1])"
            Invoke-Native { git -C $RepositoryRoot update-index --add --cacheinfo $spec } | Out-Null
            if ($LASTEXITCODE -ne 0) { return "git could not add $($item[1]) to a tree." }
        }
        $tree = @(Invoke-NativeStdout { git -C $RepositoryRoot write-tree }) | Select-Object -First 1
        if ($LASTEXITCODE -ne 0 -or -not $tree) { return 'git could not write a tree.' }
    }
    finally {
        if ($null -eq $previousIndex) { Remove-Item Env:\GIT_INDEX_FILE -ErrorAction SilentlyContinue } else { $env:GIT_INDEX_FILE = $previousIndex }
    }

    $message = "probation-results: run $RunId, $($record.tests.Count) test(s)"
    $arguments = @('-c', "user.name=$AuthorName", '-c', "user.email=$AuthorEmail", '-c', 'commit.gpgsign=false',
        'commit-tree', ([string]$tree).Trim())
    if ($state.Present) { $arguments += @('-p', $state.Commit) }
    $arguments += @('-m', $message)
    $made = @(Invoke-NativeStdout { git -C $RepositoryRoot @arguments }) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $made) { return 'git could not write a commit.' }
    $made = ([string]$made).Trim()

    if ($First -and $BeforePush) { & $BeforePush }

    # Never forced: a rejection means the branch moved since the fetch, and the next attempt
    # applies the run on top of what moved it.
    $pushed = Invoke-NativeSplit { git -C $RepositoryRoot push --quiet $Remote "${made}:refs/heads/$Branch" }
    if ($pushed.ExitCode -ne 0) { return "git push to '$Remote' was refused (exit $($pushed.ExitCode)): $($pushed.Stderr)" }
    Write-Host "Published run $RunId ($($incoming.Count) outcome(s)) to $Branch on $Remote as $made."
    return $null
}

try {
    $why = $null
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        $why = Publish-Once -First ($attempt -eq 1)
        if (-not $why) { exit 0 }
        Write-Host "Attempt $attempt of ${Attempts}: $why"
        if ($attempt -lt $Attempts -and $RetryDelaySeconds -gt 0) { Start-Sleep -Seconds $RetryDelaySeconds }
    }
    Write-Host "Could not publish run $RunId to $Branch on $Remote after $Attempts attempt(s): $why"
    exit 1
}
finally {
    Remove-Item -LiteralPath $workDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
