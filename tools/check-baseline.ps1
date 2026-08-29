# Copyright (c) Mike Grier
#
# tools/check-baseline.ps1 -- guards the Rust language baseline against
# restatement drift. The authoritative values live in exactly two places:
#
#   Cargo.toml          [workspace.package] edition, rust-version
#   rust-toolchain.toml [toolchain] channel
#
# Ten-odd other places restate them in prose, in a CI job name, and in a
# toolchain action pin. Nothing detected the day one of those disagreed with the
# manifests, which is precisely the "restatement drift" failure the repository's
# own instructions warn about: a value corrected in one place and left stale in
# the other nine, with no signal that they had diverged.
#
# The restatements cannot simply be deleted. A reviewer -- human or automated --
# reading a pull request diff cannot follow a link out of the diff to resolve
# `edition.workspace = true` against a table that is not in any hunk, so the
# documents have to state the values outright. Since the copies must exist, this
# check makes them derived-in-effect: they are still written by hand, but they
# can no longer silently disagree with the manifests they claim to describe.
#
# Two independent checks run over every registered file:
#
#   1. Structured claims. Each labelled restatement is matched by its own
#      regex and compared against the authoritative value. A claim that no
#      longer matches its regex is a FAILURE, not a pass -- that is what
#      catches a reword that drops or reshapes the value.
#
#   2. Stray-token sweep. Every Rust-version-shaped token in a registered file
#      must be the MSRV, the pinned channel, or an explicitly allow-listed
#      historical version. This is what catches drift in ordinary prose, which
#      no per-claim regex is ever written for.
#
# Usage:
#   pwsh tools/check-baseline.ps1
#
# Exits 0 when every restatement agrees with the manifests, 1 on any
# disagreement, and 2 for usage/configuration errors (missing file, or a
# manifest whose authoritative value cannot be parsed).

[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot

function Read-RepoFile([string]$relative) {
    $full = Join-Path $repoRoot $relative
    if (-not (Test-Path -LiteralPath $full)) {
        Write-Host "CONFIG ERROR: registered file not found: $relative" -ForegroundColor Red
        exit 2
    }
    return [System.IO.File]::ReadAllText($full)
}

function Get-Capture([string]$text, [string]$pattern, [string]$what) {
    $m = [regex]::Match($text, $pattern)
    if (-not $m.Success) {
        Write-Host "CONFIG ERROR: could not parse $what" -ForegroundColor Red
        exit 2
    }
    return $m.Groups[1].Value
}

# --- Authoritative values -------------------------------------------------

# Anchored to the [workspace.package] table so a crate's own inherited
# `edition.workspace = true` can never be picked up by mistake.
$rootManifest = Read-RepoFile 'Cargo.toml'
$workspacePackage = Get-Capture $rootManifest '(?ms)^\[workspace\.package\]\r?\n(.*?)(?=^\[|\z)' `
    'the [workspace.package] table of Cargo.toml'

$authoritative = [ordered]@{
    edition = Get-Capture $workspacePackage '(?m)^\s*edition\s*=\s*"([^"]+)"' `
        '[workspace.package] edition in Cargo.toml'
    msrv    = Get-Capture $workspacePackage '(?m)^\s*rust-version\s*=\s*"([^"]+)"' `
        '[workspace.package] rust-version in Cargo.toml'
    channel = Get-Capture (Read-RepoFile 'rust-toolchain.toml') '(?m)^\s*channel\s*=\s*"([^"]+)"' `
        '[toolchain] channel in rust-toolchain.toml'
}

# Rust versions that legitimately appear in the registered files without being
# the baseline. Each needs a reason, because an unexplained entry here is how a
# genuinely stale value would get waved through.
$AllowedVersions = @{
    '1.80' = 'the release that put size_of/align_of in the prelude'
}

$failures = @()

# --- Check 1: the manifests must agree with each other --------------------
#
# The channel is the MSRV at a specific patch level, so local development runs
# on exactly the floor that `rust-version` promises. If they drift apart, every
# document below is describing two different baselines at once and there is no
# single correct value to check them against.
if ($authoritative.channel -ne $authoritative.msrv -and
    -not $authoritative.channel.StartsWith("$($authoritative.msrv).")) {
    $failures += "rust-toolchain.toml channel '$($authoritative.channel)' is not the " +
                 "Cargo.toml rust-version '$($authoritative.msrv)' at a patch level"
}

# --- Check 2: structured claims -------------------------------------------
#
# `Expect` names which authoritative value the captured text must equal.
$claims = @(
    [pscustomobject]@{
        File = '.github/copilot-instructions.md'
        What = 'the "Rust edition N" headline claim'
        Pattern = '\*\*Rust edition (\d{4})\*\*'
        Expect = 'edition'
    }
    [pscustomobject]@{
        File = '.github/copilot-instructions.md'
        What = 'the "MSRV of N" headline claim'
        Pattern = '\*\*MSRV of ([\d.]+)\*\*'
        Expect = 'msrv'
    }
    [pscustomobject]@{
        File = '.github/copilot-instructions.md'
        What = 'the "toolchain N" headline claim'
        Pattern = '\*\*toolchain ([\d.]+)\*\*'
        Expect = 'channel'
    }
    [pscustomobject]@{
        File = '.github/instructions/global.rust.instructions.md'
        What = 'the Edition row of the baseline table'
        Pattern = '\|\s*Edition\s*\|\s*\*\*(\d{4})\*\*\s*\|'
        Expect = 'edition'
    }
    [pscustomobject]@{
        File = '.github/instructions/global.rust.instructions.md'
        What = 'the MSRV row of the baseline table'
        Pattern = '\|\s*MSRV\s*\|\s*\*\*([\d.]+)\*\*\s*\|'
        Expect = 'msrv'
    }
    [pscustomobject]@{
        File = '.github/instructions/global.rust.instructions.md'
        What = 'the Pinned toolchain row of the baseline table'
        Pattern = '\|\s*Pinned toolchain\s*\|\s*\*\*([\d.]+)\*\*\s*\|'
        Expect = 'channel'
    }
    [pscustomobject]@{
        File = '.github/workflows/ci.yml'
        What = 'the msrv job name'
        Pattern = '(?m)^\s*name:\s*MSRV check ([\d.]+)\s*$'
        Expect = 'msrv'
    }
    [pscustomobject]@{
        File = '.github/workflows/ci.yml'
        What = 'the msrv job toolchain pin'
        Pattern = 'dtolnay/rust-toolchain@(\d[\d.]*)'
        Expect = 'channel'
    }
    [pscustomobject]@{
        File = 'README.md'
        What = 'the stated minimum Rust version'
        Pattern = 'Requires Rust `([\d.]+)` or newer'
        Expect = 'msrv'
    }
    [pscustomobject]@{
        File = 'DEVELOPMENT.md'
        What = 'the toolchain pin described under "Toolchain"'
        Pattern = 'pins local development to the MSRV\s*\r?\n?\s*\((\d[\d.]*)'
        Expect = 'channel'
    }
    [pscustomobject]@{
        File = 'DEVELOPMENT.md'
        What = 'the msrv job pin described under "Toolchain"'
        Pattern = 'pins `@(\d[\d.]*)` to guard the floor'
        Expect = 'channel'
    }
    [pscustomobject]@{
        File = '.github/dependabot.yml'
        What = 'the pinned toolchain tag named in the ignore rationale'
        Pattern = '@stable, and @(\d[\d.]*) for the MSRV'
        Expect = 'channel'
    }
)

# Reading each registered file once keeps the sweep below honest about which
# files are actually covered.
$registered = [ordered]@{}
foreach ($file in ($claims.File | Select-Object -Unique)) {
    $registered[$file] = Read-RepoFile $file
}

foreach ($claim in $claims) {
    $expected = $authoritative[$claim.Expect]
    $m = [regex]::Match($registered[$claim.File], $claim.Pattern)
    if (-not $m.Success) {
        $failures += "$($claim.File): $($claim.What) is missing or reworded " +
                     "(expected to find $($claim.Expect) '$expected')"
        continue
    }
    $actual = $m.Groups[1].Value
    if ($actual -ne $expected) {
        $failures += "$($claim.File): $($claim.What) says '$actual' but " +
                     "$($claim.Expect) is '$expected'"
    }
}

# --- Check 3: stray-token sweep -------------------------------------------
#
# The lookbehind excludes a preceding '-' so the checklist sub-step notation
# (`RC-1.1`, `RC-1.2`) is not mistaken for a Rust version. A real version is
# preceded by whitespace, a backtick, or '@'.
$versionToken = '(?<![\w.\-])1\.\d+(?:\.\d+)?(?![\w.])'

foreach ($file in $registered.Keys) {
    $text = $registered[$file]
    $seen = @{}
    foreach ($m in [regex]::Matches($text, $versionToken)) {
        $token = $m.Value
        if ($token -eq $authoritative.msrv -or $token -eq $authoritative.channel) { continue }
        if ($AllowedVersions.ContainsKey($token)) { continue }
        if ($seen.ContainsKey($token)) { continue }
        $seen[$token] = $true

        $line = ($text.Substring(0, $m.Index) -split "`n").Count
        $failures += "${file}: line ${line} states Rust '$token', which is neither the " +
                     "MSRV '$($authoritative.msrv)' nor the pinned channel " +
                     "'$($authoritative.channel)' nor an allow-listed version"
    }
}

# --- Report ---------------------------------------------------------------

if ($failures.Count -gt 0) {
    foreach ($f in $failures) { Write-Host "BASELINE DRIFT: $f" -ForegroundColor Red }
    Write-Host ''
    Write-Host "Baseline check failed: $($failures.Count) disagreement(s)." -ForegroundColor Red
    Write-Host 'Authoritative values (Cargo.toml, rust-toolchain.toml):' -ForegroundColor Yellow
    foreach ($k in $authoritative.Keys) {
        Write-Host ("  {0,-8} {1}" -f $k, $authoritative[$k]) -ForegroundColor Yellow
    }
    Write-Host 'Update the restatements to match, or correct the manifests.' -ForegroundColor Yellow
    exit 1
}

Write-Host ("Baseline check passed: edition {0}, MSRV {1}, channel {2} -- {3} claim(s) across {4} file(s) agree." -f `
        $authoritative.edition, $authoritative.msrv, $authoritative.channel, $claims.Count, $registered.Count) `
    -ForegroundColor Green
exit 0
