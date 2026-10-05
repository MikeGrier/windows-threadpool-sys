# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-compile-fail-doctests.ps1](check-compile-fail-doctests.ps1).
    Exits 0 only if every case passes.

.DESCRIPTION
    The check trusts rustdoc for WHICH doctests are compile_fail, reads each
    one's info string from source, and asks check-compile-fail-codes.ps1
    whether that fence is recognised and pinned. Each failure it exists to
    report is reproduced here from a synthetic transcript in rustdoc's own
    format, beside the cases it must accept, so a check that passed
    everything -- or failed everything -- cannot pass this suite.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell
    5.1 ships Pester 3, whose syntax differs incompatibly from Pester 5.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Script = Join-Path $PSScriptRoot 'check-compile-fail-doctests.ps1'
$script:Failures = 0

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'heading')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'heading' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Test-Case {
    param([string] $Name, [scriptblock] $Body)
    try {
        & $Body
        Write-Report "  PASS  $Name" -Level good
    }
    catch {
        $script:Failures++
        Write-Report "  FAIL  $Name" -Level bad
        Write-Report "        $($_.Exception.Message)"
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $What)
    if ("$Expected" -ne "$Actual") {
        throw "$What -- expected '$Expected', got '$Actual'"
    }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $What)
    if ($Text -notmatch $Pattern) {
        throw "$What -- expected /$Pattern/ in: $Text"
    }
}

# A throwaway workspace: source files under crates/, keyed by relative path.
function New-Fixture {
    param([hashtable] $Files)
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("cfd-" + [guid]::NewGuid().ToString('N'))
    foreach ($relative in $Files.Keys) {
        $path = Join-Path $root $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, ($Files[$relative] -replace "`r`n", "`n"))
    }
    return $root
}

function Remove-Fixture {
    param([string] $Root)
    Remove-Item $Root -Recurse -Force -ErrorAction SilentlyContinue
}

# One rustdoc result line, in the format a `cargo test --doc` run prints.
function Format-Entry {
    param([string] $Path, [int] $Line, [string] $Kind = 'compile fail', [string] $Result = 'ok')
    return "test $Path - item (line $Line) - $Kind ... $Result"
}

# Writes each transcript (an array of lines) into the fixture and runs the check.
function Invoke-Check {
    param([string] $Root, [object[]] $Transcripts)
    $paths = @()
    $i = 0
    foreach ($lines in $Transcripts) {
        $path = Join-Path $Root "transcript-$i.txt"
        [System.IO.File]::WriteAllLines($path, [string[]]@($lines))
        $paths += $path
        $i++
    }
    # `*>&1`: the script reports through `Write-Host`.
    $output = & $script:Script -Transcript $paths -Root $Root -ScanRoot (Join-Path $Root 'crates') *>&1 | Out-String
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
}

$lib = 'crates\a\src\lib.rs'

Write-Report "check-compile-fail-doctests.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# --- Accepted ----------------------------------------------------------------

Test-Case 'a pinned doctest at its reported fence line passes' {
    $root = New-Fixture @{ $lib = "/// Doc.`n/// ``````compile_fail,E0308`n/// let x: u32 = ""no"";`n/// ```````nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @(
                (Format-Entry $lib 2),
                # A no_run doctest is `- compile`, and must not be mistaken for one.
                (Format-Entry $lib 9 -Kind 'compile')))
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'rustdoc ran \(1\)' $result.Output 'exactly one compile_fail doctest is counted'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'an attribute doctest reported at the attribute, fence lines below, passes' {
    # Measured: rustdoc reports a `doc = "..."` doctest at the attribute's
    # first line, not at the fence.
    $root = New-Fixture @{ $lib = "#[cfg_attr(`n    x,`n    doc = ""``````compile_fail,E0599"",`n    doc = ""``````""`n)]`nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1)))
        Assert-Equal 0 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'two doctests reported at one line claim two fences' {
    $root = New-Fixture @{ $lib = "#[doc = r#""`n``````compile_fail,E0308`n```````n``````compile_fail,E0599`n```````n""#]`nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1), (Format-Entry $lib 1)))
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'rustdoc ran \(2\)' $result.Output 'both are counted'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a doctest listed by several configurations is checked once' {
    $root = New-Fixture @{ $lib = "/// ``````compile_fail,E0308`n/// ```````nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(@((Format-Entry $lib 1)), @((Format-Entry $lib 1)), @((Format-Entry $lib 1)))
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'rustdoc ran \(1\)' $result.Output 'counted once, not three times'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'markdown reached through include_str! resolves by its rustdoc path' {
    $root = New-Fixture @{ 'crates\a\README.md' = "# A`n`n``````compile_fail,E0308`nlet x: u32 = ""no"";`n```````n" }
    try {
        # rustdoc names it relative to the including file, `..` and all.
        $result = Invoke-Check $root @(, @((Format-Entry 'crates\a\src\../README.md' 3)))
        Assert-Equal 0 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

# --- Refused -----------------------------------------------------------------

Test-Case 'an unpinned doctest the guard recognises is refused' {
    $root = New-Fixture @{ $lib = "/// ``````compile_fail`n/// ```````nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1)))
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'names no error code' $result.Output 'the reason is given'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'an ignored unpinned doctest is still refused' {
    $root = New-Fixture @{ $lib = "/// ``````compile_fail,ignore`n/// ```````nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1 -Result 'ignored')))
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'names no error code' $result.Output 'the reason is given'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a fence the guard does not recognise is a blind spot, even when pinned' {
    # The case this check exists for: rustdoc runs it, the regex guard cannot
    # see it, and the guard alone would report green. Measured on the real
    # tree with heal.rs's fence rewritten this way.
    $root = New-Fixture @{ $lib = "#[doc = concat!(""``````compile_fail,E0308"")]`n#[doc = ""``````""]`nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1)))
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'does not recognise' $result.Output 'the blind spot is named'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a doctest with no readable fence is refused, not passed' {
    # A fence assembled by concat! has no `compile_fail` fence on any line.
    $root = New-Fixture @{ $lib = "#[doc = concat!(""````"", ""`compile_fail"")]`n#[doc = ""``````""]`nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1)))
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'no compile_fail fence' $result.Output 'the reason is given'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a fence is claimed once, so a second doctest cannot reuse it' {
    $root = New-Fixture @{ $lib = "#[doc = r#""`n``````compile_fail,E0308`n```````n""#]`nfn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @((Format-Entry $lib 1), (Format-Entry $lib 1)))
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'no compile_fail fence' $result.Output 'the second has nothing left to claim'
    }
    finally { Remove-Fixture $root }
}

# --- Configuration errors ----------------------------------------------------

Test-Case 'transcripts that name no compile_fail doctest are an error, not a pass' {
    $root = New-Fixture @{ $lib = "fn f() {}`n" }
    try {
        $result = Invoke-Check $root @(, @('running 0 tests', 'test result: ok. 0 passed'))
        Assert-Equal 2 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a missing transcript is an error, not a pass' {
    $root = New-Fixture @{ $lib = "fn f() {}`n" }
    try {
        $output = & $script:Script -Transcript (Join-Path $root 'absent.txt') -Root $root *>&1 | Out-String
        Assert-Equal 2 $LASTEXITCODE $output
    }
    finally { Remove-Fixture $root }
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)." -Level bad
    exit 1
}
Write-Report 'All cases passed.' -Level good
exit 0
