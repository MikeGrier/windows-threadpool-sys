# Copyright (c) 2026 Mike Grier
<#
.SYNOPSIS
    Flags commits that would give a released crate a version bump it did not earn.

.DESCRIPTION
    release-please attributes a commit to a package by the PATHS it touches, not
    by the (scope) in its subject line. So a release-triggering commit -- feat,
    fix, or anything marked `!` -- that incidentally edits a second released
    crate's files gives that crate a changelog entry and a version bump for work
    that did not change it.

    This has already shipped in this repository twice: windows-ioring-sys'
    CHANGELOG carries two **guard-alloc:** entries, because those commits touched
    ioring's tests; and two `feat(topology)!` commits would have taken ioring to a
    breaking 0.3.0 over an example and one doc-comment line.

    This script FLAGS, it does not decide. It cannot: a doc-comment-only edit to
    `src/lib.rs` is indistinguishable from a real one by path alone, and the
    measured history shows 7 of 9 cross-crate commits were legitimately coupled.
    The judgement is yours; the point is that you make it deliberately rather than
    discovering it in a release PR.

.PARAMETER Range
    A git revision range to audit, e.g. 'origin/main..HEAD'. Default when neither
    -Range nor -Staged is given.

.PARAMETER Staged
    Check what is currently staged instead of history. Requires -Type.

.PARAMETER Type
    The Conventional Commits type you are about to use, e.g. 'feat', 'fix!',
    'chore'. Only meaningful with -Staged.

.EXAMPLE
    .\tools\check-commit-scope.ps1
    .\tools\check-commit-scope.ps1 -Range 'origin/main..HEAD'
    .\tools\check-commit-scope.ps1 -Staged -Type 'feat!'
#>
[CmdletBinding()]
param(
    [string] $Range,
    [switch] $Staged,
    [string] $Type
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

# The crates release-please actually versions. A `publish = false` crate cannot
# be poisoned, because it is never released -- so it is not a finding.
$manifestPath = Join-Path $repo '.release-please-manifest.json'
if (-not (Test-Path $manifestPath)) { throw "No .release-please-manifest.json at $manifestPath" }
$released = (Get-Content $manifestPath -Raw | ConvertFrom-Json).PSObject.Properties.Name |
    ForEach-Object { Split-Path $_ -Leaf }

function Get-ReleasedCrates([string[]] $paths) {
    $paths |
        Where-Object { $_ -match '^crates/[^/]+/' } |
        ForEach-Object { ($_ -split '/')[1] } |
        Sort-Object -Unique |
        Where-Object { $_ -in $released }
}

# A scope maps to a crate by convention: `topology` -> `windows-topology-sys`,
# `wtf-string` -> `wtf-string`. Derived from the crate names rather than a table,
# so a new crate needs no edit here.
function Resolve-Scope([string] $scope, [string[]] $candidates) {
    if (-not $scope) { return $null }
    foreach ($form in @($scope, "windows-$scope", "windows-$scope-sys", "$scope-sys")) {
        if ($form -in $candidates) { return $form }
    }
    return $null
}

function Get-Scope([string] $subject) {
    if ($subject -match '^[a-z]+\(([^)]+)\)!?:') { return $matches[1] }
    return $null
}

function Test-Triggering([string] $subject) {
    # Only feat, fix and breaking changes trigger a release. docs/test/refactor/
    # chore are changelog-only, so they cannot poison anything.
    $subject -match '^(feat|fix)(\([^)]+\))?!?:' -or $subject -match '^[a-z]+(\([^)]+\))?!:'
}

function Test-Breaking([string] $subject) { $subject -match '^[a-z]+(\([^)]+\))?!:' }

$findings = @()

if ($Staged) {
    if (-not $Type) { throw '-Staged requires -Type (the Conventional Commits type you intend to use).' }
    $subject = "${Type}: staged"
    if (-not (Test-Triggering $subject)) {
        Write-Host "'$Type' does not trigger a release, so it cannot poison a sibling crate. Nothing to check." -ForegroundColor Green
        exit 0
    }
    $paths = @(git --no-pager diff --cached --name-only)
    $crates = @(Get-ReleasedCrates $paths)
    if ($crates.Count -gt 1) {
        $findings += [pscustomobject]@{ Sha = '(staged)'; Subject = "$Type ..."; Crates = $crates; Breaking = (Test-Breaking $subject); Paths = $paths }
    }
} else {
    if (-not $Range) { $Range = 'origin/main..HEAD' }
    foreach ($line in (git --no-pager log $Range --format='%h|%s')) {
        $sha, $subject = $line -split '\|', 2
        if (-not (Test-Triggering $subject)) { continue }
        $paths = @(git --no-pager show $sha --name-only --format='')
        $crates = @(Get-ReleasedCrates $paths)
        if ($crates.Count -eq 0) { continue }
        # Two distinct symptoms of one disease:
        #   - the commit spans several released crates, so the siblings get bumps; or
        #   - its scope names a crate that is NOT the one it will be attributed to,
        #     which is how `feat(guard-alloc)` put two entries in ioring's changelog.
        $scope = Get-Scope $subject
        $allCrates = @($paths | Where-Object { $_ -match '^crates/[^/]+/' } | ForEach-Object { ($_ -split '/')[1] } | Sort-Object -Unique)
        $scopeCrate = Resolve-Scope $scope $allCrates
        $misScoped = ($null -ne $scopeCrate) -and ($scopeCrate -notin $crates)
        if ($crates.Count -gt 1 -or $misScoped) {
            $findings += [pscustomobject]@{ Sha = $sha; Subject = $subject; Crates = $crates; Breaking = (Test-Breaking $subject); Paths = $paths; ScopeCrate = $scopeCrate; MisScoped = $misScoped }
        }
    }
}

if (-not $findings) {
    Write-Host 'No release-triggering commit spans more than one released crate.' -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host "$($findings.Count) release-triggering change(s) span more than one released crate." -ForegroundColor Yellow
Write-Host 'Each of these crates will get a changelog entry and a version bump from it.'
Write-Host ''

foreach ($f in $findings) {
    $mark = if ($f.Breaking) { 'BREAKING' } else { 'release ' }
    Write-Host ("[{0}] {1}  {2}" -f $mark, $f.Sha, $f.Subject)
    if ($f.MisScoped) {
        Write-Host ("    scope names '{0}', which is NOT released by this commit -- the entry will be filed under the crate(s) below, mislabelled." -f $f.ScopeCrate) -ForegroundColor Yellow
    }
    foreach ($crate in $f.Crates) {
        $srcFiles = @($f.Paths | Where-Object { $_ -like "crates/$crate/src/*" })
        $other = @($f.Paths | Where-Object { $_ -like "crates/$crate/*" -and $_ -notlike "crates/$crate/src/*" })
        $note = if ($srcFiles.Count -eq 0) {
            'NO src change -- almost certainly a ride-along'
        } elseif ($srcFiles.Count -eq 1) {
            'ONE src file -- check whether it is a real change or a doc comment'
        } else {
            "$($srcFiles.Count) src files"
        }
        Write-Host ("    {0,-42} {1}" -f $crate, $note)
        if ($srcFiles.Count -le 1 -and $srcFiles.Count -gt 0) {
            Write-Host ("        {0}" -f $srcFiles[0]) -ForegroundColor DarkGray
        }
        if ($srcFiles.Count -eq 0 -and $other.Count -gt 0) {
            Write-Host ("        {0}{1}" -f $other[0], $(if ($other.Count -gt 1) { " (+$($other.Count - 1) more)" } else { '' })) -ForegroundColor DarkGray
        }
    }
    Write-Host ''
}

Write-Host 'This flags; it does not decide. Two shapes, and they need opposite answers:' -ForegroundColor Cyan
Write-Host '  GENUINELY COUPLED -- both crates behaviour changed. Leave it. The bump is earned,'
Write-Host '  and splitting would produce a commit that does not compile.'
Write-Host ''
Write-Host '  RIDE-ALONG -- the second crate only followed a rename, or its example/test/docs'
Write-Host '  moved. Put that part in its own `chore(<crate>):` commit; chore triggers no'
Write-Host '  release, so the sibling gets nothing. For a rename, the three-commit form keeps'
Write-Host '  every commit compiling: add the new name as an alias (feat, additive), move the'
Write-Host '  consumer (chore), then delete the alias (feat!, owning crate only).'
Write-Host ''
Write-Host '  Already committed and not worth rewriting? Correct it at release time with a'
Write-Host '  `Release-As: x.y.z` footer on a commit touching only that crate.'
exit 1