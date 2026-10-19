# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Check that every sabotage entry's patch still applies, building nothing.

.DESCRIPTION
    Runs [run-sabotage.ps1](run-sabotage.ps1) `-CheckPatches` over every manifest
    [common.ps1](common.ps1) `Get-SabotageManifest` finds -- the same list the
    sweep's planner builds its matrix from, and the harness's own in tools/ -- and reports each entry whose patch
    no longer matches its file exactly once, or no longer changes it.

    A patch the code has moved away from is found by reading two files, so it
    should not wait for a sweep that builds a crate per entry. This is what CI
    runs on every push and pull request.

.PARAMETER RepositoryRoot
    The repository whose manifests are checked. Defaults to this script's
    parent's parent.

.OUTPUTS
    Exits 0 when every patch in every manifest applies, 1 when one does not, and
    2 when a manifest could not be checked at all.
#>
[CmdletBinding()]
param(
    [string] $RepositoryRoot
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }
$harness = Join-Path $PSScriptRoot 'run-sabotage.ps1'

$manifests = @(Get-SabotageManifest -RepositoryRoot $RepositoryRoot -IncludeTools)
if ($manifests.Count -eq 0) {
    Write-Host "No sabotage manifests under '$RepositoryRoot'." -ForegroundColor Red
    exit 2
}

# Each manifest in its own process, as a sweep runs it: the harness exits with
# its verdict, and in this process that exit would end the loop at the first one.
$shell = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell' }
$worst = 0
foreach ($manifest in $manifests) {
    Invoke-Native { & $shell -NoProfile -File $harness -Manifest $manifest -CheckPatches } |
        ForEach-Object { Write-Host $_ }
    $code = $LASTEXITCODE
    if ($code -gt $worst) { $worst = $code }
}

if ($worst -eq 0) {
    Write-Host "Every patch in $($manifests.Count) manifests applies." -ForegroundColor Green
}
exit $worst
