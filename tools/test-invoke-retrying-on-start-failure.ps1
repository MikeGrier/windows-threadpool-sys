# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [invoke-retrying-on-start-failure.ps1](invoke-retrying-on-start-failure.ps1),
    on whichever PowerShell host runs it. Exits 0 only if every case passes.

.DESCRIPTION
    The wrapper's whole value is the line it draws: retry a process-start
    failure, never retry anything else. So both sides are tested, with fixture
    scripts that count their own attempts in a file -- a wrapper that retried
    everything, or nothing, fails this suite.

    The wrapper runs its target under the SAME host it runs in, so CI runs this
    suite under both pwsh and Windows PowerShell 5.1.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell
    5.1 ships Pester 3, whose syntax differs incompatibly from Pester 5.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Wrapper = Join-Path $PSScriptRoot 'invoke-retrying-on-start-failure.ps1'
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
    if ("$Expected" -ne "$Actual") { throw "$What -- expected '$Expected', got '$Actual'" }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $What)
    if ($Text -notmatch $Pattern) { throw "$What -- expected /$Pattern/ in: $Text" }
}

function Assert-NoMatch {
    param([string] $Pattern, [string] $Text, [string] $What)
    if ($Text -match $Pattern) { throw "$What -- did not expect /$Pattern/ in: $Text" }
}

# A fixture script that records each attempt, then runs $Body with $attempt set.
function New-Target {
    param([string] $Body)
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ('irsf-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    $counter = Join-Path $dir 'attempts.txt'
    $text = @(
        "`$counter = '$counter'",
        "`$attempt = 1 + @(if (Test-Path `$counter) { Get-Content `$counter }).Count",
        "Add-Content -Path `$counter -Value `$attempt",
        $Body
    ) -join "`r`n"
    $path = Join-Path $dir 'target.ps1'
    [System.IO.File]::WriteAllText($path, $text)
    return [pscustomobject]@{ Dir = $dir; Path = $path; Counter = $counter }
}

function Invoke-Wrapper {
    param($Target, [int] $MaxAttempts = 2, [switch] $InActions)
    $saved = $env:GITHUB_ACTIONS
    try {
        $env:GITHUB_ACTIONS = if ($InActions) { 'true' } else { $null }
        $output = & $script:Wrapper -Script $Target.Path -MaxAttempts $MaxAttempts *>&1 | Out-String
        $code = $LASTEXITCODE
    }
    finally { $env:GITHUB_ACTIONS = $saved }
    $attempts = if (Test-Path $Target.Counter) { @(Get-Content $Target.Counter).Count } else { 0 }
    return [pscustomobject]@{ ExitCode = $code; Output = $output; Attempts = $attempts }
}

Write-Report "invoke-retrying-on-start-failure.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# --- Not retried -------------------------------------------------------------

Test-Case 'a passing script runs once and passes' {
    $t = New-Target "Write-Host 'all good'; exit 0"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 0 $r.ExitCode $r.Output
        Assert-Equal 1 $r.Attempts 'attempts'
        Assert-Match 'all good' $r.Output 'the output is streamed through'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'an ordinary failure is not retried, and keeps its exit code' {
    $t = New-Target "Write-Host 'expected [0], got [101]'; exit 3"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 3 $r.ExitCode $r.Output
        Assert-Equal 1 $r.Attempts 'a real failure must fail on its first attempt'
        Assert-Match 'Not retried' $r.Output 'the decision is stated'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'a failure that merely mentions a large number is not retried' {
    # Negative and longer, so only the detector's digit boundaries reject it.
    $t = New-Target "Write-Host 'offset -10737415029'; exit 1"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 1 $r.Attempts 'a digit run containing the code is not the code'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

# --- Retried -----------------------------------------------------------------

Test-Case 'a raw process-start code in the output is retried, and a pass then passes' {
    # The exact text the CI failure printed, before the reporters named codes.
    $t = New-Target "if (`$attempt -eq 1) { Write-Host 'expected [0], got [-1073741502]'; exit 1 }; Write-Host 'second time lucky'; exit 0"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 0 $r.ExitCode $r.Output
        Assert-Equal 2 $r.Attempts 'attempts'
        Assert-Match 'PROCESS-START FAILURE in target\.ps1, attempt 1 of 2' $r.Output 'the detection is logged'
        Assert-Match 'physical memory free' $r.Output 'the host state is logged'
        Assert-Match 'passed on attempt 2' $r.Output 'the retry is reported, not absorbed'
        Assert-NoMatch '::warning' $r.Output 'no annotation outside GitHub Actions'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'the named marker is retried too' {
    $t = New-Target ". '$PSScriptRoot\common.ps1'; if (`$attempt -eq 1) { Write-Host (Format-ExitCode -1073741502); exit 1 }; exit 0"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 0 $r.ExitCode $r.Output
        Assert-Equal 2 $r.Attempts 'attempts'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'a script that itself exits with a process-start code is retried' {
    $t = New-Target "if (`$attempt -eq 1) { exit -1073741502 }; exit 0"
    try {
        $r = Invoke-Wrapper $t
        Assert-Equal 0 $r.ExitCode $r.Output
        Assert-Equal 2 $r.Attempts 'attempts'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'a persistent process-start failure gives up after the last attempt' {
    $t = New-Target "Write-Host 'got [-1073741502]'; exit 7"
    try {
        $r = Invoke-Wrapper $t -MaxAttempts 3
        Assert-Equal 7 $r.ExitCode $r.Output
        Assert-Equal 3 $r.Attempts 'every allowed attempt is used, and no more'
        Assert-Match 'Giving up after 3 attempts' $r.Output 'the outcome is stated'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Test-Case 'under GitHub Actions the failure and the retry are annotated' {
    $t = New-Target "if (`$attempt -eq 1) { Write-Host 'got [-1073741502]'; exit 1 }; exit 0"
    try {
        $r = Invoke-Wrapper $t -InActions
        Assert-Equal 0 $r.ExitCode $r.Output
        Assert-Match '::warning title=Process-start failure::' $r.Output 'the failure is annotated'
        Assert-Match '::warning title=Retried after a process-start failure::' $r.Output 'the retry is annotated'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

# --- Configuration errors ----------------------------------------------------

Test-Case 'a missing script is a configuration error' {
    $output = & $script:Wrapper -Script (Join-Path ([System.IO.Path]::GetTempPath()) 'irsf-absent.ps1') *>&1 | Out-String
    Assert-Equal 2 $LASTEXITCODE $output
}

Test-Case 'fewer than one attempt is a configuration error' {
    $t = New-Target 'exit 0'
    try {
        $r = Invoke-Wrapper $t -MaxAttempts 0
        Assert-Equal 2 $r.ExitCode $r.Output
        Assert-Equal 0 $r.Attempts 'nothing ran'
    }
    finally { Remove-Item $t.Dir -Recurse -Force }
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)." -Level bad
    exit 1
}
Write-Report 'All cases passed.' -Level good
exit 0
