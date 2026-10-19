# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Merge records of which tests caught each sabotage entry into one.

.DESCRIPTION
    Each shard of a sweep writes a record of the entries it ran, on top of the
    record it started from (see "Which tests caught each entry" in
    [common.ps1](common.ps1)). This keeps, for every entry any input has, the
    latest result, so the shards of one run and the record before them become
    the record after them. A missing or unreadable input counts as empty.

    The sweep workflow runs this once per run, after every shard, and keeps the
    result in the Actions cache for the next run.

.PARAMETER Path
    The records to merge, in any order; directories are searched for
    `catchers.json` files.

.PARAMETER Output
    Where to write the merged record.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string[]] $Path,
    [Parameter(Mandatory = $true)] [string] $Output
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

$files = @(foreach ($item in $Path) {
        if (Test-Path -LiteralPath $item -PathType Container) {
            Get-ChildItem -LiteralPath $item -Recurse -Filter 'catchers.json' -File | ForEach-Object { $_.FullName }
        }
        else { $item }
    })
# Counted before the write, which may be to one of the inputs.
$present = @($files | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf }).Count
$records = @(foreach ($file in $files) { Read-SabotageCatcherRecord -Path $file })
$merged = Merge-SabotageCatcherRecord -Records $records
Write-SabotageCatcherRecord -Path $Output -Record $merged

$entries = 0
foreach ($manifest in $merged.Keys) { $entries += $merged[$manifest].Count }
Write-Host "Merged $present of $($files.Count) record(s) found: $entries entries across $($merged.Count) manifest(s), written to $Output."
