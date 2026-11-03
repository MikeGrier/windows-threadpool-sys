# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Publish a record of which tests caught each sabotage entry to a branch of its own.

.DESCRIPTION
    The record (see "Which tests caught each entry" in [common.ps1](common.ps1))
    is derived data: a sweep writes it, and a scoped sweep reads it to run fewer
    entries. CI keeps it in the Actions cache, which evicts and which no clone can
    read. This also publishes it to a branch, `sabotage-records` by default, which
    holds nothing else and shares no history with `main`, so a clone can fetch it
    ([fetch-sabotage-catchers.ps1](fetch-sabotage-catchers.ps1)) and `main` gains
    no commit per merge. The decision is in DESIGN-NOTES.md, "sabotage-records".

    What is published is the merge of the record given and the record the branch
    already holds, each entry's latest result kept, so a publisher that swept a few
    manifests does not discard what another swept. When that merge is what the
    branch already holds, nothing is committed.

    A blob, a tree and a commit are written with git plumbing and pushed as a
    fast-forward: no working tree, index or checked-out branch is read or changed,
    so this runs from any checkout, a developer's included. A push the remote
    rejects -- another publisher got there first -- is retried from a fresh fetch,
    so the record that publisher pushed is merged and not overwritten. The push is
    never forced.

.PARAMETER Record
    The record to publish.

.PARAMETER Branch
    The branch that holds the published record.

.PARAMETER Remote
    The remote that holds it.

.PARAMETER RepositoryRoot
    The repository to publish from, for its remote. Defaults to the one this
    script is in.

.PARAMETER Attempts
    How many times to fetch, merge and push before giving up.

.PARAMETER RetryDelaySeconds
    How long to wait between attempts.

.PARAMETER AuthorName
    The name on the commit.

.PARAMETER AuthorEmail
    The email on the commit.

.PARAMETER BeforePush
    A script block run once between merging and pushing, on the first attempt.
    A test seam: it is how the tests make another publisher win the race, through
    the same code a real race goes through.

.OUTPUTS
    Exits 0 when the branch holds the record, or already did; 1 when it could not
    be made to.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string] $Record,
    [string] $Branch = 'sabotage-records',
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

. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
if ($Attempts -lt 1) { Write-Host "-Attempts must be at least 1; got $Attempts."; exit 2 }
if (-not (Test-Path -LiteralPath $Record -PathType Leaf)) { Write-Host "No record to publish at $Record."; exit 2 }

# Names in the published tree. Changing either is a change to what a clone
# fetches, so both are named once.
$recordFile = 'catchers.json'
$readmeFile = 'README.md'

# Seven-bit ASCII, and no link: it is read on the branch, where a relative link
# to a file on `main` resolves to nothing.
$readme = @'
# sabotage-records

This branch holds one generated file, `catchers.json`: for each sabotage entry,
which tests caught it, when, and at which commit of `main`'s history.

It is derived data, written by the `sabotage sweeps` workflow after a sweep and
read by `tools/run-sabotage.ps1 -Since` to run fewer entries. Do not edit it by
hand, and do not merge it into anything.

It shares no history with `main`. Its history may be squashed or deleted at any
time: a record is only ever a hint, and a missing one makes a scoped sweep run
more entries, never fewer.

To use it in a clone, run `tools/fetch-sabotage-catchers.ps1`. An entry is used
only if the commit it names is in the clone's history, so a record from a commit
the clone does not have changes nothing.

The reasoning is in DESIGN-NOTES.md on `main`, under "sabotage-records".
'@ -replace "`r`n", "`n"

$workDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('publish-sabotage-catchers-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $workDirectory | Out-Null

# Writes `Record` as the writer does, so two records with the same content are
# the same bytes and can be compared as text.
function ConvertTo-RecordText {
    param([hashtable] $Value)
    $path = Join-Path $workDirectory ([guid]::NewGuid().ToString('N') + '.json')
    Write-SabotageCatcherRecord -Path $path -Record $Value
    return [System.IO.File]::ReadAllText($path)
}

# Writes `Text` to the object database and returns its id.
function Write-Blob {
    param([string] $Text)
    $path = Join-Path $workDirectory ([guid]::NewGuid().ToString('N') + '.blob')
    [System.IO.File]::WriteAllText($path, $Text, [System.Text.UTF8Encoding]::new($false))
    $id = @(Invoke-NativeStdout { git -C $RepositoryRoot hash-object -w --no-filters -- $path }) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $id) { throw 'git could not write a blob.' }
    return ([string]$id).Trim()
}

# One attempt. Returns $null when the branch holds the record, or why this
# attempt did not get it there.
function Publish-Once {
    param([bool] $First)
    $state = Get-SabotageRecordBranch -RepositoryRoot $RepositoryRoot -Remote $Remote -Branch $Branch
    if ($state.Error) { return $state.Error }

    $current = New-SabotageCatcherRecord
    if ($state.Present -and $null -ne $state.Text) {
        $held = Join-Path $workDirectory 'held.json'
        [System.IO.File]::WriteAllText($held, $state.Text, [System.Text.UTF8Encoding]::new($false))
        $current = Read-SabotageCatcherRecord -Path $held
    }
    $merged = Merge-SabotageCatcherRecord -Records @($current, (Read-SabotageCatcherRecord -Path $Record))
    $mergedText = ConvertTo-RecordText $merged
    if ($state.Present -and $null -ne $state.Text -and [string]::Equals((ConvertTo-RecordText $current), $mergedText, [System.StringComparison]::Ordinal)) {
        Write-Host "The $Branch branch on $Remote already holds this record; nothing to publish."
        return $null
    }

    $recordBlob = Write-Blob $mergedText
    $readmeBlob = Write-Blob ($readme + "`n")
    # The tree is built in an index of its own and read out with `write-tree`, and
    # not piped to `mktree`: PowerShell ends every line it pipes to a native command
    # with CRLF, which `mktree` takes as part of the name, so the files would be
    # named `catchers.json\r` and nothing could find them. `GIT_INDEX_FILE` keeps
    # the repository's own index out of it.
    $previousIndex = $env:GIT_INDEX_FILE
    $tree = $null
    try {
        $env:GIT_INDEX_FILE = Join-Path $workDirectory ('index-' + [guid]::NewGuid().ToString('N'))
        foreach ($item in @(@($readmeBlob, $readmeFile), @($recordBlob, $recordFile))) {
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

    $count = 0
    foreach ($manifest in $merged.Keys) { $count += $merged[$manifest].Count }
    $message = "sabotage-records: $count entries across $($merged.Count) manifest(s)"
    $arguments = @('-c', "user.name=$AuthorName", '-c', "user.email=$AuthorEmail", '-c', 'commit.gpgsign=false',
        'commit-tree', ([string]$tree).Trim())
    if ($state.Present) { $arguments += @('-p', $state.Commit) }
    $arguments += @('-m', $message)
    $commit = @(Invoke-NativeStdout { git -C $RepositoryRoot @arguments }) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0 -or -not $commit) { return 'git could not write a commit.' }
    $commit = ([string]$commit).Trim()

    if ($First -and $BeforePush) { & $BeforePush }

    # Never forced: a rejection means the branch moved since the fetch, and the
    # next attempt merges what moved it.
    $pushed = Invoke-NativeSplit { git -C $RepositoryRoot push --quiet $Remote "${commit}:refs/heads/$Branch" }
    if ($pushed.ExitCode -ne 0) { return "git push to '$Remote' was refused (exit $($pushed.ExitCode)): $($pushed.Stderr)" }
    Write-Host "Published $count entries across $($merged.Count) manifest(s) to $Branch on $Remote as $commit."
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
    Write-Host "Could not publish the record to $Branch on $Remote after $Attempts attempt(s): $why"
    exit 1
}
finally {
    Remove-Item -LiteralPath $workDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
