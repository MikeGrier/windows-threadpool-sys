# Copyright (c) Mike Grier.

<#
.SYNOPSIS
    Build the sabotage-sweep job matrix from the manifests themselves.

.DESCRIPTION
    Emits the `strategy.matrix` object that `.github/workflows/sabotage-sweep.yml`
    consumes, as `{"include":[{"crate":...,"manifest":...,"label":...,"shard":N,"shards":M},...]}`.

    WHY THIS EXISTS RATHER THAN A LIST IN THE WORKFLOW. A hand-written matrix
    stores two facts the repository already owns: which crates have a manifest,
    and how many entries each one has. Both drift silently, and both already
    did -- within a single branch, a manifest grew and left four stale numbers
    behind it in the workflow, one of them a worked shard table that no longer
    described any real split. Deriving the matrix makes that class of drift
    unrepresentable rather than merely discouraged: a new manifest is swept
    without anyone remembering, and a growing one gets more shards on its own.

    WHY THE DISPATCH INPUT IS VALIDATED HERE. The workflow used to select jobs
    for a named manifest with an `endsWith` test on each matrix entry. A typo
    matched NO entry, so every sweep step was skipped, every job succeeded, and
    the run reported green having swept nothing -- the worst outcome available
    to a gate, since it is indistinguishable from a clean sweep. Resolving the
    name here turns that into a single loud failure before any sweep starts.

.PARAMETER Manifest
    Optional manifest path to restrict the matrix to, as passed by
    `workflow_dispatch`. Blank means every manifest. A non-blank value that
    names no known manifest is an ERROR, never an empty matrix.

.PARAMETER EntriesPerShard
    Target entries per shard. Each manifest gets `ceil(entries / this)` shards,
    at least one. This is the only tuning knob: raise it for fewer, longer jobs
    and lower it for more, shorter ones.

.PARAMETER RepositoryRoot
    Repository root to search. Defaults to this script's parent directory.

.PARAMETER GitHubOutput
    Optional path to the GitHub Actions step-output file. When given, the
    matrix is appended as `matrix=<json>`, and the number of jobs in it as
    `jobs=<n>`; the matrix is always written to stdout too, so a local run shows
    exactly what CI will consume.

.PARAMETER Since
    Optional base revision. When given, each manifest is planned for the
    entries a change since then could have affected, and a manifest with none is
    left out: every entry when the change reached a crate the manifest's crate
    builds on, directly or not, through any kind of path dependency, or an input
    every sweep shares, or when its base cannot be measured from -- blank, all
    zeros, or absent from this clone; within the crate, the entries the record
    of which tests caught each entry cannot rule out. The rule is
    `Select-SabotageEntry` in [common.ps1](common.ps1), the one the harness
    applies with the same -Since, so the shard counts here are for the entries
    each shard will find. It reads the dependencies from `cargo metadata`, so
    cargo must be on PATH. The matrix may then be empty, `{"include":[]}`.

.PARAMETER CatcherRecord
    The record of which tests caught each entry, for -Since. Missing or empty,
    every entry of an affected crate is planned.
#>
[CmdletBinding()]
param(
    [string] $Manifest = '',
    [int] $EntriesPerShard = 10,
    [string] $RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string] $GitHubOutput = '',
    [string] $Since = '',
    [string] $CatcherRecord = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

if ($EntriesPerShard -lt 1) {
    throw "-EntriesPerShard must be at least 1; got $EntriesPerShard."
}

$cratesDir = Join-Path $RepositoryRoot 'crates'
if (-not (Test-Path -LiteralPath $cratesDir)) {
    throw "No crates directory under '$RepositoryRoot'."
}

# Sorted so the matrix is stable run to run: a job name that moves between runs
# makes two runs impossible to compare, and GitHub orders jobs as given. The list
# is common.ps1's, which the patch check walks too.
$manifests = @(Get-SabotageManifest -RepositoryRoot $RepositoryRoot)

if ($manifests.Count -eq 0) {
    throw "No crates/*/sabotage*.json found under '$RepositoryRoot'."
}

# Compare by repository-relative path with forward slashes, so a dispatch input
# written the way it appears in the workflow ('crates/x/sabotage.json') matches
# on Windows, where the discovered path is absolute and backslashed.
function Get-RelativePath {
    param([string] $FullPath)
    $full = [System.IO.Path]::GetFullPath($FullPath)
    $root = [System.IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
    $relative = $full.Substring($root.Length).TrimStart('\', '/')
    return $relative.Replace('\', '/')
}

$known = @{}
foreach ($path in $manifests) {
    $known[(Get-RelativePath $path)] = $path
}

$chosen = $manifests
if (-not [string]::IsNullOrWhiteSpace($Manifest)) {
    $wanted = $Manifest.Trim().Replace('\', '/').TrimStart('./')
    if (-not $known.ContainsKey($wanted)) {
        $names = ($known.Keys | Sort-Object) -join "`n  "
        throw @"
Manifest '$Manifest' matches no sabotage manifest in this repository.

A dispatch naming a manifest that does not exist would otherwise select no
jobs at all and report success having swept nothing, so it fails here instead.

Known manifests:
  $names
"@
    }
    $chosen = @($known[$wanted])
}

# Why the plan is what it is, on the host rather than stdout, which carries the
# matrix alone. The one place this script says anything else.
function Write-PlanNote {
    param([string] $Message)
    Write-Host "plan: $Message"
}

# Each manifest's entry count under -Since, or $null when every entry counts.
$scopedCounts = $null
if (-not [string]::IsNullOrWhiteSpace($Since)) {
    $change = Get-SabotageChange -RepositoryRoot $RepositoryRoot -Since $Since
    $dependencies = $null
    $packages = $null
    if ($change.Known) {
        $dependencies = Get-SabotageCrateDependency -RepositoryRoot $RepositoryRoot
        $packages = Get-SabotageCratePackage -RepositoryRoot $RepositoryRoot
    }
    $record = Read-SabotageCatcherRecord -Path $CatcherRecord
    $scopedCounts = @{}
    $planned = 0
    foreach ($path in $chosen) {
        $spec = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
        $scope = Select-SabotageEntry -RepositoryRoot $RepositoryRoot -Manifest (Get-RelativePath $path) `
            -Entries $spec.sabotages -SourceRoot (Get-SabotageSourceRoot -ManifestPath $path -Spec $spec) `
            -Change $change -Dependencies $dependencies -Record $record -Spec $spec -Packages $packages
        $scopedCounts[$path] = @($scope.Entries).Count
        if ($scopedCounts[$path] -gt 0) {
            $planned++
            Write-PlanNote "$(Get-RelativePath $path): $($scope.Reason)."
        }
    }
    Write-PlanNote "$planned of $($chosen.Count) manifests since $Since."
}
$include = @()
foreach ($path in $chosen) {
    $spec = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    $count = @($spec.sabotages).Count
    if ($count -eq 0) {
        throw "Manifest '$(Get-RelativePath $path)' declares no sabotages."
    }
    if ($null -ne $scopedCounts) {
        $count = $scopedCounts[$path]
        if ($count -eq 0) { continue }
    }

    # One shard per `EntriesPerShard` entries, rounded up, so a manifest that
    # grows past a multiple gains a shard without anyone editing a list. The
    # harness's split is even to within one entry at ANY shard count, so this
    # only has to be roughly right.
    $shards = [math]::Max(1, [math]::Ceiling($count / [double]$EntriesPerShard))

    $crate = Split-Path -Leaf (Split-Path -Parent $path)
    # A crate may carry several manifests, each with its own `testArgs`: `sabotage.json` and
    # `sabotage-<kind>.json`. The label names a job, a cache and an artifact, so it must differ
    # between them; for the plain one it is the crate, as it always was.
    $kind = [System.IO.Path]::GetFileNameWithoutExtension($path).Substring('sabotage'.Length).TrimStart('-')
    $label = if ($kind) { "$crate-$kind" } else { $crate }
    for ($shard = 0; $shard -lt $shards; $shard++) {
        $include += [ordered]@{
            crate    = $crate
            manifest = Get-RelativePath $path
            label    = $label
            shard    = $shard
            shards   = [int]$shards
        }
    }
}

# -Compress because a step output is a single line; -Depth because ConvertTo-Json
# truncates nested structures at 2 by default and would emit the hashtables as
# type names.
$json = [ordered]@{ include = $include } | ConvertTo-Json -Depth 5 -Compress

Write-Output $json

if (-not [string]::IsNullOrWhiteSpace($GitHubOutput)) {
    Add-Content -LiteralPath $GitHubOutput -Value "matrix=$json"
    Add-Content -LiteralPath $GitHubOutput -Value "jobs=$(@($include).Count)"
}
