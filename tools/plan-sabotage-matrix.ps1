# Copyright (c) Mike Grier.

<#
.SYNOPSIS
    Build the sabotage-sweep job matrix from the manifests themselves.

.DESCRIPTION
    Emits the `strategy.matrix` object that `.github/workflows/sabotage-sweep.yml`
    consumes, as `{"include":[{"crate":...,"shard":N,"shards":M},...]}`.

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
    matrix is appended as `matrix=<json>`; it is always written to stdout too,
    so a local run shows exactly what CI will consume.
#>
[CmdletBinding()]
param(
    [string] $Manifest = '',
    [int] $EntriesPerShard = 10,
    [string] $RepositoryRoot = (Split-Path -Parent $PSScriptRoot),
    [string] $GitHubOutput = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($EntriesPerShard -lt 1) {
    throw "-EntriesPerShard must be at least 1; got $EntriesPerShard."
}

$cratesDir = Join-Path $RepositoryRoot 'crates'
if (-not (Test-Path -LiteralPath $cratesDir)) {
    throw "No crates directory under '$RepositoryRoot'."
}

# Sorted so the matrix is stable run to run: a job name that moves between runs
# makes two runs impossible to compare, and GitHub orders jobs as given.
$manifests = @(
    Get-ChildItem -Path $cratesDir -Directory |
        ForEach-Object { Join-Path $_.FullName 'sabotage.json' } |
        Where-Object { Test-Path -LiteralPath $_ } |
        Sort-Object
)

if ($manifests.Count -eq 0) {
    throw "No crates/*/sabotage.json found under '$RepositoryRoot'."
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

$include = @()
foreach ($path in $chosen) {
    $spec = Get-Content -LiteralPath $path -Raw | ConvertFrom-Json
    $count = @($spec.sabotages).Count
    if ($count -eq 0) {
        throw "Manifest '$(Get-RelativePath $path)' declares no sabotages."
    }

    # One shard per `EntriesPerShard` entries, rounded up, so a manifest that
    # grows past a multiple gains a shard without anyone editing a list. The
    # harness's split is even to within one entry at ANY shard count, so this
    # only has to be roughly right.
    $shards = [math]::Max(1, [math]::Ceiling($count / [double]$EntriesPerShard))

    $crate = Split-Path -Leaf (Split-Path -Parent $path)
    for ($shard = 0; $shard -lt $shards; $shard++) {
        $include += [ordered]@{
            crate  = $crate
            shard  = $shard
            shards = [int]$shards
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
}
