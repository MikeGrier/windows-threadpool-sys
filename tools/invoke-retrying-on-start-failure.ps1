# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs a script, and retries it only when its failure is a process that
    Windows could not START -- never when the script itself failed.

.DESCRIPTION
    One PR #113 CI run of test-run-sabotage.ps1 failed four cases because their
    child processes died with 0xC0000142 (STATUS_DLL_INIT_FAILED) before running
    a line, and passed on re-run. That is the runner, not the code under test,
    so the step is retried -- but ONLY for that signature. A real failure must
    fail on the first attempt, or a retry would hide exactly the defects these
    suites exist to find.

    The signature comes from common.ps1 (Test-ProcessStartFailureText and
    Get-ProcessStartFailure), which every reporter there marks it with, so what
    counts as "could not start" is defined in one place.

    Every attempt's output is streamed as it is produced. A detected
    process-start failure is logged with a snapshot of the host's memory,
    commit, process and handle counts, and -- under GitHub Actions -- raised as a
    ::warning:: annotation, so a retried flake is visible on the run rather than
    absorbed by it.

    The script runs in a fresh process of the SAME PowerShell host as this one,
    so a step running under Windows PowerShell 5.1 still tests 5.1.

.EXAMPLE
    ./tools/invoke-retrying-on-start-failure.ps1 -Script ./tools/test-run-sabotage.ps1

.NOTES
    Exit code: the script's own exit code from its last attempt, or 2 for a
    configuration error here.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string] $Script,
    [string[]] $Arguments = @(),
    [int] $MaxAttempts = 2
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'note')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'note' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message }
    }
}

# A GitHub Actions annotation, which shows on the run's summary page. Outside
# Actions it would only be noise, so it is written only there.
function Write-Annotation {
    param([string] $Title, [string] $Message)
    if ($env:GITHUB_ACTIONS -ne 'true') { return }
    $escaped = $Message.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
    Write-Report "::warning title=$($Title)::$escaped"
}

if (-not (Test-Path -LiteralPath $Script)) {
    Write-Report "CONFIG ERROR: script not found: $Script" -Level bad
    exit 2
}
if ($MaxAttempts -lt 1) {
    Write-Report "CONFIG ERROR: -MaxAttempts must be at least 1, and was $MaxAttempts" -Level bad
    exit 2
}
$scriptPath = (Resolve-Path -LiteralPath $Script).Path
$hostExe = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName

# One attempt, output streamed through and kept. `Continue` locally for the
# reason Invoke-Native gives in common.ps1: redirected stderr from a native
# command is a terminating error under `Stop` on Windows PowerShell 5.1.
function Invoke-Attempt {
    $ErrorActionPreference = 'Continue'
    $captured = New-Object System.Collections.Generic.List[string]
    & $hostExe -NoProfile -ExecutionPolicy Bypass -File $scriptPath @Arguments 2>&1 |
        ConvertTo-OutputLines | ForEach-Object {
            Write-Report $_
            $captured.Add($_)
        }
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Text = ($captured -join "`n") }
}

$name = Split-Path -Leaf $scriptPath
for ($attempt = 1; $attempt -le $MaxAttempts; $attempt++) {
    if ($attempt -gt 1) {
        Write-Report ''
        Write-Report "=== $name, attempt $attempt of $MaxAttempts ===" -Level note
    }
    $result = Invoke-Attempt

    if ($result.ExitCode -eq 0) {
        if ($attempt -gt 1) {
            $message = "$name passed on attempt $attempt after a process-start failure on an earlier attempt. " +
            'The runner could not start a process; see the log above for the host state at the time.'
            Write-Report $message -Level note
            Write-Annotation -Title 'Retried after a process-start failure' -Message $message
        }
        exit 0
    }

    $startFailure = (Test-ProcessStartFailureText $result.Text) -or [bool](Get-ProcessStartFailure $result.ExitCode)
    if (-not $startFailure) {
        Write-Report ''
        Write-Report ("$name failed, exit $(Format-ExitCode $result.ExitCode), with no process-start " +
            'failure in its output. Not retried: a real failure fails on its first attempt.') -Level bad
        exit $result.ExitCode
    }

    Write-Report ''
    Write-Report ("=== $($script:ProcessStartFailureMarker) in $name, attempt $attempt of $MaxAttempts " +
        "(exit $(Format-ExitCode $result.ExitCode)). A child process could not be started; host state now:") -Level bad
    foreach ($line in (Get-HostPressureReport)) { Write-Report $line }
    Write-Annotation -Title 'Process-start failure' -Message ("$name, attempt $attempt of ${MaxAttempts}: a child " +
        'process could not be started (see the step log for the code and host state).')

    if ($attempt -eq $MaxAttempts) {
        Write-Report "Giving up after $MaxAttempts attempts." -Level bad
        exit $result.ExitCode
    }
}
