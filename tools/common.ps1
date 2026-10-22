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

# --- A link that could not open its output -----------------------------------
#
# `link.exe` reports `LINK : fatal error LNK1104: cannot open file '<path>'`
# when it cannot open a file it was given. For a test executable that is the
# link's OUTPUT, and the usual reason is that some other process still has the
# previous build's copy open: a scanner or an inventory service looking at a
# freshly run test binary for a moment after it exited. A sweep links the same
# path again for every entry, a second or two after the last one ran, so it
# meets this by chance. The cargo build then fails with nothing wrong in the
# patch or in the suite, and the same entry run alone builds. The evidence is in
# DESIGN-RATIONALE.md, "A link that could not open its output".
#
# Why this one signature, and not "retry a failed build": a build that fails
# because the patch is not valid Rust must fail on the first attempt, or a retry
# would double the cost of every manifest that is wrong. LNK1104 is also what the
# linker says for an INPUT it cannot find (`kernel32.lib`, a missing `.rlib`), and
# that is not transient. So the file named has to be an executable or a library
# it produces, `.exe` or `.dll`, and EVERY LNK1104 in the output has to be of that
# kind: one that names an input makes the failure the build's own.
#
# What the signature cannot tell apart is a held file from any other refusal to
# open an output -- a path too long, a missing directory. A retry of one of
# those fails the same way, and the caller says so after the second attempt,
# which costs one more build and hides nothing.
#
# Returns the path of the first file named, or $null when the text has no
# LNK1104, or one that is not an output of this kind.
#
# The diagnostic is recognised only as a LINE of its own: the linker's `LINK :`
# prefix, optionally behind cargo's `= note:` marker, at the start of the line
# and ending at the line's last quote. Two ways of being wrong that avoids. The
# same words inside a compiler's quoted source line -- a string in the patch
# that spells the message -- are not a linker diagnostic, and reading one as
# such would turn a compile failure into `INFRASTRUCTURE` and bypass
# `refused-by-build` scoring. And a path may contain an apostrophe, so the
# capture runs to the last quote on the line, not the first.
function Get-LinkOutputOpenFailure {
    param([AllowNull()][string] $Text)
    if (-not $Text) { return $null }
    $first = $null
    $pattern = "(?m)^[ \t]*(?:=[ \t]*note:[ \t]*)?LINK : fatal error LNK1104: cannot open file '([^\r\n]+)'[ \t]*\r?$"
    foreach ($match in [regex]::Matches($Text, $pattern)) {
        $path = $match.Groups[1].Value
        if ($path -notmatch '(?i)\.(exe|dll)$') { return $null }
        if ($null -eq $first) { $first = $path }
    }
    return $first
}

# --- Sabotage manifests and their patches -------------------------------------
#
# Where the manifests are. The planner builds the sweep matrix from this list and
# the patch check walks it, so a manifest one of them sees the other sees too: a
# second glob, in either, is how a manifest ends up checked and never swept, or the
# other way round. Sorted ordinally, so a matrix built from it is stable run to run and host to host.
#
# Returns full paths. A repository with no `crates` directory has none, and saying
# so is the caller's business. `-IncludeTools` adds the manifests in `tools/`,
# which sabotage the harness itself and need a runner of their own, so the sweep
# planner leaves them out and the patch check, which runs nothing, takes them in.
function Get-SabotageManifest {
    param([string] $RepositoryRoot, [switch] $IncludeTools)
    $cratesDir = Join-Path $RepositoryRoot 'crates'
    [string[]] $found = @()
    if (Test-Path -LiteralPath $cratesDir) {
        $found += @(Get-ChildItem -Path $cratesDir -Directory |
                ForEach-Object { Get-ChildItem -Path $_.FullName -Filter 'sabotage*.json' -File } |
                ForEach-Object { $_.FullName })
    }
    $toolsDir = Join-Path $RepositoryRoot 'tools'
    if ($IncludeTools -and (Test-Path -LiteralPath $toolsDir)) {
        $found += @(Get-ChildItem -Path $toolsDir -Filter 'sabotage*.json' -File | ForEach-Object { $_.FullName })
    }
    if ($found.Count -eq 0) { return }
    # Ordinal, not Sort-Object: that one is culture-sensitive, and the two hosts
    # order 'sabotage-x.json' and 'sabotage.json' differently under it.
    [Array]::Sort($found, [System.StringComparer]::Ordinal)
    $found
}

# Whether a sabotage's patch applies to `Original`, the text of the file it names.
#
# The find text must occur EXACTLY once, not at least once: a pattern matching two
# sites patches whichever the string replace happens to reach, and the sabotage is
# then not the one described. And the patch must change the file, or it sabotages
# nothing. Those are the two ways a manifest goes wrong without anything being
# built, so the sweep asks this before it builds, and the no-build check asks the
# same question of every entry -- this is the one place the rule is written.
#
# `Outcome` is `ok`, `stale` (the count is not one; `Occurrences` says what it is)
# or `inert`. `Patched` is the patched text for `ok` and `$null` otherwise. The
# lines of `Find` and `Replace` are joined with LF, as the manifests are written.
function Test-SabotagePatch {
    param([string] $Original, [string[]] $Find, [string[]] $Replace)
    $findText = ($Find -join "`n")
    $replaceText = ($Replace -join "`n")
    $occurrences = ([regex]::Matches($Original, [regex]::Escape($findText))).Count
    $outcome = 'stale'
    $patched = $null
    if ($occurrences -eq 1) {
        $patched = $Original.Replace($findText, $replaceText)
        # Ordinal: PowerShell's -eq ignores case, so a patch that changed only case
        # would read as one that changed nothing.
        if ([string]::Equals($patched, $Original, [System.StringComparison]::Ordinal)) {
            $outcome = 'inert'
            $patched = $null
        }
        else { $outcome = 'ok' }
    }
    return [pscustomobject]@{
        Outcome     = $outcome
        Occurrences = $occurrences
        FindText    = $findText
        ReplaceText = $replaceText
        Patched     = $patched
    }
}
# --- Which manifests a change could have affected ------------------------------
#
# Every sabotage entry costs a build and a run of its crate's suite, so a change
# is swept by the manifests it could have affected, and a nightly sweep of all of
# them is the backstop (DESIGN-NOTES.md, "sabotage-scope"). These are the pieces:
# what changed, which crates build on which, which paths every sweep shares, and
# the selection. The planner and the harness both use them, so the two cannot
# disagree about what a change reaches.

# Paths every sweep depends on, whichever crate it sweeps, so a change to one
# reaches every manifest: the workspace's manifest, lockfile and toolchain, the
# sabotage tools and the helper they share, the sweep workflow, and
# win-job-launcher, which supervises every phase of every sweep and decides what
# its exit means, with win-handles-sys, the one workspace crate it builds on for
# its handle duplication. A test checks the list against cargo, so a path
# dependency the launcher gains later is not missed. `Path` is relative to the
# repository root; either slash.
function Test-SabotageSharedInput {
    param([string] $Path)
    $normal = $Path.Replace('\', '/')
    foreach ($pattern in @('Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', 'rust-toolchain', '.cargo/*',
            'tools/common.ps1', 'tools/*sabotage*.ps1', '.github/workflows/sabotage-sweep.yml', 'crates/win-job-launcher/*',
            'crates/win-handles-sys/*')) {
        if ($normal -like $pattern) { return $true }
    }
    return $false
}

# The paths in git's output, one per line, as git holds them: a path with a byte
# outside printable ASCII in it, or a quote or backslash, is quoted and escaped in
# C style -- `"crates/x/caf\303\251.rs"` -- and is unquoted here, its octal escapes
# taken as the bytes of UTF-8 they are.
#
# Quoted, deliberately, and not -z. With -z git writes each path's raw bytes, and
# PowerShell decodes a native command's output with the console's code page, which
# on a Windows PowerShell host need not be UTF-8: an e-acute then arrives as two other
# characters, not as a replacement character, and the path silently matches
# nothing. Quoted output is ASCII, which every code page decodes alike, so it
# arrives intact on any host. Callers ask for it with `-c core.quotePath=true`, so
# a user's setting cannot turn it off. A line that still carries a character
# outside ASCII was decoded by a host this cannot vouch for, and so does a quoting
# that does not unquote: `Ok` is then false, and the caller cannot say what
# changed, which sweeps more. Returned as an object rather than a list, so that no
# paths and an answer that cannot be trusted never look alike.
function Split-SabotageGitPaths {
    param($Lines)
    $failed = [pscustomobject]@{ Ok = $false; Paths = [string[]]@() }
    $paths = New-Object 'System.Collections.Generic.List[string]'
    foreach ($line in @($Lines)) {
        if (-not $line) { continue }
        $line = [string]$line
        foreach ($c in $line.ToCharArray()) { if ([int]$c -gt 0x7E -or [int]$c -lt 0x20) { return $failed } }
        if ($line.Length -ge 2 -and $line.StartsWith('"') -and $line.EndsWith('"')) {
            $line = ConvertFrom-GitQuotedPath $line
            if ($null -eq $line) { return $failed }
        }
        $paths.Add($line)
    }
    return [pscustomobject]@{ Ok = $true; Paths = [string[]]$paths.ToArray() }
}

# A path git quoted, as C does, unquoted: `\a \b \t \n \v \f \r \" \\` and three
# octal digits for any other byte, the bytes then read as UTF-8. $null for text
# that is not such a quoting, or bytes that are not UTF-8.
function ConvertFrom-GitQuotedPath {
    param([string] $Quoted)
    if ($Quoted.Length -lt 2 -or $Quoted[0] -ne '"' -or $Quoted[$Quoted.Length - 1] -ne '"') { return $null }
    $body = $Quoted.Substring(1, $Quoted.Length - 2)
    $bytes = New-Object 'System.Collections.Generic.List[byte]'
    $simple = @{ [char]'a' = 7; [char]'b' = 8; [char]'t' = 9; [char]'n' = 10; [char]'v' = 11; [char]'f' = 12; [char]'r' = 13; [char]'"' = 34; [char]'\' = 92 }
    $i = 0
    while ($i -lt $body.Length) {
        $c = $body[$i]
        if ($c -eq '"') { return $null }
        if ($c -ne '\') { $bytes.Add([byte][char]$c); $i++; continue }
        if ($i + 1 -ge $body.Length) { return $null }
        $next = $body[$i + 1]
        if ($simple.ContainsKey($next)) { $bytes.Add([byte]$simple[$next]); $i += 2; continue }
        if ($i + 3 -lt $body.Length + 1 -and $body.Substring($i + 1, [Math]::Min(3, $body.Length - $i - 1)) -match '^[0-3][0-7][0-7]$') {
            $bytes.Add([Convert]::ToByte($body.Substring($i + 1, 3), 8)); $i += 4; continue
        }
        return $null
    }
    $strict = New-Object System.Text.UTF8Encoding($false, $true)
    try { return $strict.GetString($bytes.ToArray()) } catch { return $null }
}

# What differs between commit `From` and the working tree -- committed, staged,
# unstaged, and untracked files git does not ignore -- as paths relative to the
# root, forward slashes, sorted ordinally, each once. $null when git cannot say,
# or reports a path this host could not decode (Split-SabotageGitPaths). Cached per repository and commit,
# since a sweep or a plan asks the same question of many entries recorded at the
# same commit; the working tree can change between measurements, so
# Get-SabotageChange, which begins every one, empties the cache first.
function Get-SabotageWorkingPaths {
    param([string] $RepositoryRoot, [string] $From)
    $key = "$RepositoryRoot|$From"
    if ($script:SabotageWorkingPaths.ContainsKey($key)) { return , $script:SabotageWorkingPaths[$key] }
    $result = $null
    $diffLines = @(Invoke-NativeStdout { git -c core.quotePath=true -C $RepositoryRoot diff --name-only --no-renames $From })
    if ($LASTEXITCODE -eq 0) {
        $untrackedLines = @(Invoke-NativeStdout { git -c core.quotePath=true -C $RepositoryRoot ls-files --others --exclude-standard })
        if ($LASTEXITCODE -eq 0) {
            $diff = Split-SabotageGitPaths $diffLines
            $untracked = Split-SabotageGitPaths $untrackedLines
            if ($diff.Ok -and $untracked.Ok) {
                $result = Join-SabotagePaths @(@($diff.Paths) + @($untracked.Paths))
            }
        }
    }
    $script:SabotageWorkingPaths[$key] = $result
    return , $result
}

# `Paths`, each once, slashes forward, sorted ordinally.
function Join-SabotagePaths {
    param([string[]] $Paths)
    # Ordinal and case-sensitive, like the sort in Get-SabotageManifest: Sort-Object
    # -Unique orders by culture and folds case, so its answer depends on the host.
    $set = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
    foreach ($path in @($Paths)) {
        if (-not $path) { continue }
        [void]$set.Add($path.Replace('\', '/'))
    }
    [string[]] $sorted = @($set)
    [Array]::Sort($sorted, [System.StringComparer]::Ordinal)
    return , $sorted
}

# The text of `Path` at `Commit`, or $null when that commit has no such file. Cached.
function Get-SabotageFileAt {
    param([string] $RepositoryRoot, [string] $Commit, [string] $Path)
    $key = "$RepositoryRoot|$Commit|$Path"
    if (-not $script:SabotageFileAt.ContainsKey($key)) {
        $text = Invoke-NativeStdout { git -C $RepositoryRoot show "${Commit}:$Path" } | Out-String
        if ($LASTEXITCODE -ne 0) { $text = $null }
        $script:SabotageFileAt[$key] = $text
    }
    return $script:SabotageFileAt[$key]
}

# Per-process caches for the three functions above, declared as common.ps1 is
# dot-sourced because strict mode refuses to read a variable that was never set.
$script:SabotageWorkingPaths = @{}
$script:SabotageFileAt = @{}

# Why git's view of the working tree cannot be trusted to show every edit, or
# $null when it can. A tracked file marked assume-unchanged or skip-worktree --
# set by hand, or by a sparse checkout -- is left out of `git diff` and `git
# status` whatever its contents, while a sweep copies and builds what is on disk,
# so an edit there would be swept and never measured. `git ls-files -v` tags such
# a file with a lowercase letter (assume-unchanged) or `S` (skip-worktree). Git
# failing to answer is a reason too.
function Get-SabotageHiddenIndexState {
    param([string] $RepositoryRoot)
    $lines = @(Invoke-NativeStdout { git -c core.quotePath=true -C $RepositoryRoot ls-files -v })
    if ($LASTEXITCODE -ne 0) { return 'git could not list the files it tracks' }
    foreach ($line in $lines) {
        if (-not $line) { continue }
        $tag = [string]$line[0]
        if ($tag -ceq 'S' -or ($tag -cmatch '^[a-z]$')) {
            return "git is told to ignore the working copy of a tracked file: $line"
        }
    }
    return $null
}

# Whether the directory `Tree` holds exactly what commit `Commit` does: every file
# the commit tracks, with the same content as git would store it, and no other
# file git would not ignore. Asked of git itself, through an index of its own
# built from the commit, so that nothing the repository's own index says --
# stat information, assume-unchanged, skip-worktree -- can stand in for reading
# the files. $false when any of it cannot be established.
#
# This is what lets a sweep say which commit it swept: it compares the copy it
# built and is about to sweep, not the checkout at some other moment, so an edit
# made and discarded while it runs cannot make a dirty snapshot look clean.
function Test-SabotageTreeIsCommit {
    param([string] $RepositoryRoot, [string] $Tree, [string] $Commit)
    if (-not $Commit -or -not (Test-Path -LiteralPath $Tree -PathType Container)) { return $false }
    $index = Join-Path ([System.IO.Path]::GetTempPath()) ('sabotage-index-' + [guid]::NewGuid().ToString('N'))
    $saved = $env:GIT_INDEX_FILE
    try {
        $env:GIT_INDEX_FILE = $index
        Invoke-NativeStdout { git -C $RepositoryRoot read-tree $Commit } | Out-Null
        if ($LASTEXITCODE -ne 0) { return $false }
        # Exits non-zero when it finds a file that differs; diff-index decides.
        Invoke-NativeStdout { git -C $RepositoryRoot --work-tree $Tree update-index -q --refresh } | Out-Null
        Invoke-NativeStdout { git -C $RepositoryRoot --work-tree $Tree diff-index --quiet $Commit -- } | Out-Null
        if ($LASTEXITCODE -ne 0) { return $false }
        $extra = @(Invoke-NativeStdout { git -C $RepositoryRoot --work-tree $Tree ls-files --others --exclude-standard } | Where-Object { $_ })
        if ($LASTEXITCODE -ne 0 -or $extra.Count -gt 0) { return $false }
        return $true
    }
    finally {
        if ($null -eq $saved) { Remove-Item -Path Env:GIT_INDEX_FILE -ErrorAction SilentlyContinue }
        else { $env:GIT_INDEX_FILE = $saved }
        Remove-Item -LiteralPath $index -Force -ErrorAction SilentlyContinue
    }
}

# What differs between `Since` and the working tree, measured from the merge base
# of `Since` and HEAD (Get-SabotageWorkingPaths).
#
# When `Since` is not an ancestor of HEAD, the paths `Since` changed after the
# merge base count too. That is a push that dropped commits -- a force-push --
# whose dropped changes are reverted by it, and a branch behind its base, which
# lacks the base's own changes; in both, the tree differs from `Since` in those
# paths, and leaving them out would sweep less than the change could affect.
# Rebasing a branch onto its base keeps a local scope to the branch's own work.
#
# Returns `Known` and `Paths` (relative to the root, forward slashes) rather than
# a bare list, because "nothing changed" and "cannot tell" must never look alike:
# an empty list wrapped in @() is indistinguishable from a $null one, and treating
# "cannot tell" as "nothing" would sweep nothing. It cannot tell for a blank or
# all-zero revision (a push that created its branch) and a revision this clone
# does not have; `Reason` says which.
function Get-SabotageChange {
    param([string] $RepositoryRoot, [string] $Since)
    $script:SabotageWorkingPaths = @{}
    $unknown = { param($why) [pscustomobject]@{ Known = $false; Base = $null; Divergent = $false; Paths = @(); Reason = $why } }
    if ([string]::IsNullOrWhiteSpace($Since)) { return & $unknown 'no base revision was given' }
    if ($Since -match '^0+$') { return & $unknown "the base revision $Since is the all-zero one a new branch pushes" }
    $base = @(Invoke-NativeStdout { git -C $RepositoryRoot merge-base $Since HEAD })
    if ($LASTEXITCODE -ne 0 -or $base.Count -eq 0) {
        return & $unknown "this clone has no merge base with '$Since'"
    }
    $mergeBase = $base[0].Trim()
    $hidden = Get-SabotageHiddenIndexState -RepositoryRoot $RepositoryRoot
    if ($hidden) { return & $unknown $hidden }
    $working = Get-SabotageWorkingPaths -RepositoryRoot $RepositoryRoot -From $mergeBase
    if ($null -eq $working) { return & $unknown "git could not say what differs from $mergeBase" }
    # 0 is an ancestor, 1 is not, and anything else is git unable to say.
    Invoke-NativeStdout { git -C $RepositoryRoot merge-base --is-ancestor $Since HEAD } | Out-Null
    $ancestry = $LASTEXITCODE
    $divergent = $false
    $dropped = @()
    if ($ancestry -eq 1) {
        $divergent = $true
        $droppedLines = @(Invoke-NativeStdout { git -c core.quotePath=true -C $RepositoryRoot diff --name-only --no-renames $mergeBase $Since })
        if ($LASTEXITCODE -ne 0) { return & $unknown "git could not diff $mergeBase against '$Since'" }
        $split = Split-SabotageGitPaths $droppedLines
        if (-not $split.Ok) { return & $unknown 'git reported a path this host could not decode' }
        $dropped = $split.Paths
    }
    elseif ($ancestry -ne 0) { return & $unknown "git could not say whether '$Since' is an ancestor of HEAD" }
    $paths = Join-SabotagePaths @(@($working) + @($dropped))
    $reason = "$($paths.Count) path(s) differ from $mergeBase"
    if ($divergent) { $reason += ", counting what '$Since' changed since then, as it is not an ancestor of HEAD" }
    return [pscustomobject]@{ Known = $true; Base = $mergeBase; Divergent = $divergent; Paths = $paths; Reason = $reason }
}
# Which workspace crates each crate depends on, through path dependencies of every
# kind: a dev-dependency builds the tests that catch a sabotage, so it counts. Keyed
# by the crate's directory relative to the root ('crates/x'), each value the
# directories it depends on directly. From `cargo metadata --no-deps`, which reads
# manifests and builds nothing. $null when cargo cannot answer.
function Get-SabotageCrateDependency {
    param([string] $RepositoryRoot)
    $manifest = Join-Path $RepositoryRoot 'Cargo.toml'
    $json = Invoke-NativeStdout { cargo metadata --format-version 1 --no-deps --manifest-path $manifest } | Out-String
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($json)) { return $null }
    $root = [System.IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
    $relative = {
        param($full)
        $at = [System.IO.Path]::GetFullPath($full)
        if (-not $at.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) { return $null }
        $at.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
    }
    $map = @{}
    foreach ($package in ($json | ConvertFrom-Json).packages) {
        $dir = & $relative (Split-Path -Parent $package.manifest_path)
        if (-not $dir) { continue }
        $map[$dir] = @($package.dependencies |
                Where-Object { ($_.PSObject.Properties.Name -contains 'path') -and $_.path } |
                ForEach-Object { & $relative $_.path } |
                Where-Object { $_ } | Sort-Object -Unique)
    }
    return $map
}

# Whether `Path` lies in the crate at `Crate`, both relative to the root with
# forward slashes. A crate at the root, '', holds every path. Case is ignored: a
# path written in another case names the same file here, and taking it for another
# file would make a change to the crate look like none.
function Test-SabotagePathInCrate {
    param([string] $Path, [string] $Crate)
    if (-not $Crate) { return $true }
    return $Path.StartsWith($Crate + '/', [System.StringComparison]::OrdinalIgnoreCase)
}

# A manifest's crate: the directory it is in, relative to the root, forward
# slashes, and '' for one at the root.
function Get-SabotageManifestCrate {
    param([string] $Manifest)
    $at = $Manifest.LastIndexOf('/')
    if ($at -lt 0) { return '' }
    return $Manifest.Substring(0, $at)
}

# `Path`, full, in the case each of its directories and its file have on disk, so
# a path typed in another case is named as git names it. A part that is not there
# is kept as given.
function Get-SabotageOnDiskPath {
    param([string] $Path)
    $full = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($full)
    $at = $root
    foreach ($part in $full.Substring($root.Length).Split([char[]]@('\', '/'), [System.StringSplitOptions]::RemoveEmptyEntries)) {
        $found = $null
        if ([System.IO.Directory]::Exists($at)) {
            $found = @((New-Object System.IO.DirectoryInfo $at).GetFileSystemInfos($part)) | Select-Object -First 1
        }
        $name = if ($null -ne $found) { $found.Name } else { $part }
        $at = [System.IO.Path]::Combine($at, $name)
    }
    return $at
}
# Which workspace crate each package is, by name: the package's directory relative
# to the root, forward slashes, '' for one at the root. From `cargo metadata
# --no-deps`, which builds nothing. $null when cargo cannot answer.
function Get-SabotageCratePackage {
    param([string] $RepositoryRoot)
    $manifest = Join-Path $RepositoryRoot 'Cargo.toml'
    $json = Invoke-NativeStdout { cargo metadata --format-version 1 --no-deps --manifest-path $manifest } | Out-String
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($json)) { return $null }
    $root = [System.IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
    $map = @{}
    foreach ($package in ($json | ConvertFrom-Json).packages) {
        $dir = [System.IO.Path]::GetFullPath((Split-Path -Parent $package.manifest_path))
        if (-not $dir.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) { continue }
        $map[$package.name] = $dir.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
    }
    return $map
}

# The packages a manifest's sweep runs the tests of: its `package`, and every
# package its `testArgs` names with `-p`/`--package` before the `--` that ends
# cargo's own arguments. `Workspace` is set by `--workspace` or `--all`, which
# test every crate, and by `testArgs` that name no package at all: the harness
# runs `testArgs` in place of `test -p <package>`, from the workspace root, so
# with no package named cargo tests the workspace's default members, which here
# are all of them. `Unknown` says why the answer cannot be trusted -- a package
# given as a pattern, which cargo matches against names this does not know --
# and is $null otherwise.
function Get-SabotageTestedPackage {
    param($Spec)
    $names = New-Object 'System.Collections.Generic.List[string]'
    $workspace = $false
    $unknown = $null
    if ($Spec.PSObject.Properties.Name -contains 'package' -and $Spec.package) { $names.Add([string]$Spec.package) }
    $arguments = @()
    if ($Spec.PSObject.Properties.Name -contains 'testArgs' -and $Spec.testArgs) { $arguments = @($Spec.testArgs) }
    for ($i = 0; $i -lt $arguments.Count; $i++) {
        $argument = [string]$arguments[$i]
        if ($argument -eq '--') { break }
        $value = $null
        if ($argument -eq '-p' -or $argument -eq '--package') {
            if ($i + 1 -lt $arguments.Count) { $value = [string]$arguments[$i + 1]; $i++ }
        }
        elseif ($argument.StartsWith('--package=', [System.StringComparison]::Ordinal)) { $value = $argument.Substring(10) }
        elseif ($argument.StartsWith('-p', [System.StringComparison]::Ordinal) -and $argument.Length -gt 2 -and $argument[2] -ne '-') { $value = $argument.Substring(2) }
        elseif ($argument -eq '--workspace' -or $argument -eq '--all') { $workspace = $true }
        if ($null -eq $value) { continue }
        # A package id may carry a version, `name@1.2.3`; the name is what is matched.
        $name = ($value -split '@')[0]
        if ($name -match '[*?\[\]/:]' -or -not $name) {
            $unknown = "it tests packages given as '$value', which is not a package name"
            continue
        }
        $names.Add($name)
    }
    $named = $names.Count
    if ($Spec.PSObject.Properties.Name -contains 'package' -and $Spec.package) { $named-- }
    if ($arguments.Count -gt 0 -and $named -eq 0 -and -not $unknown) { $workspace = $true }
    return [pscustomobject]@{ Names = [string[]]@($names | Select-Object -Unique); Workspace = $workspace; Unknown = $unknown }
}

# `Crate` and every crate it builds on, directly or not, through
# Get-SabotageCrateDependency's map. A crate the map does not know reaches only
# itself, and a cycle is walked once.
function Get-SabotageCrateReach {
    param([string] $Crate, [hashtable] $Dependencies)
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
    $pending = New-Object 'System.Collections.Generic.Queue[string]'
    $pending.Enqueue($Crate)
    while ($pending.Count -gt 0) {
        $at = $pending.Dequeue()
        if (-not $seen.Add($at)) { continue }
        if ($Dependencies -and $Dependencies.ContainsKey($at)) {
            foreach ($next in $Dependencies[$at]) { $pending.Enqueue($next) }
        }
    }
    $seen
}

# The directory a manifest's entries name their `file` relative to: the
# manifest's own, or its `root`, relative to it, when it gives one. Not checked
# for existence; the harness reports a `root` that is not there.
function Get-SabotageSourceRoot {
    param([string] $ManifestPath, $Spec)
    $manifestDir = Split-Path -Parent $ManifestPath
    if ($null -ne $Spec -and ($Spec.PSObject.Properties.Name -contains 'root') -and $Spec.root) {
        return (Join-Path $manifestDir $Spec.root)
    }
    return $manifestDir
}

# --- Which tests caught each entry ---------------------------------------------
#
# A sweep records, per entry it judged, the tests that failed under its patch,
# so that a later sweep can tell which test changes could alter that entry's
# result. It is derived data, not committed: a sweep writes it beside its
# transcripts and CI keeps it in the Actions cache. Its absence only makes a
# scoped sweep larger; see DESIGN-NOTES.md, "sabotage-scope".
#
# The shape, written by Write-SabotageCatcherRecord:
#
#   { "version": 1, "manifests": { "crates/x/sabotage.json": {
#       "<entry name>": { "tests": ["module::test", ...], "at": "<UTC, round-trip format>",
#                         "commit": "<HEAD of the tree it was run in>" } } } }
#
# `at` is when the entry was run. Records from several shards are merged by
# keeping each entry's latest, and the round-trip format orders as text in the
# order it orders as time. `commit` is what makes an entry's record usable: it
# says which tests caught the entry in that tree, and a checkout that does not
# contain that commit -- an older run restoring a record a newer one saved, or a
# history the record was not learned on -- cannot tell from its own change what
# has happened to those tests since. Get-SabotageCatcherTests is the one place
# that decides.

# The names of the tests libtest reported FAILED in `Text`, in the order they
# first appear, each once. A doctest's name is its file and item, `src\lib.rs -
# Item (line 12)`; a `#[should_panic]` test's ` - should panic` suffix is dropped
# so the name is the function's.
function Get-FailedTestName {
    param([AllowNull()][string] $Text)
    if (-not $Text) { return }
    $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
    foreach ($match in [regex]::Matches($Text, '(?m)^test (.+?) \.\.\. FAILED[ \t]*\r?$')) {
        $name = $match.Groups[1].Value -replace ' - should panic$', ''
        if ($seen.Add($name)) { $name }
    }
}

# An empty record.
function New-SabotageCatcherRecord {
    return @{}
}

# The record at `Path` as nested hashtables, manifest to entry to
# @{ Tests; At }. A file that is missing, unreadable, or not a record of this
# version reads as empty: the record only ever narrows a sweep, so losing it
# costs time and nothing else.
function Read-SabotageCatcherRecord {
    param([string] $Path)
    $record = New-SabotageCatcherRecord
    if (-not $Path -or -not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $record }
    try { $parsed = [System.IO.File]::ReadAllText($Path) | ConvertFrom-Json }
    catch { return $record }
    if ($null -eq $parsed -or -not ($parsed.PSObject.Properties.Name -contains 'version') -or $parsed.version -ne 1 -or
        -not ($parsed.PSObject.Properties.Name -contains 'manifests') -or $null -eq $parsed.manifests) {
        return $record
    }
    foreach ($manifest in $parsed.manifests.PSObject.Properties) {
        $entries = @{}
        foreach ($entry in $manifest.Value.PSObject.Properties) {
            $value = $entry.Value
            if ($null -eq $value -or -not ($value.PSObject.Properties.Name -contains 'at')) { continue }
            $tests = @()
            if ($value.PSObject.Properties.Name -contains 'tests' -and $null -ne $value.tests) { $tests = @($value.tests) }
            # ConvertFrom-Json turns an ISO 8601 string into a DateTime on its own, so a
            # parsed date is turned back into the round-trip text the merge compares.
            $at = $value.at
            if ($at -is [datetime]) { $at = $at.ToUniversalTime().ToString('o') }
            $commit = $null
            if ($value.PSObject.Properties.Name -contains 'commit') { $commit = [string]$value.commit }
            $entries[$entry.Name] = @{ Tests = [string[]]$tests; At = [string]$at; Commit = $commit }
        }
        $record[$manifest.Name] = $entries
    }
    return $record
}

# Set what caught `Entry` of `Manifest` (a path relative to the root, forward
# slashes), as of `At`, in a tree at `Commit`.
function Set-SabotageCatcher {
    param([hashtable] $Record, [string] $Manifest, [string] $Entry, [string[]] $Tests, [string] $At, [string] $Commit)
    if (-not $Record.ContainsKey($Manifest)) { $Record[$Manifest] = @{} }
    $Record[$Manifest][$Entry] = @{ Tests = [string[]]@($Tests | Where-Object { $_ }); At = $At; Commit = $Commit }
}

# Whether each recorded commit is an ancestor of HEAD, asked of git once per
# repository and commit. Declared here, as common.ps1 is dot-sourced, because
# strict mode refuses to read a variable that was never set.
$script:SabotageAncestry = @{}

# The tests recorded as catching `Entry` of `Manifest`, if the record of it can be
# used in the checkout at `RepositoryRoot`: it names a commit, and that commit is
# HEAD or an ancestor of it. Returns `Usable` and `Tests`; an entry with no usable
# record has no tests, which every caller reads as "nothing is known", so a record
# from a commit this checkout does not contain can only make a sweep run more.
function Get-SabotageCatcherTests {
    param([string] $RepositoryRoot, [hashtable] $Record, [string] $Manifest, [string] $Entry)
    $none = [pscustomobject]@{ Usable = $false; Tests = [string[]]@(); Commit = $null }
    if ($null -eq $Record -or -not $Record.ContainsKey($Manifest) -or -not $Record[$Manifest].ContainsKey($Entry)) { return $none }
    $value = $Record[$Manifest][$Entry]
    if (-not $value.Commit) { return $none }
    $key = "$RepositoryRoot|$($value.Commit)"
    if (-not $script:SabotageAncestry.ContainsKey($key)) {
        Invoke-NativeStdout { git -C $RepositoryRoot merge-base --is-ancestor $value.Commit HEAD } | Out-Null
        $script:SabotageAncestry[$key] = ($LASTEXITCODE -eq 0)
    }
    if (-not $script:SabotageAncestry[$key]) { return $none }
    return [pscustomobject]@{ Usable = $true; Tests = [string[]]@($value.Tests); Commit = [string]$value.Commit }
}

# One record holding, for every entry any of `Records` has, the latest of them.
function Merge-SabotageCatcherRecord {
    param([hashtable[]] $Records)
    $merged = New-SabotageCatcherRecord
    foreach ($record in $Records) {
        if ($null -eq $record) { continue }
        foreach ($manifest in $record.Keys) {
            foreach ($entry in $record[$manifest].Keys) {
                $value = $record[$manifest][$entry]
                $held = $null
                if ($merged.ContainsKey($manifest) -and $merged[$manifest].ContainsKey($entry)) { $held = $merged[$manifest][$entry] }
                if ($null -eq $held -or [string]::CompareOrdinal($value.At, $held.At) -gt 0) {
                    Set-SabotageCatcher -Record $merged -Manifest $manifest -Entry $entry -Tests $value.Tests -At $value.At -Commit $value.Commit
                }
            }
        }
    }
    return $merged
}

# Write `Record` to `Path` as JSON, keys in ordinal order so that two writes of
# the same record are the same bytes.
function Write-SabotageCatcherRecord {
    param([string] $Path, [hashtable] $Record)
    $sortedKeys = {
        param($keys)
        [string[]] $list = @($keys)
        [Array]::Sort($list, [System.StringComparer]::Ordinal)
        , $list
    }
    $manifests = [ordered]@{}
    foreach ($manifest in (& $sortedKeys $Record.Keys)) {
        $entries = [ordered]@{}
        foreach ($entry in (& $sortedKeys $Record[$manifest].Keys)) {
            $value = $Record[$manifest][$entry]
            $entries[$entry] = [ordered]@{ tests = @($value.Tests); at = $value.At; commit = $value.Commit }
        }
        $manifests[$manifest] = $entries
    }
    $json = [ordered]@{ version = 1; manifests = $manifests } | ConvertTo-Json -Depth 6
    $directory = Split-Path -Parent $Path
    if ($directory -and -not (Test-Path -LiteralPath $directory)) { New-Item -ItemType Directory -Force -Path $directory | Out-Null }
    [System.IO.File]::WriteAllText($Path, $json + "`n", [System.Text.UTF8Encoding]::new($false))
}

# Which entries of one manifest a change could have affected, and why each.
#
# `Manifest` is the manifest's path from the root, forward slashes; `Entries` its
# entries as parsed; `SourceRoot` the directory their `file` is relative to; `Spec`
# the manifest as parsed. `Change` is Get-SabotageChange's result, `Dependencies`
# Get-SabotageCrateDependency's map, `Packages` Get-SabotageCratePackage's, and
# `Record` the record of which tests caught each entry. The manifest's crate is its
# directory.
#
# Every entry is affected when the change cannot be measured, a map is missing, or
# the manifest's packages cannot be resolved. Otherwise each entry is judged
# against two changes, and is affected if either reaches it: the one given, from
# its base, and -- for an entry whose usable record was learned at another
# commit -- everything since that commit. The second is what keeps a record honest
# when the run that should have refreshed it never saved one: a sweep cancelled,
# failed, or overlapping another. Against a change from a commit X:
#   - every entry, when it touched an input every sweep shares; a crate the
#     manifest's crate builds on, or another package it tests (`Spec`'s `package`
#     and `testArgs`, Get-SabotageTestedPackage), or one of those builds on; the
#     crate's own build settings (its Cargo.toml, build.rs, .cargo/), which can
#     switch tests off without touching one; or the manifest's own settings;
#   - no entry, when it touched nothing in the crate or under the manifest's
#     `root`;
#   - otherwise the entry, when it is new or differs from the manifest at X; the
#     file it patches changed; its record names no test catching it, or a
#     doctest caught it, whose source may be a file spliced into the crate's
#     documentation rather than the one it names; or a file defining a function
#     named as one of its catching tests changed, or defined one at X, or no file
#     of the crate defines one any more.
# An entry with no usable record (Get-SabotageCatcherTests) is affected whatever
# the change: there is no commit it was last swept at to measure from.
# A change elsewhere in the crate selects nothing on its own. That, and a test
# helper changing under the tests that call it, are what the nightly sweep is for
# (DESIGN-NOTES.md, "sabotage-scope").
#
# Returns `All`, the affected `Entries` in the order given, `Reasons` (each
# affected entry's name to why), and a `Reason` for the whole.
function Select-SabotageEntry {
    param([string] $RepositoryRoot, [string] $Manifest, [object[]] $Entries, [string] $SourceRoot,
        $Change, [hashtable] $Dependencies, [hashtable] $Record, $Spec, [hashtable] $Packages)
    $Entries = @($Entries)
    $every = {
        param($why)
        $reasons = @{}
        foreach ($entry in $Entries) { $reasons[$entry.name] = $why }
        [pscustomobject]@{ All = $true; Entries = $Entries; Reasons = $reasons; Reason = "every entry: $why" }
    }
    if ($null -eq $Change -or -not $Change.Known) {
        $why = if ($null -eq $Change) { 'no change was measured' } else { $Change.Reason }
        return & $every $why
    }
    if ($null -eq $Dependencies) { return & $every 'cargo metadata could not say which crates build on which' }
    if ($null -eq $Spec) { return & $every 'the manifest''s settings were not given' }

    # The crates whose tests the sweep runs: the manifest's own, and every package
    # its settings name, which may be one that builds on it -- a driver whose tests
    # are the ones that catch a sabotage of the core it is built on.
    $tested = Get-SabotageTestedPackage -Spec $Spec
    if ($tested.Unknown) { return & $every $tested.Unknown }
    $crate = Get-SabotageManifestCrate $Manifest
    $roots = New-Object 'System.Collections.Generic.List[string]'
    $roots.Add($crate)
    if ($tested.Workspace) { foreach ($dir in $Dependencies.Keys) { $roots.Add($dir) } }
    foreach ($name in $tested.Names) {
        if ($null -eq $Packages -or -not $Packages.ContainsKey($name)) {
            return & $every "it tests package '$name', which cargo does not list"
        }
        $roots.Add($Packages[$name])
    }
    $reach = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::Ordinal)
    foreach ($dir in $roots) { foreach ($at in @(Get-SabotageCrateReach -Crate $dir -Dependencies $Dependencies)) { [void]$reach.Add($at) } }

    $root = [System.IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
    $relative = {
        param($full)
        $at = [System.IO.Path]::GetFullPath($full)
        if ($at.StartsWith($root, [System.StringComparison]::OrdinalIgnoreCase)) {
            return $at.Substring($root.Length).TrimStart('\', '/').Replace('\', '/')
        }
        return $at.Replace('\', '/')
    }
    # A manifest whose `root` points outside its crate patches files there, so a
    # change under that root reaches it as a change in its own crate does.
    $sourceDir = & $relative $SourceRoot
    if ($sourceDir -eq $root.Replace('\', '/')) { $sourceDir = '' }
    # What is worked out once and read by the blocks below. A hashtable, because a
    # block run with & has a scope of its own, and a variable assigned there would
    # be gone when it returned; a table it was given is the same table.
    $memo = @{ Settings = $null; Definitions = $null; Roots = $null; RootDirs = @(); Redirected = $false }

    # The manifest's own settings, which decide how every entry is patched and
    # judged: everything but `sabotages`.
    $settingsOf = {
        param($spec)
        $kept = [ordered]@{}
        foreach ($name in @($spec.PSObject.Properties.Name | Where-Object { $_ -ne 'sabotages' } | Sort-Object)) {
            $kept[$name] = $spec.$name
        }
        $kept | ConvertTo-Json -Depth 8 -Compress
    }
    # The manifest at commit X, as entry name to its JSON, or $null when X has no
    # such manifest or it does not parse. Cached per commit.
    $specsAt = @{}
    $specAt = {
        param($commit)
        if (-not $specsAt.ContainsKey($commit)) {
            $parsed = $null
            $text = Get-SabotageFileAt -RepositoryRoot $RepositoryRoot -Commit $commit -Path $Manifest
            if ($null -ne $text) { try { $parsed = $text | ConvertFrom-Json } catch { $parsed = $null } }
            $specsAt[$commit] = $parsed
        }
        $specsAt[$commit]
    }

    # What a change, from commit X over the paths `Changed` (a HashSet), does to the
    # manifest as a whole: `Every` is why every entry is reached, or $null, and
    # `Reached` whether the crate, or the manifest's `root`, is touched at all.
    $judgeManifest = {
        param([string] $commit, $changed)
        $inDir = { param($dir) foreach ($path in $changed) { if (Test-SabotagePathInCrate $path $dir) { return $true } }; $false }
        foreach ($path in $changed) {
            if (Test-SabotageSharedInput $path) { return [pscustomobject]@{ Every = "a shared input changed: $path"; Reached = $true } }
        }
        foreach ($below in $reach) {
            if ($below -ne $crate -and (& $inDir $below)) {
                return [pscustomobject]@{ Every = "a crate it builds on or tests changed: $below"; Reached = $true }
            }
        }
        if ($crate) {
            foreach ($path in $changed) {
                if ($path -eq "$crate/Cargo.toml" -or $path -eq "$crate/build.rs" -or $path.StartsWith("$crate/.cargo/", [System.StringComparison]::OrdinalIgnoreCase)) {
                    return [pscustomobject]@{ Every = "the crate's build settings changed: $path"; Reached = $true }
                }
            }
        }
        if ($changed.Contains($Manifest)) {
            $old = & $specAt $commit
            if ($null -ne $old) {
                if ($null -eq $memo.Settings) {
                    $memo.Settings = & $settingsOf ([System.IO.File]::ReadAllText((Join-Path $RepositoryRoot $Manifest)) | ConvertFrom-Json)
                }
                if (-not [string]::Equals((& $settingsOf $old), $memo.Settings, [System.StringComparison]::Ordinal)) {
                    return [pscustomobject]@{ Every = 'the manifest''s own settings changed'; Reached = $true }
                }
            }
        }
        [pscustomobject]@{ Every = $null; Reached = ((& $inDir $crate) -or (& $inDir $sourceDir)) }
    }

    # Every function the crate defines, by name, to the files defining it: test
    # names are function names, and the same name is often defined in several
    # files, so a test is looked for in all of them. Built once, on first need, into
    # $memo.Definitions. Alongside it, into $memo, the crate's target roots -- the
    # files cargo finds by convention, and any `path` naming a .rs file in its
    # Cargo.toml, which can only be a target's -- the directories they are in, and
    # whether any file names its module's file with `#[path]`.
    $define = {
        $map = @{}
        $roots = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
        $crateDir = if ($crate) { Join-Path $RepositoryRoot $crate } else { $RepositoryRoot }
        foreach ($file in Get-ChildItem -LiteralPath $crateDir -Recurse -Filter '*.rs' -File) {
            $rel = & $relative $file.FullName
            $text = [System.IO.File]::ReadAllText($file.FullName)
            foreach ($match in [regex]::Matches($text, '\bfn\s+(\w+)')) {
                $name = $match.Groups[1].Value
                if (-not $map.ContainsKey($name)) {
                    $map[$name] = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
                }
                [void]$map[$name].Add($rel)
            }
            if ($text -match '#\s*\[\s*path\s*=') { $memo.Redirected = $true }
            $inCrate = if ($crate) { $rel.Substring($crate.Length + 1) } else { $rel }
            if ($inCrate -match '^(src/(lib|main)|src/bin/[^/]+(/main)?|(tests|examples|benches)/[^/]+(/main)?)\.rs$') { [void]$roots.Add($rel) }
        }
        $cargo = Join-Path $crateDir 'Cargo.toml'
        if (Test-Path -LiteralPath $cargo -PathType Leaf) {
            foreach ($match in [regex]::Matches([System.IO.File]::ReadAllText($cargo), '\bpath\s*=\s*"([^"]+\.rs)"')) {
                [void]$roots.Add((& $relative (Join-Path $crateDir $match.Groups[1].Value)))
            }
        }
        $memo.Roots = $roots
        $memo.RootDirs = @($roots | ForEach-Object { $at = $_.LastIndexOf('/'); if ($at -lt 0) { '' } else { $_.Substring(0, $at) } } | Select-Object -Unique)
        $map
    }
    # The functions the crate's changed files defined at commit X: a recorded test
    # deleted from one of them may share its name with a function elsewhere, which
    # would make it look present still. Cached per commit.
    $definedAt = @{}
    $defineAt = {
        param([string] $commit, $changed)
        if (-not $definedAt.ContainsKey($commit)) {
            $map = @{}
            foreach ($path in $changed) {
                if (-not $path.EndsWith('.rs') -or -not (Test-SabotagePathInCrate $path $crate)) { continue }
                $text = Get-SabotageFileAt -RepositoryRoot $RepositoryRoot -Commit $commit -Path $path
                if ($null -eq $text) { continue }
                foreach ($match in [regex]::Matches($text, '\bfn\s+(\w+)')) {
                    $name = $match.Groups[1].Value
                    if (-not $map.ContainsKey($name)) {
                        $map[$name] = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
                    }
                    [void]$map[$name].Add($path)
                }
            }
            $definedAt[$commit] = $map
        }
        $definedAt[$commit]
    }

    # The modules the file at `Path` declares, `mod name;` or `mod name {`: in the
    # working tree when `Commit` is empty, or at that commit. Cached.
    $declared = @{}
    $declares = {
        param([string] $commit, [string] $path)
        $key = "$commit|$path"
        if (-not $declared.ContainsKey($key)) {
            $text = $null
            if ($commit) { $text = Get-SabotageFileAt -RepositoryRoot $RepositoryRoot -Commit $commit -Path $path }
            else {
                $full = Join-Path $RepositoryRoot $path
                if (Test-Path -LiteralPath $full -PathType Leaf) { $text = [System.IO.File]::ReadAllText($full) }
            }
            $names = @()
            if ($null -ne $text) { $names = @([regex]::Matches($text, '\bmod\s+(\w+)\s*[;{]') | ForEach-Object { $_.Groups[1].Value }) }
            $declared[$key] = [string[]]$names
        }
        $declared[$key]
    }
    # The changed file on the module path of a recorded test `a::b::name`, or
    # $null. A test leaves the build, its own file unchanged, when a file declaring
    # a module it is in drops or gates that declaration: the target root declaring
    # `a`, or the file of `a`, or of `a::b`, beside a root, as `a.rs` or `a/mod.rs`.
    # In a crate where any file names its module's file with `#[path]`, a file's
    # name says nothing of its module, so any changed file declaring a module of the
    # path, now or at commit X, counts.
    $onPath = {
        param([string] $commit, $changed, [string] $test)
        $parts = @($test -split '::')
        $modules = @($parts | Select-Object -First ($parts.Count - 1))
        if ($modules.Count -eq 0) { return $null }
        foreach ($path in $changed) {
            if (-not $path.EndsWith('.rs', [System.StringComparison]::OrdinalIgnoreCase) -or -not (Test-SabotagePathInCrate $path $crate)) { continue }
            $declaring = @()
            if ($memo.Redirected) { $declaring = $modules }
            elseif ($memo.Roots.Contains($path)) { $declaring = @($modules[0]) }
            foreach ($module in $declaring) {
                if ((@(& $declares '' $path) -contains $module) -or (@(& $declares $commit $path) -contains $module)) { return $path }
            }
            $stem = $path.Substring(0, $path.Length - 3)
            if ($stem.EndsWith('/mod', [System.StringComparison]::OrdinalIgnoreCase)) { $stem = $stem.Substring(0, $stem.Length - 4) }
            foreach ($dir in $memo.RootDirs) {
                $prefix = if ($dir) { "$dir/" } else { '' }
                for ($depth = 1; $depth -le $modules.Count; $depth++) {
                    $candidate = $prefix + (@($modules | Select-Object -First $depth) -join '/')
                    if ([string]::Equals($stem, $candidate, [System.StringComparison]::OrdinalIgnoreCase)) { return $path }
                }
            }
        }
        $null
    }

    # Why a change from commit X over `Changed` reaches one entry, in a crate it
    # touched, or $null. `Tests` are the entry's usable recorded catching tests.
    $judgeEntry = {
        param([string] $commit, $changed, $entry, [string[]] $tests)
        if ($changed.Contains($Manifest)) {
            $old = & $specAt $commit
            $before = $null
            if ($null -ne $old) { $before = @($old.sabotages | Where-Object { $_.name -eq $entry.name }) | Select-Object -First 1 }
            if ($null -eq $before) { return 'the entry is new' }
            $then = $before | ConvertTo-Json -Depth 8 -Compress
            $now = $entry | ConvertTo-Json -Depth 8 -Compress
            if (-not [string]::Equals($then, $now, [System.StringComparison]::Ordinal)) { return 'the entry changed' }
        }
        if ($changed.Contains((& $relative (Join-Path $SourceRoot $entry.file)))) {
            return "the file it patches changed: $($entry.file)"
        }
        $tests = @($tests)
        if ($tests.Count -eq 0) { return 'no test is recorded as catching it' }
        if (@($tests | Where-Object { $_ -match ' \(line \d+\)' }).Count -gt 0) { return 'a doctest caught it' }
        if ($null -eq $memo.Definitions) { $memo.Definitions = & $define }
        $definitions = $memo.Definitions
        $before = & $defineAt $commit $changed
        foreach ($test in $tests) {
            $function = ($test -split '::')[-1]
            if (-not $definitions.ContainsKey($function)) { return "its catching test is gone: $test" }
            $moved = @($definitions[$function] | Where-Object { $changed.Contains($_) })
            if ($before.ContainsKey($function)) { $moved += @($before[$function]) }
            if ($moved.Count -gt 0) { return "its catching test's file changed: $($moved[0])" }
            $declaring = & $onPath $commit $changed $test
            if ($declaring) { return "a module its catching test is in changed: $declaring" }
        }
        return $null
    }

    # Paths are matched ignoring case throughout: a manifest, `root` or `file`
    # written in another case than git's names the same file here, and a match it
    # missed would make a change look like none.
    $changedNow = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($path in @($Change.Paths)) { if ($path) { [void]$changedNow.Add($path) } }
    $now = & $judgeManifest $Change.Base $changedNow
    if ($now.Every) { return & $every $now.Every }

    # The change since each other commit an entry was recorded at, and what it
    # does to the manifest, worked out once per commit.
    $since = @{}
    $sinceRecord = {
        param([string] $commit)
        if (-not $since.ContainsKey($commit)) {
            $paths = Get-SabotageWorkingPaths -RepositoryRoot $RepositoryRoot -From $commit
            $value = $null
            if ($null -ne $paths) {
                $set = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
                foreach ($path in $paths) { [void]$set.Add($path) }
                $value = [pscustomobject]@{ Changed = $set; Manifest = (& $judgeManifest $commit $set) }
            }
            $since[$commit] = $value
        }
        $since[$commit]
    }

    $reasons = @{}
    $affected = @(foreach ($entry in $Entries) {
            $known = Get-SabotageCatcherTests -RepositoryRoot $RepositoryRoot -Record $Record -Manifest $Manifest -Entry $entry.name
            $why = $null
            # With no usable record there is no commit to measure "since it was last
            # swept" from, and a change made between that sweep and this base --
            # its run cancelled, failed, or its record lost from the cache -- would
            # be invisible. So the entry runs. A lost record costs one wider run,
            # which saves a record; it never costs a smaller one.
            if ($now.Reached) { $why = & $judgeEntry $Change.Base $changedNow $entry $known.Tests }
            if (-not $known.Usable -and (-not $why -or $why -eq 'no test is recorded as catching it')) {
                $why = 'no usable record of when it was last swept'
            }
            if (-not $why -and $known.Usable -and -not [string]::Equals($known.Commit, $Change.Base, [System.StringComparison]::OrdinalIgnoreCase)) {
                $short = $known.Commit.Substring(0, [Math]::Min(8, $known.Commit.Length))
                $then = & $sinceRecord $known.Commit
                if ($null -eq $then) { $why = "git could not say what changed since it was last swept, at $short" }
                elseif ($then.Manifest.Every) { $why = "since it was last swept, at ${short}: $($then.Manifest.Every)" }
                elseif ($then.Manifest.Reached) {
                    $was = & $judgeEntry $known.Commit $then.Changed $entry $known.Tests
                    if ($was) { $why = "since it was last swept, at ${short}: $was" }
                }
            }
            if ($why) {
                $reasons[$entry.name] = $why
                $entry
            }
        })
    $reason = "$($affected.Count) of $($Entries.Count) entries; "
    if ($now.Reached) { $reason += 'its crate changed' } else { $reason += 'nothing it builds on changed' }
    if ($affected.Count -eq 0 -and -not $now.Reached) { $reason = 'nothing it builds on changed' }
    return [pscustomobject]@{ All = $false; Entries = $affected; Reasons = $reasons; Reason = $reason }
}
# Whether a run of `CargoArgs` can be narrowed by naming tests: not when they
# already name some. libtest runs a test that matches ANY name it is given, so a
# name added to a filter the manifest set would widen the run past the suite it
# stands for, to a test that suite does not run, and a failure there would be
# credited as a catch. A filter is anything but a flag after cargo's `--`, or,
# before it, a word that is neither the `test` subcommand nor the value of an
# option below: the TESTNAME `cargo test` passes on to libtest. Conservative: an
# option's value written as a separate word after `--` (`--test-threads 1`), or
# after an option not listed, counts too, and only costs narrowing.
function Test-SabotageNarrowable {
    param([string[]] $CargoArgs)
    $arguments = @($CargoArgs)
    $separator = [array]::IndexOf($arguments, '--')
    $end = if ($separator -lt 0) { $arguments.Count } else { $separator }
    $valued = @('-p', '--package', '--exclude', '-F', '--features', '--target', '--target-dir', '--manifest-path',
        '--lockfile-path', '--test', '--bin', '--example', '--bench', '-j', '--jobs', '--profile', '--config', '-Z',
        '--color', '--message-format')
    $start = if ($end -gt 0 -and [string]::Equals([string]$arguments[0], 'test', [System.StringComparison]::Ordinal)) { 1 } else { 0 }
    for ($i = $start; $i -lt $end; $i++) {
        $argument = [string]$arguments[$i]
        if (-not $argument.StartsWith('-')) { return $false }
        if ($valued -ccontains $argument) { $i++ }
    }
    if ($separator -lt 0) { return $true }
    foreach ($argument in @($arguments | Select-Object -Skip ($separator + 1))) {
        if (-not ([string]$argument).StartsWith('-')) { return $false }
    }
    return $true
}

# `CargoArgs` narrowed to the tests named, each matched exactly: the names go
# after cargo's `--`, where libtest reads them, and every test binary is given
# them, so a binary holding none runs nothing and passes. The sweep harness's
# run of an entry's recorded catching tests (run-sabotage.ps1, Invoke-Sabotaged). `--exact` is added only
# if the arguments after `--` do not carry it already: libtest refuses it twice,
# before running anything.
function Add-TestFilter {
    param([string[]] $CargoArgs, [string[]] $Tests)
    $separator = [array]::IndexOf($CargoArgs, '--')
    if ($separator -lt 0) { return @($CargoArgs) + @('--') + @($Tests) + @('--exact') }
    $tail = @($CargoArgs | Select-Object -Skip ($separator + 1))
    $extra = @($Tests)
    if ($tail -notcontains '--exact') { $extra += '--exact' }
    return @($CargoArgs) + $extra
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

# One argument, quoted for a command line the way the MSVC runtime -- and
# Rust's std, which the launcher parses with -- splits it back apart.
# Start-Process passes -ArgumentList through as text, so without this a path
# containing a space arrives as two arguments.
function ConvertTo-NativeArgument {
    param([AllowEmptyString()][string] $Value)
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') { return $Value }
    $text = New-Object System.Text.StringBuilder
    [void]$text.Append('"')
    $backslashes = 0
    foreach ($c in $Value.ToCharArray()) {
        if ($c -eq [char]'\') { $backslashes++; continue }
        # Backslashes are literal except before a quote, where each must be
        # doubled and the quote itself escaped.
        if ($c -eq [char]'"') { [void]$text.Append([char]'\', 2 * $backslashes + 1) }
        elseif ($backslashes -gt 0) { [void]$text.Append([char]'\', $backslashes) }
        [void]$text.Append($c)
        $backslashes = 0
    }
    # Trailing backslashes precede the closing quote, so they double too.
    if ($backslashes -gt 0) { [void]$text.Append([char]'\', 2 * $backslashes) }
    [void]$text.Append('"')
    return $text.ToString()
}
