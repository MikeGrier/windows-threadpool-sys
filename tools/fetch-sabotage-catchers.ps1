# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Adopt the published record of which tests caught each sabotage entry.

.DESCRIPTION
    Fetches the branch [publish-sabotage-catchers.ps1](publish-sabotage-catchers.ps1)
    keeps, and merges its record into the one a sweep here reads and writes, so a
    clone starts from what CI last learned and not from nothing. Without it, the
    first scoped sweep of a manifest in a clone is a full one, because an entry
    with no usable record is always swept (DESIGN-NOTES.md, "sabotage-scope").

    The merge keeps each entry's latest result. Entries of the local record that
    name no commit are dropped first, by Select-SabotageCommittedRecord in
    [common.ps1](common.ps1), which the publisher applies to what it publishes:
    a sweep of a tree that was not a commit records what it learned with none,
    and no scoped sweep ever uses such an entry, but being the newer it would
    displace a published one that can be used.

    Adopting a record cannot make a sweep select fewer entries than it should. An
    entry is used only if the commit it names is in this clone's history, so one
    learned on a commit the clone does not have is as good as absent. The branch
    not existing yet is not an error: nothing is changed.

.PARAMETER Output
    The record to merge into. Defaults to the one a sweep reads, under the
    repository's `.scratch/sabotage`.

.PARAMETER Branch
    The branch that holds the published record.

.PARAMETER Remote
    The remote that holds it.

.PARAMETER RepositoryRoot
    The repository to fetch into. Defaults to the one this script is in.

.OUTPUTS
    Exits 0 when the record is adopted or there is none to adopt, 1 when the
    remote could not be asked.
#>
[CmdletBinding()]
param(
    [string] $Output,
    [string] $Branch = 'sabotage-records',
    [string] $Remote = 'origin',
    [string] $RepositoryRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
if (-not $Output) { $Output = Join-Path $RepositoryRoot '.scratch\sabotage\catchers.json' }

$state = Get-SabotageRecordBranch -RepositoryRoot $RepositoryRoot -Remote $Remote -Branch $Branch
if ($state.Error) {
    Write-Host $state.Error
    exit 1
}
if (-not $state.Present) {
    Write-Host "No $Branch branch on $Remote yet; nothing to adopt."
    exit 0
}
if ($null -eq $state.Text) {
    Write-Host "The $Branch branch on $Remote holds no catchers.json; nothing to adopt."
    exit 0
}

$workDirectory = Join-Path ([System.IO.Path]::GetTempPath()) ('fetch-sabotage-catchers-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $workDirectory | Out-Null
try {
    $published = Join-Path $workDirectory 'published.json'
    [System.IO.File]::WriteAllText($published, $state.Text, [System.Text.UTF8Encoding]::new($false))

    $local = Read-SabotageCatcherRecord -Path $Output
    $committed = Select-SabotageCommittedRecord -Record $local
    $dropped = $committed.Dropped
    $merged = Merge-SabotageCatcherRecord -Records @($committed.Record, (Read-SabotageCatcherRecord -Path $published))
    Write-SabotageCatcherRecord -Path $Output -Record $merged

    $entries = 0
    foreach ($manifest in $merged.Keys) { $entries += $merged[$manifest].Count }
    Write-Host "Adopted $Branch ($($state.Commit)) from ${Remote}: $entries entries across $($merged.Count) manifest(s) in $Output; dropped $dropped local entries that name no commit."
    exit 0
}
finally {
    Remove-Item -LiteralPath $workDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
