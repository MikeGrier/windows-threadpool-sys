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

# The registry the publish job consults before releasing a crate with
# workspace-sibling dependencies: each named crate must appear on crates.io at
# the required version first. A release-managed crate missing from it is not
# waited for, so a dependent can race its sibling's tag and fail `cargo publish`
# instead of pausing -- which is why this list is checked here rather than kept
# by hand and hoped for.
$registryLine = $workflow -split "`n" | Where-Object { $_ -match 'workspace_crates="' } | Select-Object -First 1
$registry = @()
if ($registryLine -match 'workspace_crates="([^"]*)"') {
    $registry = ($Matches[1] -split '\s+') | Where-Object { $_ }
}

$missing = @()
if (-not $registryLine) {
    $missing += "the workflow has no workspace_crates registry, so no sibling dependency is ever waited for"
}
foreach ($crate in $managed) {
    # **Anchored to a real YAML list item.** A bare substring search over the
    # whole file is satisfied by a commented-out trigger or an incidental
    # mention, so the check would pass while no tag actually starts the
    # workflow. The dispatch check below was already anchored; this one was not.
    $hasTag = $workflow -match ("(?m)^\s+- '" + [regex]::Escape("$crate-v*") + "'\s*$")
    $hasDispatch = $workflow -match ("(?m)^\s+- " + [regex]::Escape($crate) + "\s*$")
    if (-not $hasTag) { $missing += "$crate : no tag trigger, so its release tag would publish nothing" }
    if (-not $hasDispatch) { $missing += "$crate : not a workflow_dispatch choice, so it cannot be published by hand either" }
    if ($registryLine -and $registry -notcontains $crate) {
        $missing += "$crate : absent from workspace_crates, so a dependent can race its tag instead of waiting for it"
    }
}

if ($missing.Count -gt 0) {
    Write-Host "Release-managed crates that cannot be published:" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    Write-Host ''
    Write-Host "Add them to .github/workflows/publish-crate.yml: the tag list, the dispatch choices,"
    Write-Host "and the workspace_crates registry the sibling-dependency wait reads."
    exit 1
}

Write-Host "All $($managed.Count) release-managed crates have a publish trigger." -ForegroundColor Green
exit 0
