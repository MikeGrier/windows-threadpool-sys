# Copyright (c) Mike Grier.
#
# tools/check-test-tiers.ps1 -- a test's tier marker and the module it sits in agree.
#
# WHAT A TIER IS. DEVELOPMENT.md ("Test tiers") and DESIGN-NOTES.md ("long-haul-tier") say: a long-haul test is
# `#[ignore = "long-haul: <what it costs>"]` in a module named `long_haul`; a probation test is
# `#[ignore = "probation: <what it costs>"]` in a module named `probation`. The tier's own run selects by NAME,
# because libtest filters by name and cannot filter by ignore reason: `--ignored long_haul::` runs every
# ignored test whose path contains `long_haul::`.
#
# WHY A CENSUS. The marker and the path are two statements of one fact, and nothing makes them agree. A test
# marked into a tier its path does not select would be ignored everywhere and run by no one, which looks exactly
# like a test that passes; a test in the tier's module but not marked would run on every push, or, if ignored
# for another reason, be picked up by the tier's run for a reason that is not its own. So both directions are
# checked, with the rules in Get-TierViolation.
#
# THE CENSUS ASKS THE TEST BINARIES. It builds the workspace's test executables and has each one list its own
# tests, with their paths and ignore reasons, through libtest's JSON discovery. A scan of the source for
# `#[ignore]` and `mod long_haul` would be a proxy for what rustc and libtest decide -- macro-generated tests,
# `#[path]` modules and `cfg` all change the answer -- and a census of the tier's membership that disagreed
# with the tier's own run would be worse than none. Unlisted, and so unchecked: doctests, which the tiers do
# not apply to.
#
# The listing needs libtest's unstable `--format json`, which a test executable accepts under RUSTC_BOOTSTRAP=1
# (the same arrangement as the compile_fail doctest check). That is set for the executables only.
#
#   ./tools/check-test-tiers.ps1                           # the workspace, all features (CI)
#   ./tools/check-test-tiers.ps1 -ListingPath FILE         # a recorded listing, for the tests
#   ./tools/check-test-tiers.ps1 -RepositoryRoot DIR -CargoArgs @('--workspace')   # a fixture workspace
#
# Exit: 0 consistent, 1 a test breaks a rule, 2 the census could not be taken.
#
# A listing file is one JSON object per line: {"binary":"<name>","name":"<test path>","ignore":<bool>,
# "ignore_message":"<reason>"}. tools/test-check-test-tiers.ps1 writes them.
[CmdletBinding()]
param(
    [string] $RepositoryRoot,
    [string] $ListingPath,
    [string[]] $CargoArgs = @('--workspace', '--all-features', '--locked')
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $RepositoryRoot) { $RepositoryRoot = Split-Path -Parent $PSScriptRoot }

# The tiers, spelled once. `Marker` is what an ignore reason starts with, `Module` the module a tier's tests sit
# in, and `Pattern` what a reason that is MEANT as the marker looks like when it is not spelled right. The
# tier's run filters on `<Module>::`, and a test's path is `<outer>::<module>::<test>`, so the filter selects
# exactly the tests with that module segment and no other.
$tiers = @(
    [pscustomobject]@{ Name = 'long-haul'; Marker = 'long-haul:'; Module = 'long_haul'; Pattern = '(?i)long[\s_-]*haul' }
    [pscustomobject]@{ Name = 'probation'; Marker = 'probation:'; Module = 'probation'; Pattern = '(?i)probation' }
)

# libtest writes an ignore reason into its JSON without escaping a backslash, so a reason that names a path
# (`\\localhost\C$`) is not valid JSON and a strict parser refuses the whole line. The fields read here are a
# test's path, whether it is ignored, and the reason's leading text, so a line the parser refuses is read by shape.
function ConvertFrom-LibtestLine {
    param([string] $Line)
    $parsed = $null
    try { $parsed = $Line | ConvertFrom-Json -ErrorAction Stop } catch { $parsed = $null }
    if ($null -ne $parsed) {
        if ($parsed.type -ne 'test' -or $parsed.event -ne 'discovered') { return $null }
        return [pscustomobject]@{ name = [string]$parsed.name; ignore = [bool]$parsed.ignore; ignore_message = [string]$parsed.ignore_message }
    }
    $shape = [regex]::Match($Line, '^\s*\{\s*"type":\s*"test",\s*"event":\s*"discovered",\s*"name":\s*"(?<name>[^"]*)",\s*"ignore":\s*(?<ignore>true|false)(?:,\s*"ignore_message":\s*"(?<message>.*?)")?,\s*"source_path":')
    if (-not $shape.Success) {
        # No test reaches this: no executable the suite can build writes a line that is neither JSON nor this
        # shape. It exists so a libtest that changes its format fails the census instead of being read as empty.
        throw "A test listing line is neither JSON nor the shape libtest writes: $Line"
    }
    [pscustomobject]@{ name = $shape.Groups['name'].Value; ignore = ($shape.Groups['ignore'].Value -eq 'true'); ignore_message = $shape.Groups['message'].Value }
}

function Get-TierViolation {
    param([object[]] $Tests)
    foreach ($test in $Tests) {
        $reason = if ($test.ignore) { [string]$test.ignore_message } else { '' }
        $segments = @($test.name -split '::')
        $modules = if ($segments.Count -gt 1) { @($segments[0..($segments.Count - 2)]) } else { @() }
        foreach ($tier in $tiers) {
            $marked = $reason.StartsWith($tier.Marker, [System.StringComparison]::Ordinal)
            $inModule = $modules -ccontains $tier.Module
            $filter = $tier.Module + '::'
            $selected = $test.name.Contains($filter)
            $problem = $null
            if ($marked -and -not $inModule) {
                $problem = "is marked $($tier.Name) but is not in a module named $($tier.Module), so `"$filter`" never selects it and nothing runs it"
            }
            elseif ($marked -and $reason.Substring($tier.Marker.Length).Trim().Length -eq 0) {
                $problem = "is marked $($tier.Name) but says nothing after `"$($tier.Marker)`": the reason is where what it costs is written"
            }
            elseif ($inModule -and -not $marked) {
                $problem = if ($test.ignore) {
                    "is in a module named $($tier.Module) but ignored for another reason, so the $($tier.Name) run would pick it up for a reason that is not its own"
                }
                else { "is in a module named $($tier.Module) but is not ignored, so it runs on every push and not in the $($tier.Name) tier" }
            }
            elseif ($selected -and -not $inModule) {
                $problem = "has `"$filter`" in its path without a module of that name, so the $($tier.Name) run would select it by accident"
            }
            elseif ($reason -match $tier.Pattern -and -not $marked) {
                $problem = "has an ignore reason that reads like a $($tier.Name) marker but does not start with `"$($tier.Marker)`""
            }
            if ($problem) {
                [pscustomobject]@{ Binary = $test.binary; Name = $test.name; Reason = $reason; Problem = $problem }
            }
        }
    }
}

# The tests every executable lists, as records. Exits 2 when the census cannot be taken, because a census that
# silently saw fewer tests than exist would pass for the wrong reason.
function Get-TestListing {
    # Every executable is asked for its list, and one that is not a libtest harness would instead run its own
    # main with arguments it does not expect. None is today; one added later is a decision to make here, not a
    # surprise to find in a CI run.
    $custom = @(Get-ChildItem -LiteralPath $RepositoryRoot -Recurse -Filter Cargo.toml -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -notmatch '[\\/](target|\.scratch|node_modules)[\\/]' } |
            Where-Object { (Get-Content -LiteralPath $_.FullName -Raw) -match '(?m)^\s*harness\s*=\s*false' } |
            ForEach-Object { $_.FullName })
    if ($custom.Count -gt 0) {
        Write-Host "A target with harness = false is declared in: $($custom -join ', ')"
        Write-Host 'This check asks each test executable for its list, which would run such a target instead; teach it about the target first.'
        exit 2
    }

    # Function-local: on Windows PowerShell 5.1 cargo's progress on stderr is a terminating error under `Stop`
    # even when it is discarded, and the exit code is what says whether the build worked.
    $ErrorActionPreference = 'Continue'
    Push-Location -LiteralPath $RepositoryRoot
    try {
        $built =  @(& cargo test @CargoArgs --no-run --message-format=json 2>$null)
        if ($LASTEXITCODE -ne 0) {
            Write-Host "cargo could not build the tests (exit $LASTEXITCODE); run: cargo test $($CargoArgs -join ' ') --no-run"
            exit 2
        }
    }
    finally { Pop-Location }
    $executables = @($built | Where-Object { $_ -like '{*' } | ForEach-Object {
            try { $_ | ConvertFrom-Json } catch { $null }
        } | Where-Object { $null -ne $_ -and $_.reason -eq 'compiler-artifact' -and $_.executable -and $_.profile.test } |
            ForEach-Object { [pscustomobject]@{ Name = $_.target.name; Path = $_.executable } } |
            Sort-Object Path -Unique)
    if ($executables.Count -eq 0) {
        Write-Host 'cargo built no test executables, so there is nothing to ask.'
        exit 2
    }

    foreach ($exe in $executables) {
        $info = New-Object System.Diagnostics.ProcessStartInfo
        $info.FileName = $exe.Path
        $info.Arguments = '-Z unstable-options --format json --list'
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $info.RedirectStandardOutput = $true
        $info.RedirectStandardError = $true
        # The decoder is named, not the console's: a name or a reason need not be ASCII, and the console's code
        # page is not always UTF-8 (see Get-GitBlobUtf8 in common.ps1).
        $info.StandardOutputEncoding = New-Object System.Text.UTF8Encoding($false)
        $info.EnvironmentVariables['RUSTC_BOOTSTRAP'] = '1'
        $process = [System.Diagnostics.Process]::Start($info)
        try {
            $stderr = $process.StandardError.ReadToEndAsync()
            $stdout = $process.StandardOutput.ReadToEndAsync()
            if (-not $process.WaitForExit(120000)) {
                try { $process.Kill() } catch { $null = $_ }
                Write-Host "$($exe.Name) did not list its tests within 120 seconds."
                exit 2
            }
            $process.WaitForExit()
            if ($process.ExitCode -ne 0) {
                Write-Host "$($exe.Name) ($($exe.Path)) exited $($process.ExitCode) when asked to list its tests: $($stderr.Result.Trim())"
                exit 2
            }
            foreach ($line in ($stdout.Result -split "`r?`n")) {
                if (-not $line.TrimStart().StartsWith('{')) { continue }
                $event = ConvertFrom-LibtestLine -Line $line
                if ($null -ne $event) {
                    [pscustomobject]@{ binary = $exe.Name; name = $event.name; ignore = $event.ignore; ignore_message = $event.ignore_message }
                }
            }
        }
        finally { $process.Dispose() }
    }
}

$tests = @(if ($ListingPath) {
        if (-not (Test-Path -LiteralPath $ListingPath -PathType Leaf)) { Write-Host "No listing at $ListingPath."; exit 2 }
        foreach ($line in [System.IO.File]::ReadAllLines($ListingPath)) {
            if ($line.Trim().Length -eq 0) { continue }
            $record = $line | ConvertFrom-Json
            [pscustomobject]@{ binary = [string]$record.binary; name = [string]$record.name; ignore = [bool]$record.ignore; ignore_message = [string]$record.ignore_message }
        }
    }
    else { Get-TestListing })

$violations = @(Get-TierViolation -Tests $tests | Sort-Object Binary, Name, Problem)
if ($violations.Count -gt 0) {
    foreach ($v in $violations) {
        Write-Host "$($v.Binary): $($v.Name)"
        Write-Host "    $($v.Problem)"
        if ($v.Reason) { Write-Host "    ignore reason: $($v.Reason)" }
    }
    Write-Host ''
    Write-Host "$($violations.Count) test tier problem(s). The convention is in DEVELOPMENT.md, `"Test tiers`"."
    exit 1
}

$summary = @($tiers | ForEach-Object {
        $tier = $_
        "$($tier.Name) $(@($tests | Where-Object { $_.ignore -and $_.ignore_message.StartsWith($tier.Marker, [System.StringComparison]::Ordinal) }).Count)"
    }) -join ', '
Write-Host "Test tiers consistent: $($tests.Count) tests across $(@($tests | ForEach-Object { $_.binary } | Sort-Object -Unique).Count) executables; $summary."
exit 0
