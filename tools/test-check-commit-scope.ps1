# Copyright (c) 2026 Mike Grier. All rights reserved.
<#
.SYNOPSIS
    Tests for check-commit-scope.ps1. Exits 0 only if every case passes.

.DESCRIPTION
    WHY THIS EXISTS. The script decides which crates release-please versions,
    and it read that from the wrong file: `.release-please-manifest.json`
    records the current version of each managed package, but nothing prunes an
    entry when a package leaves `release-please-config.json`. This repository's
    manifest still carries `windows-platform-probes`, which the config does not
    manage and whose Cargo.toml says `publish = false`.

    So the script treated an unreleased crate as released and flagged a commit
    for mislabelling a changelog entry that could never be written. The flag was
    acted on. Nothing caught it, because a guard with no test is enforced by
    whoever remembers it.

    Both directions are checked here, because a guard that has stopped firing
    passes a one-directional test as happily as a correct one.

    WHY A THROWAWAY REPOSITORY. Asserting against this repository's own history
    would tie the tests to specific commits and to a package list that is
    expected to change. Each case builds a two-crate repository in a temporary
    directory, writes whatever config and manifest the case is about, and makes
    commits touching whichever crates it needs.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell
    5.1 ships Pester 3, whose syntax differs incompatibly from Pester 5.
#>
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'test-common.ps1')

$script:Script = Join-Path $PSScriptRoot 'check-commit-scope.ps1'

<#
.SYNOPSIS
    A git repository with two crates, and whatever release configuration the
    caller asks for.
#>
function New-Fixture {
    param(
        [string[]] $ManagedCrates,
        [string[]] $ManifestCrates
    )
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("ccs-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $root | Out-Null

    $packages = @{}
    foreach ($crate in $ManagedCrates) {
        $packages["crates/$crate"] = @{ 'package-name' = $crate; component = $crate }
    }
    @{ packages = $packages } | ConvertTo-Json -Depth 5 |
        Set-Content -Path (Join-Path $root 'release-please-config.json') -Encoding utf8

    $manifest = @{}
    foreach ($crate in $ManifestCrates) { $manifest["crates/$crate"] = '0.1.0' }
    $manifest | ConvertTo-Json -Depth 5 |
        Set-Content -Path (Join-Path $root '.release-please-manifest.json') -Encoding utf8

    Push-Location $root
    try {
        git init --quiet 2>&1 | Out-Null
        git config user.email 'test@example.invalid' 2>&1 | Out-Null
        git config user.name 'Test' 2>&1 | Out-Null
        git add -A 2>&1 | Out-Null
        git commit --quiet -m 'chore: fixture' 2>&1 | Out-Null
    }
    finally { Pop-Location }
    return $root
}

<#
.SYNOPSIS
    Commit an edit to each named crate's `src/lib.rs`, with the given subject.
#>
function Add-CrateCommit {
    param([string] $Root, [string] $Subject, [string[]] $Crates)
    Push-Location $Root
    try {
        foreach ($crate in $Crates) {
            $dir = Join-Path $Root "crates/$crate/src"
            New-Item -ItemType Directory -Path $dir -Force | Out-Null
            Add-Content -Path (Join-Path $dir 'lib.rs') -Value "// $([guid]::NewGuid())"
        }
        git add -A 2>&1 | Out-Null
        git commit --quiet -m $Subject 2>&1 | Out-Null
        return (git rev-parse HEAD).Trim()
    }
    finally { Pop-Location }
}

function Invoke-Check {
    param([string] $Root, [string] $Range)
    # `*>&1`, not `2>&1`: the script reports through `Write-Host`, which writes
    # to the host rather than the output stream, so a plain merge captures
    # nothing and every assertion here would pass against silence.
    $output = & $script:Script -RepoRoot $Root -Range $Range *>&1 | Out-String
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
}

Write-Report "check-commit-scope.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# **The defect.** A crate present in the manifest but absent from the config is
# not released, so a commit touching it alongside a released crate spans one
# released crate, not two.
Test-Case 'a crate in the manifest but not the config is not treated as released' {
    $root = New-Fixture -ManagedCrates @('alpha') -ManifestCrates @('alpha', 'orphan')
    try {
        Add-CrateCommit -Root $root -Subject 'fix(alpha): touch both' -Crates @('alpha', 'orphan') | Out-Null
        $result = Invoke-Check -Root $root -Range 'HEAD~1..HEAD'
        Assert-Equal 0 $result.ExitCode 'exit code'
        if ($result.Output -notmatch 'No release-triggering commit spans more than one') {
            throw "expected a clean result, got: $($result.Output)"
        }
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# The other direction. A guard that stopped firing would pass the case above.
Test-Case 'a release commit spanning two managed crates is still flagged' {
    $root = New-Fixture -ManagedCrates @('alpha', 'beta') -ManifestCrates @('alpha', 'beta')
    try {
        Add-CrateCommit -Root $root -Subject 'fix(alpha): touch both' -Crates @('alpha', 'beta') | Out-Null
        $result = Invoke-Check -Root $root -Range 'HEAD~1..HEAD'
        Assert-Equal 1 $result.ExitCode 'exit code'
        if ($result.Output -notmatch 'span more than one released crate') {
            throw "expected the span warning, got: $($result.Output)"
        }
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a release commit touching one managed crate is not flagged' {
    $root = New-Fixture -ManagedCrates @('alpha', 'beta') -ManifestCrates @('alpha', 'beta')
    try {
        Add-CrateCommit -Root $root -Subject 'fix(alpha): touch one' -Crates @('alpha') | Out-Null
        $result = Invoke-Check -Root $root -Range 'HEAD~1..HEAD'
        Assert-Equal 0 $result.ExitCode 'exit code'
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# `chore` triggers no release, so it cannot poison a sibling whatever it touches.
Test-Case 'a chore spanning two managed crates is not flagged' {
    $root = New-Fixture -ManagedCrates @('alpha', 'beta') -ManifestCrates @('alpha', 'beta')
    try {
        Add-CrateCommit -Root $root -Subject 'chore(alpha): touch both' -Crates @('alpha', 'beta') | Out-Null
        $result = Invoke-Check -Root $root -Range 'HEAD~1..HEAD'
        Assert-Equal 0 $result.ExitCode 'exit code'
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# The drift that produced the defect is reported, so the next reader does not
# have to rediscover which file is authoritative.
Test-Case 'manifest entries the config does not manage are reported' {
    $root = New-Fixture -ManagedCrates @('alpha') -ManifestCrates @('alpha', 'orphan')
    try {
        Add-CrateCommit -Root $root -Subject 'fix(alpha): touch one' -Crates @('alpha') | Out-Null
        $result = Invoke-Check -Root $root -Range 'HEAD~1..HEAD'
        if ($result.Output -notmatch 'orphan') {
            throw "expected the orphan note, got: $($result.Output)"
        }
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# A config with no packages is a misread file, not an empty release set: every
# commit would silently pass. Better to stop.
Test-Case 'a config naming no packages is refused rather than passing everything' {
    $root = New-Fixture -ManagedCrates @() -ManifestCrates @('alpha')
    try {
        Add-CrateCommit -Root $root -Subject 'fix(alpha): touch one' -Crates @('alpha') | Out-Null
        # A terminating error, so it is caught here rather than read off the
        # output: refusing loudly is the behaviour, and a test that let the
        # throw escape would report the right outcome as a failure.
        $refused = $false
        try { Invoke-Check -Root $root -Range 'HEAD~1..HEAD' | Out-Null }
        catch { $refused = "$($_.Exception.Message)" -match 'names no packages' }
        if (-not $refused) {
            throw 'expected the script to refuse a config with no packages'
        }
    }
    finally { Remove-Item $root -Recurse -Force -ErrorAction SilentlyContinue }
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)" -Level bad
    exit 1
}
Write-Report 'all cases passed' -Level good
exit 0
