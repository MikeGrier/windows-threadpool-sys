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
