# Copyright (c) 2026 Mike Grier
<#
.SYNOPSIS
    Checks that every release-managed crate can actually be published.

.DESCRIPTION
    release-please decides which crates get versioned and tagged;
    publish-crate.yml decides which tags trigger a publish. Nothing connects
    them, so a crate can be added to the first and forgotten in the second --
    and the failure is silent in the worst way: release-please raises the PR,
    the tag is created, the workflow simply does not run, and the crate is
    "released" everywhere except on crates.io.

    That happened to windows-waitable-queues, and was found by review rather
    than by any check. This script is the check.

    A crate may be deliberately absent from release-please -- windows-placement-
    probe ships as a downloadable binary and is not on crates.io yet -- so the
    comparison is one-directional: everything release-please manages must be
    publishable. The reverse is allowed.
#>
[CmdletBinding()]
param(
    [string] $RepositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
)

$ErrorActionPreference = 'Stop'

$configPath = Join-Path $RepositoryRoot 'release-please-config.json'
$workflowPath = Join-Path $RepositoryRoot '.github/workflows/publish-crate.yml'

foreach ($required in @($configPath, $workflowPath)) {
    if (-not (Test-Path $required)) {
        throw "cannot check publication: $required is missing"
    }
}

$managed = (Get-Content $configPath -Raw | ConvertFrom-Json).packages.PSObject.Properties.Name |
    ForEach-Object { Split-Path $_ -Leaf } |
    Sort-Object

$workflow = Get-Content $workflowPath -Raw

$missing = @()
foreach ($crate in $managed) {
    $hasTag = $workflow -match [regex]::Escape("'$crate-v*'")
    $hasDispatch = $workflow -match ("(?m)^\s+- " + [regex]::Escape($crate) + "\s*$")
    if (-not $hasTag) { $missing += "$crate : no tag trigger, so its release tag would publish nothing" }
    if (-not $hasDispatch) { $missing += "$crate : not a workflow_dispatch choice, so it cannot be published by hand either" }
}

if ($missing.Count -gt 0) {
    Write-Host "Release-managed crates that cannot be published:" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    Write-Host ''
    Write-Host "Add them to .github/workflows/publish-crate.yml, in both the tag list and the dispatch choices."
    exit 1
}

Write-Host "All $($managed.Count) release-managed crates have a publish trigger." -ForegroundColor Green
exit 0
