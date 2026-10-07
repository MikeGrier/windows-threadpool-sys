# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Shared support for the scripts in `tools/`. Dot-source it; do not run it.

.DESCRIPTION
    Dot-sourced rather than imported as a module, and that is a correctness
    requirement rather than a style choice. See "Why not a module" below before
    converting this to a `.psm1`.

    Every script here that captures a native command's output needs the same
    guard against Windows PowerShell 5.1's treatment of stderr, and that guard
    had been copied into three scripts by the time it was written down. This is
    the one copy.

.NOTES
    Usage, from any script in this directory:

        . (Join-Path $PSScriptRoot 'common.ps1')

    `$PSScriptRoot` is populated in a script body on both hosts. It is NOT
    populated while evaluating a parameter default on a `[CmdletBinding()]`
    script under 5.1, so keep this call in the body, as the scripts here do for
    their own `-OutputDirectory` defaults.
#>

# Render a captured record as plain text.
#
# `2>&1` wraps a native command's stderr in ErrorRecords. One of those
# stringifies to the literal text `System.Management.Automation.RemoteException`,
# which lands in the middle of a captured compiler diagnostic and makes a
# transcript less legible than the console output of the same failure.
function ConvertTo-OutputLines {
    param([Parameter(ValueFromPipeline = $true)] $Record)
    process {
        if ($Record -is [System.Management.Automation.ErrorRecord]) {
            $Record.Exception.Message
        }
        else {
            "$Record"
        }
    }
}

# Run a native command, capturing merged stdout+stderr as plain strings.
#
# Under Windows PowerShell 5.1, a native command that writes to stderr while
# `$ErrorActionPreference` is `Stop` raises a TERMINATING error when its stderr
# is redirected with `2>&1`. PowerShell 7 does not. Flipping the preference to
# `Continue` for the duration of the call is what makes the capture work on both,
# and keeping that flip function-local is what leaves `Stop` in force for
# everything that is not a native call.
#
# **No restoration is needed, and none is attempted.** `$ErrorActionPreference =
# 'Continue'` here creates a FUNCTION-LOCAL variable: PowerShell assignment
# always writes to the current scope, so the caller's own value is untouched and
# the local one is discarded when this returns. `& $Command` runs the scriptblock
# in a child of this scope, so it inherits `Continue` -- which is exactly the
# reach the guard needs -- while nothing outside sees it.
#
# An earlier version wrapped the call in `try { } finally { $ErrorActionPreference
# = $previous }`. That restored the local copy nobody could observe, so it was
# dead code, and worse: three cases in `test-common.ps1` claimed to cover it and
# could not fail. Measured on both hosts -- with the `try/finally` deleted
# outright, the caller still reads `Stop` immediately after the call, identically
# to the version that had it.
#
# The property that DOES need a test is the other direction: that the flip never
# escapes into the caller. Writing `$script:` or `$global:` here would leave the
# caller running under `Continue` for everything afterwards, and `test-common.ps1`
# covers that (a `$script:`-scoped mutant leaves the caller at `Continue` and
# fails those cases).
#
# `$LASTEXITCODE` is global, so a caller still reads the command's exit code
# after this returns. That matters here: these scripts distinguish a broken
# instrument from a finding by exactly that code.
#
# ## Why not a module
#
# Moving this function into a `.psm1` and importing it SILENTLY BREAKS IT under
# 5.1, which is the only host it exists to protect.
#
# A scriptblock carries the session state it was created in. `Invoke-Native
# { cargo build }` builds that scriptblock in the CALLER's script scope, so
# `& $Command` runs it there -- not in the module's scope. A module copy of this
# function sets `$ErrorActionPreference` in the module's own scope, the flip
# never reaches the scriptblock, and the native call still runs under `Stop`.
#
# Measured, because the failure is invisible on the host most people run: with
# the identical body in a `.psm1`, 5.1 threw `RemoteException` and captured
# nothing while PowerShell 7 succeeded. Dot-sourcing lands the function in the
# caller's own scope, where the plain assignment below does reach the call, and
# both hosts then behave identically.
#
# A module CAN be made to work by reaching into the caller's session state
# (`$PSCmdlet.SessionState.PSVariable.Set(...)`), and that was measured working
# too. It is rejected because the guard would then depend on a subtlety that
# looks removable: anyone simplifying it back to a plain assignment would
# reintroduce a defect that still passes on PowerShell 7 and in CI. Dot-sourcing
# makes the property hold by construction instead of by counter-measure.
#
# [test-common.ps1](test-common.ps1) asserts this on both hosts.
function Invoke-Native {
    param([Parameter(Mandatory = $true)][scriptblock] $Command)
    # Function-local by construction -- see the note above on why there is no
    # restoration to do. Never `$script:` or `$global:` here.
    $ErrorActionPreference = 'Continue'
    & $Command 2>&1 | ConvertTo-OutputLines
}

# Run a native command and capture ONLY its stdout, discarding stderr.
#
# **Use this whenever the output is PARSED rather than shown.** `Invoke-Native`
# above merges stderr into the capture, which is right for a transcript and
# wrong for data: a command that writes progress or warnings to stderr while
# succeeding on stdout produces a capture with the two interleaved, and the
# parse then fails on text that was never part of the answer.
#
# That is not hypothetical. Routing `cargo metadata --no-deps --format-version 1`
# through the merging helper turned green locally and red in CI, because a warm
# workspace writes nothing to stderr while a cold runner emits rustup's
# `info: syncing channel updates` -- so `ConvertFrom-Json` failed with
# "Unexpected character encountered while parsing value: i". Reproduced on both
# hosts against a stand-in that writes both streams.
#
# The redirect is still what makes this need the same `Continue` flip: on
# Windows PowerShell 5.1 ANY stderr redirect, `2>$null` included, turns a native
# command's stderr into a terminating error under `Stop`. Measured -- both
# spellings throw, an unredirected call does not.
#
# `$LASTEXITCODE` survives, so a caller still distinguishes success from
# failure; what it loses is the diagnostic text, which is the trade a parsed
# command is making anyway.
function Invoke-NativeStdout {
    param([Parameter(Mandatory = $true)][scriptblock] $Command)
    $ErrorActionPreference = 'Continue'
    & $Command 2>$null
}

# Run a native command and capture its streams SEPARATELY.
#
# For the case `Invoke-NativeStdout` cannot serve: output that is parsed on
# success, but whose stderr is the diagnostic worth reporting on failure.
# Discarding stderr keeps the parse clean and throws away the only explanation
# of what went wrong, and merging keeps the explanation and corrupts the parse;
# this keeps both by not choosing.
#
# Measured, because which stream carries the message is NOT uniform and the
# obvious assumption is wrong for the common case. `gh`:
#
#   REST 404          stdout carries the JSON error body, stderr `gh: Not Found`
#   network failure   stdout EMPTY, stderr `error connecting to ...`
#   usage error       stdout EMPTY, stderr the usage text
#
# So a failure reported from stdout alone is blank exactly when the cause is
# least guessable -- an unreachable host, a bad flag, an auth problem -- which
# is the shape a broken-instrument message exists to explain.
#
# Returns an object with `Stdout`, `Stderr` and `ExitCode`, both texts already
# flattened to plain strings. `$LASTEXITCODE` is also left set, so a caller that
# only wants the code need not unpack anything.
function Invoke-NativeSplit {
    param([Parameter(Mandatory = $true)][scriptblock] $Command)
    $ErrorActionPreference = 'Continue'

    # Merged with `2>&1`, then partitioned by RECORD TYPE: PowerShell wraps a
    # native command's stderr in ErrorRecords and leaves stdout as plain
    # strings, so the merge is losslessly separable even though it looks like a
    # mixed stream.
    #
    # A file redirect (`2>$path`) is the obvious alternative and is WRONG on
    # Windows PowerShell 5.1. There it writes PowerShell's *formatted* error
    # record to the file -- `cmd.exe : to-err`, then the offending source line, a
    # caret ruler, CategoryInfo and FullyQualifiedErrorId -- rather than the raw
    # stderr text, so the diagnostic would arrive wrapped in a stack trace of
    # this helper. PowerShell 7 writes the raw text, so that version passed there
    # and failed on 5.1; the cross-host suite caught it.
    $merged = & $Command 2>&1
    $code = $LASTEXITCODE

    $stdout = @($merged | Where-Object { $_ -isnot [System.Management.Automation.ErrorRecord] })
    $stderr = @($merged |
            Where-Object { $_ -is [System.Management.Automation.ErrorRecord] } |
            ForEach-Object { $_.Exception.Message }) -join "`n"

    return [pscustomobject]@{
        Stdout   = $stdout
        Stderr   = $stderr
        ExitCode = $code
    }
}

# Read a whole text file that another handle may still have open for writing.
#
# `[IO.File]::ReadAllText` opens with `FileShare.Read`, which REFUSES the open
# while any other handle holds write access -- it throws "The process cannot
# access the file ... because it is being used by another process". A file a
# native command's stdout or stderr was redirected into is exactly such a file,
# and it stays one for a moment after the process reports `HasExited`.
#
# Measured, both hosts, with nothing but `cmd /c echo` as the child: reading the
# redirect file immediately after `HasExited` turned true failed 7 times in 40
# with `ReadAllText`, and 0 times in 40 with this, which also saw the child's
# complete output every time. So the lingering handle is not a grandchild still
# writing; but if one ever were, this returns what has been written so far
# rather than throwing. The caller that motivated it searches the text for the
# message a build was declared to fail with, so a short read can only turn a
# match into a miss -- withholding credit, never granting it.
#
# Found by CI: run-sabotage.ps1 read a build's stderr this way, and the harness
# suite went red under 5.1 on a case whose logic was correct.
function Read-SharedText {
    param([Parameter(Mandatory = $true)][string] $Path)
    $share = [System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete
    $stream = New-Object System.IO.FileStream($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, $share)
    try {
        $reader = New-Object System.IO.StreamReader($stream)
        try { return $reader.ReadToEnd() } finally { $reader.Dispose() }
    }
    finally { $stream.Dispose() }
}

# --- A process that never started --------------------------------------------
#
# Exit codes that mean Windows could not START a child process at all, so it
# never ran its own code and reported nothing. Recorded once here; everything
# that reports an exit code, and the CI retry wrapper that keys on these, asks
# this table.
#
# Why it exists: one PR #113 CI run of test-run-sabotage.ps1 had four cases get
# 0xC0000142 (STATUS_DLL_INIT_FAILED) from their child processes, one of them
# `git`, which the harness then reported as "Not inside a git repository" --
# the wrong diagnosis, since nothing had asked git anything. The job passed on
# re-run. Without a name for the code, the log said nothing about the cause.
#
# The marker is what a log reader, and the retry wrapper, search for. Keep it
# stable: invoke-retrying-on-start-failure.ps1 decides whether to retry on it.
$script:ProcessStartFailureMarker = 'PROCESS-START FAILURE'
$script:ProcessStartFailures = [ordered]@{
    0xC0000142L = 'STATUS_DLL_INIT_FAILED'
    0xC0000135L = 'STATUS_DLL_NOT_FOUND'
    0xC0000139L = 'STATUS_ENTRYPOINT_NOT_FOUND'
    0xC000007BL = 'STATUS_INVALID_IMAGE_FORMAT'
    0xC0000017L = 'STATUS_NO_MEMORY'
    0xC000012DL = 'STATUS_COMMITMENT_LIMIT'
}

# The NTSTATUS name of an exit code that means the process never started, or
# $null for any other code -- including success, an ordinary failure, and no
# code at all. Accepts the signed form PowerShell reports in $LASTEXITCODE.
function Get-ProcessStartFailure {
    param([AllowNull()] $ExitCode)
    # Anything that is not a whole number is not an exit code, and callers
    # such as an assertion's failure message pass whatever they compared.
    $value = 0L
    if ($null -eq $ExitCode -or -not [int64]::TryParse("$ExitCode", [ref]$value)) { return $null }
    $unsigned = $value -band 0xFFFFFFFFL
    foreach ($code in $script:ProcessStartFailures.Keys) {
        if ($unsigned -eq $code) {
            return ('0x{0:X8} {1}' -f $code, $script:ProcessStartFailures[$code])
        }
    }
    return $null
}

# An exit code as text, naming it -- and marking it -- when it is a
# process-start failure, so a log line says what happened instead of printing
# a bare negative number.
function Format-ExitCode {
    param([AllowNull()] $ExitCode)
    $start = Get-ProcessStartFailure $ExitCode
    if ($start) {
        return "$ExitCode [$($script:ProcessStartFailureMarker) $($start): Windows could not start the process, so it reported nothing]"
    }
    return "$ExitCode"
}

# Whether captured output shows a process-start failure: the marker above, or
# one of the table's codes written raw, in decimal or hex, by something that
# did not go through Format-ExitCode.
function Test-ProcessStartFailureText {
    param([AllowNull()][string] $Text)
    if (-not $Text) { return $false }
    if ($Text.Contains($script:ProcessStartFailureMarker)) { return $true }
    # Every form Get-ProcessStartFailure accepts: signed decimal (what
    # $LASTEXITCODE holds), unsigned decimal, and hex. The unsigned decimal
    # form was missing until the PR #113 review. Digit boundaries on both
    # decimal forms, so a longer number that merely contains one is not it.
    foreach ($code in $script:ProcessStartFailures.Keys) {
        $signed = [int32]([int64]$code - 0x100000000L)
        $unsigned = [uint32]$code
        if ($Text -match "(?<![\d-])$signed(?!\d)" -or
            $Text -match "(?<![\d-])$unsigned(?!\d)" -or
            $Text -match ('(?i)\b0x{0:X8}\b' -f $code)) { return $true }
    }
    return $false
}

# What the host looked like at the moment of a failure, as plain lines. Best
# effort: every figure that cannot be read is reported as unknown rather than
# failing the report, because this runs on a path that is already failing.
function Get-HostPressureReport {
    $lines = New-Object System.Collections.Generic.List[string]
    try {
        $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
        $lines.Add(('  physical memory free: {0:N0} MiB of {1:N0} MiB' -f
                ($os.FreePhysicalMemory / 1KB), ($os.TotalVisibleMemorySize / 1KB))) | Out-Null
        $lines.Add(('  commit (virtual) free: {0:N0} MiB of {1:N0} MiB' -f
                ($os.FreeVirtualMemory / 1KB), ($os.TotalVirtualMemorySize / 1KB))) | Out-Null
        $lines.Add("  processes (OS count): $($os.NumberOfProcesses)") | Out-Null
    }
    catch { $lines.Add("  memory and OS process count: unknown ($($_.Exception.Message))") | Out-Null }
    try {
        $processes = @(Get-Process -ErrorAction Stop)
        $handles = ($processes | Measure-Object -Property HandleCount -Sum).Sum
        $lines.Add("  handles across visible processes: $handles") | Out-Null
        $top = $processes | Group-Object ProcessName | Sort-Object Count -Descending | Select-Object -First 5 |
            ForEach-Object { "$($_.Name) x$($_.Count)" }
        $lines.Add("  most numerous processes: $($top -join ', ')") | Out-Null
    }
    catch { $lines.Add("  process and handle counts: unknown ($($_.Exception.Message))") | Out-Null }
    # Unrolled: there are always at least two lines, so callers get an array
    # whether they write `@(Get-HostPressureReport)` or `foreach`. A
    # comma-wrapped return made `@(...)` a one-element array of arrays.
    return $lines.ToArray()
}

# Builds win-job-launcher, the job-object launcher the sabotage harness runs
# every phase through, and returns the path of its executable.
#
# Built from THIS checkout's crates/win-job-launcher -- found from this file,
# never from the repository being swept -- because the launcher is the
# harness's own machinery. A sweep of a fixture repository has no launcher crate
# at all, and a sweep of the launcher's own manifest patches a COPY of it; in
# both cases the supervisor must be the unsabotaged build. Throws on a failed
# build, with cargo's output, so each caller decides how to report it.
function Build-JobLauncher {
    param([Parameter(Mandatory = $true)][string] $TargetDirectory)
    $manifest = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\crates\win-job-launcher\Cargo.toml'))
    if (-not (Test-Path -LiteralPath $manifest)) {
        throw "No launcher crate at $manifest."
    }
    $output = @(Invoke-Native { cargo build --quiet --manifest-path $manifest --target-dir $TargetDirectory })
    if ($LASTEXITCODE -ne 0) {
        throw ((@("Could not build win-job-launcher (cargo exit $(Format-ExitCode $LASTEXITCODE)):") + $output) -join "`n")
    }
    return Join-Path $TargetDirectory 'debug\win-job-launcher.exe'
}
