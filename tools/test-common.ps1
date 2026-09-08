# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [common.ps1](common.ps1), run on BOTH PowerShell hosts.

.DESCRIPTION
    Runs its cases in the host that invoked it, then re-invokes itself in the
    other host and requires that run to pass too.

    The cross-host run is the point of this file, not a nicety. The defect
    `Invoke-Native` exists to prevent appears ONLY under Windows PowerShell 5.1:
    PowerShell 7 captures a native command's stderr under `Stop` without
    complaint. A suite that tested one host would report green while the guard
    was broken on the only host that needs it -- which is how the original
    defect reached `main` and survived review, since CI runs `shell: pwsh`.

    It refuses to pass vacuously: if the other host cannot be found, that is a
    FAILURE rather than a skip, because "tested one host" is not the claim this
    file is here to make.

.PARAMETER SingleHost
    Run the cases in this process only, without re-invoking the other host.
    Used internally for the child run; also useful when debugging one host.
#>
[CmdletBinding()]
param(
    [switch] $SingleHost
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

$script:Failures = 0
$script:Host51 = 'C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe'

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

Write-Report "common.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# The guard's whole purpose. A bare `& { ... } 2>&1` here would throw on 5.1.
Test-Case 'a native command writing to stderr is captured, not thrown' {
    $out = Invoke-Native { cmd /c "echo to-stderr 1>&2" }
    Assert-Equal 'to-stderr' ("$out".Trim()) 'captured text'
}

Test-Case 'stdout and stderr are merged in one capture' {
    $out = Invoke-Native { cmd /c "echo on-out & echo on-err 1>&2" }
    $text = ($out -join ' ')
    if ($text -notmatch 'on-out' -or $text -notmatch 'on-err') {
        throw "both streams must appear, got '$text'"
    }
}

# The reason the script that motivated this cares: it tells a broken instrument
# from a finding by the exit code, so the guard must not swallow it.
Test-Case 'the exit code survives the capture' {
    $null = Invoke-Native { cmd /c "echo boom 1>&2 & exit 3" }
    Assert-Equal 3 $LASTEXITCODE 'LASTEXITCODE after a failing native command'
}

Test-Case 'a failing command still yields its diagnostic text' {
    $out = Invoke-Native { cmd /c "echo diagnostic-line 1>&2 & exit 1" }
    if ("$out" -notmatch 'diagnostic-line') {
        throw "the diagnostic must be captured, got '$out'"
    }
}

# Flattening is what keeps a transcript readable; without it a captured stderr
# line can stringify to `System.Management.Automation.RemoteException`.
Test-Case 'captured records are plain strings, not ErrorRecords' {
    $out = @(Invoke-Native { cmd /c "echo to-stderr 1>&2" })
    foreach ($line in $out) {
        if ($line -is [System.Management.Automation.ErrorRecord]) {
            throw 'a raw ErrorRecord escaped ConvertTo-OutputLines'
        }
    }
    if ("$out" -match 'RemoteException') {
        throw "a record stringified to RemoteException: '$out'"
    }
}

# The flip must REACH the scriptblock, and this is the only case that shows it
# directly. The stderr case above shows it too, but only on 5.1 -- PowerShell 7
# captures either way, so on 7 nothing else here distinguishes a guard that works
# from one that does nothing. Observing the preference from inside the passed
# scriptblock is host-independent.
Test-Case 'the flip reaches the scriptblock it is handed' {
    $seen = Invoke-Native { $ErrorActionPreference }
    Assert-Equal 'Continue' ("$seen".Trim()) 'ErrorActionPreference as seen inside the command'
}

# The three cases below are the OTHER direction: the flip must not escape.
#
# They are deliberately not described as testing a "restoration". `Invoke-Native`
# assigns to a function-local `$ErrorActionPreference`, so the caller's value is
# never modified and there is nothing to restore -- an earlier version wrapped
# the call in a `try/finally` that restored a copy nobody could observe, and
# these three cases could not fail against deleting it. Measured on both hosts.
#
# What they do catch is real and is the mutation worth guarding: writing
# `$script:ErrorActionPreference` or `$global:` in `Invoke-Native` would leave
# the CALLER running under `Continue` for everything afterwards, silently
# disarming `Stop` for the rest of the script. A `$script:`-scoped mutant leaves
# the caller at `Continue` and fails all three.
# Each of these three sets `$script:ErrorActionPreference` to a known value
# first, rather than capturing whatever it happens to be. That is not ceremony:
# a mutant that escapes to script scope leaks `Continue` on its FIRST call, so a
# later case reading "before" would capture `Continue`, compare it against
# `Continue` afterwards, and pass -- the contamination hiding itself. Measured:
# without this reset, a `$script:`-scoped mutant was caught only by the
# behavioural case below, and the two variable-observing cases passed.
Test-Case 'the flip does not escape into the caller' {
    $script:ErrorActionPreference = 'Stop'
    $null = Invoke-Native { cmd /c "echo to-stderr 1>&2" }
    Assert-Equal 'Stop' $ErrorActionPreference 'the caller''s ErrorActionPreference after the call'
}

# The same property observed through behaviour rather than through the variable:
# `Stop` must still terminate on something that is not the native call.
Test-Case 'Stop still terminates a non-native error after the call' {
    $script:ErrorActionPreference = 'Stop'
    $null = Invoke-Native { cmd /c "echo to-stderr 1>&2" }
    $threw = $false
    try { Get-Item 'Q:\no\such\path\at\all.txt' | Out-Null } catch { $threw = $true }
    if (-not $threw) { throw 'Stop was left disarmed for cmdlet errors' }
}

# And on the path where the command throws, which is where a scope-escaping
# assignment would be least likely to be noticed by hand.
Test-Case 'the flip does not escape when the command throws' {
    $script:ErrorActionPreference = 'Stop'
    try { $null = Invoke-Native { throw 'deliberate' } } catch { }
    Assert-Equal 'Stop' $ErrorActionPreference 'the caller''s ErrorActionPreference after a throw'
}

if (-not $SingleHost) {
    # The other host, which is the claim this file exists to make.
    $isSeven = $PSVersionTable.PSVersion.Major -ge 6
    $other = if ($isSeven) { $script:Host51 } else { 'pwsh' }
    $otherName = if ($isSeven) { 'Windows PowerShell 5.1' } else { 'PowerShell 7' }

    $resolved = if ($isSeven) {
        if (Test-Path $other) { $other } else { $null }
    }
    else {
        $command = Get-Command $other -ErrorAction SilentlyContinue
        if ($command) { $command.Source } else { $null }
    }

    if (-not $resolved) {
        $script:Failures++
        Write-Report ''
        Write-Report "FAIL  $otherName was not found, so the cross-host claim is untested." -Level bad
        Write-Report '      This suite exists to prove the guard on BOTH hosts: the defect it'
        Write-Report '      guards against appears only on 5.1, so a single-host pass is not'
        Write-Report '      the result this file reports. Install the missing host or run it'
        Write-Report '      there by hand rather than treating this as a skip.'
    }
    else {
        Write-Report ''
        Write-Report "=== re-running under $otherName ===" -Level heading
        & $resolved -NoProfile -File $PSCommandPath -SingleHost
        if ($LASTEXITCODE -ne 0) {
            $script:Failures++
            Write-Report "FAIL  the $otherName run reported failures." -Level bad
        }
    }
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)." -Level bad
    exit 1
}

Write-Report 'All cases passed.' -Level good
exit 0
