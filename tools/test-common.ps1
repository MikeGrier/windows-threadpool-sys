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

# --- Invoke-NativeStdout: the same guard, but for output that is PARSED -------
#
# The distinction is not cosmetic and these cases exist because getting it wrong
# shipped: routing `cargo metadata` through the MERGING helper passed locally and
# failed in CI, where a cold runner emits rustup's `info:` line on stderr and
# `ConvertFrom-Json` then choked on the `i`.

Test-Case 'stdout-only capture discards stderr instead of merging it' {
    $out = Invoke-NativeStdout { cmd /c "echo noise 1>&2 & echo {`"ok`":true}" }
    $text = ("$out").Trim()
    Assert-Equal '{"ok":true}' $text 'only stdout may be captured'
    if ($text -match 'noise') { throw "stderr leaked into a parsed capture: $text" }
}

Test-Case 'stdout-only capture still yields parseable JSON under stderr noise' {
    # The exact shape that failed in CI, reduced.
    $out = Invoke-NativeStdout { cmd /c "echo info: syncing channel updates 1>&2 & echo {`"ok`":true}" }
    $parsed = $out | ConvertFrom-Json
    Assert-Equal $true $parsed.ok 'the JSON must survive stderr noise'
}

Test-Case 'stdout-only capture preserves the exit code' {
    $null = Invoke-NativeStdout { cmd /c "echo boom 1>&2 & exit 4" }
    Assert-Equal 4 $LASTEXITCODE 'LASTEXITCODE after a failing parsed command'
}

Test-Case 'stdout-only capture does not throw on 5.1 despite redirecting' {
    # `2>$null` is a redirect too, so it hits the same 5.1 rule the merging
    # helper guards. Without the Continue flip this throws there and passes on 7.
    $out = Invoke-NativeStdout { cmd /c "echo only-stderr 1>&2" }
    Assert-Equal '' ("$out").Trim() 'a command writing only to stderr yields nothing, and does not throw'
}

# --- Invoke-NativeSplit: parse stdout, report stderr --------------------------
#
# For output parsed on success whose stderr is the diagnostic on failure.
# Discarding stderr keeps the parse clean and loses the only explanation of what
# went wrong; merging keeps the explanation and corrupts the parse. These pin
# that it does neither.

Test-Case 'split capture keeps the two streams apart' {
    $r = Invoke-NativeSplit { cmd /c "echo to-err 1>&2 & echo to-out" }
    Assert-Equal 'to-out' ("$($r.Stdout)").Trim() 'stdout must not carry stderr'
    if (("$($r.Stderr)").Trim() -ne 'to-err') {
        throw "stderr must be captured separately, got '$($r.Stderr)'"
    }
}

Test-Case 'split capture parses stdout as JSON under stderr noise' {
    $r = Invoke-NativeSplit { cmd /c "echo info: syncing 1>&2 & echo {`"ok`":true}" }
    $parsed = $r.Stdout | ConvertFrom-Json
    Assert-Equal $true $parsed.ok 'the JSON must survive stderr noise'
}

# The case the whole helper exists for, and the one a stdout-only capture got
# wrong: measured on `gh`, a network failure or a usage error leaves stdout EMPTY
# and puts the entire explanation on stderr. Reporting from stdout alone was
# blank exactly when the cause was least guessable.
Test-Case 'split capture still has a diagnostic when stdout is empty' {
    $r = Invoke-NativeSplit { cmd /c "echo only-on-stderr 1>&2 & exit 5" }
    Assert-Equal 5 $r.ExitCode 'the exit code must survive'
    Assert-Equal '' ("$($r.Stdout)").Trim() 'this command writes nothing to stdout'
    if (("$($r.Stderr)").Trim() -ne 'only-on-stderr') {
        throw "the diagnostic must survive on stderr, got '$($r.Stderr)'"
    }
}

Test-Case 'split capture does not throw on 5.1 despite redirecting' {
    # A file redirect is still a redirect, so this hits the same 5.1 rule.
    $r = Invoke-NativeSplit { cmd /c "echo x 1>&2" }
    Assert-Equal 0 $r.ExitCode 'a command that only writes to stderr still succeeds'
}

Test-Case 'split capture reports an empty stderr as empty, not null' {
    # StrictMode: a `$null` here would make `.Trim()` at the call sites throw.
    $r = Invoke-NativeSplit { cmd /c "echo quiet" }
    Assert-Equal '' ("$($r.Stderr)").Trim() 'a silent command yields empty stderr'
    if ($null -eq $r.Stderr) { throw 'Stderr must be an empty string rather than $null' }
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

# --- Read-SharedText: a file another handle still has open for writing --------
#
# Deterministic stand-in for the race CI hit: a redirect file is briefly still
# open for writing after its process exits. Holding a write handle open here
# makes that window permanent, so both directions are asserted on every run --
# the control proves the condition is real (ReadAllText refuses it), and the
# helper must read through it anyway.

Test-Case 'ReadAllText refuses a file still open for writing (the control)' {
    $path = Join-Path ([IO.Path]::GetTempPath()) "rst-control-$PID.txt"
    $writer = New-Object IO.FileStream($path, 'Create', 'Write', 'ReadWrite')
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes('held')
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        $threw = $false
        try { $null = [IO.File]::ReadAllText($path) } catch { $threw = $true }
        if (-not $threw) { throw 'ReadAllText read a write-held file, so this case no longer reproduces the race' }
    }
    finally { $writer.Dispose(); Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue }
}

Test-Case 'Read-SharedText reads a file still open for writing' {
    $path = Join-Path ([IO.Path]::GetTempPath()) "rst-shared-$PID.txt"
    $writer = New-Object IO.FileStream($path, 'Create', 'Write', 'ReadWrite')
    try {
        $bytes = [Text.Encoding]::UTF8.GetBytes("error: GUARD FIRED`n")
        $writer.Write($bytes, 0, $bytes.Length); $writer.Flush()
        Assert-Equal "error: GUARD FIRED`n" (Read-SharedText -Path $path) 'text read through a held write handle'
    }
    finally { $writer.Dispose(); Remove-Item -LiteralPath $path -ErrorAction SilentlyContinue }
}

Test-Case 'Read-SharedText still throws for a file that does not exist' {
    $threw = $false
    try { $null = Read-SharedText -Path (Join-Path ([IO.Path]::GetTempPath()) "rst-absent-$PID-none.txt") } catch { $threw = $true }
    if (-not $threw) { throw 'a missing file must not read as empty text' }
}

# --- Process-start failures --------------------------------------------------
#
# The decoder, the formatter and the text detector are what the CI retry
# wrapper and every exit-code report rely on, so each is tested both ways: a
# start failure is named, and nothing else is.

Test-Case 'a process-start code is named in either signed or unsigned form' {
    Assert-Equal '0xC0000142 STATUS_DLL_INIT_FAILED' (Get-ProcessStartFailure -1073741502) 'signed, as $LASTEXITCODE holds it'
    Assert-Equal '0xC0000142 STATUS_DLL_INIT_FAILED' (Get-ProcessStartFailure 3221225794) 'unsigned'
    Assert-Equal '0xC0000135 STATUS_DLL_NOT_FOUND' (Get-ProcessStartFailure -1073741515) 'another table entry'
}

Test-Case 'ordinary exit codes, non-numbers and null are not process-start failures' {
    foreach ($code in @(0, 1, 2, 101, -1, 'abc', '')) {
        if (Get-ProcessStartFailure $code) { throw "'$code' was named a process-start failure" }
    }
    if (Get-ProcessStartFailure $null) { throw 'null was named a process-start failure' }
}

Test-Case 'Format-ExitCode marks a start failure and leaves other codes bare' {
    $named = Format-ExitCode -1073741502
    if ($named -notmatch 'PROCESS-START FAILURE 0xC0000142 STATUS_DLL_INIT_FAILED') { throw "not marked: $named" }
    Assert-Equal '101' (Format-ExitCode 101) 'an ordinary code'
}

Test-Case 'the text detector finds the marker and raw codes, and nothing else' {
    # Every form Get-ProcessStartFailure accepts: signed decimal (as
    # $LASTEXITCODE holds it), unsigned decimal, and hex. PR #113 review found
    # the unsigned decimal form missing.
    foreach ($text in @((Format-ExitCode -1073741502), 'got [-1073741502]', 'got [3221225794]',
            'exit 0xc0000142', 'code 0xC0000135')) {
        if (-not (Test-ProcessStartFailureText $text)) { throw "missed: $text" }
    }
    # The last two are what the digit boundaries exist for: a longer negative
    # number that STARTS with the code, and the code glued to a preceding digit.
    foreach ($text in @('got [101]', 'took 10737415029 ns', 'id 11073741502', '', $null,
            'offset -10737415029', 'range 9-1073741502', 'size 32212257945', 'id 13221225794')) {
        if (Test-ProcessStartFailureText $text) { throw "false alarm: $text" }
    }
}

Test-Case 'the host pressure report produces lines rather than throwing' {
    $lines = @(Get-HostPressureReport)
    if ($lines.Count -lt 2) { throw "expected several lines, got $($lines.Count)" }
}

# --- A link that could not open its output ------------------------------------
#
# The sweep harness retries a build exactly once, and only for this. So the
# signature is tested both ways: the output of the real failure is recognised,
# and each way a link can fail for a reason that is NOT transient is not.

# The failure as cargo printed it in a sweep, with the middle of the link line
# omitted. Its `/OUT:` argument ends in `.exe"` as the error's own path does, and
# is not the error: a pattern that looked for the file name alone would find it.
$script:RealLinkFailure = @'
   Compiling win-multi-file-tail-watcher v0.1.3 (Q:\github\windows-threadpool-sys\.scratch\sabotage\tree\crates\win-multi-file-tail-watcher)
error: linking with `link.exe` failed: exit code: 1104
  |
  = note: "C:\\Program Files\\Microsoft Visual Studio\\18\\Enterprise\\VC\\Tools\\MSVC\\14.51.36231\\bin\\HostX64\\x64\\link.exe" "/NOLOGO" "<257 object files omitted>" "/OUT:Q:\\github\\windows-threadpool-sys\\.scratch\\sabotage\\target\\debug\\deps\\win_multi_file_tail_watcher-39ceb5c78e0fe780.exe" "/OPT:REF,NOICF" "/DEBUG"
  = note: some arguments are omitted. use `--verbose` to show all linker arguments
  = note: LINK : fatal error LNK1104: cannot open file 'Q:\github\windows-threadpool-sys\.scratch\sabotage\target\debug\deps\win_multi_file_tail_watcher-39ceb5c78e0fe780.exe'
          

error: could not compile `win-multi-file-tail-watcher` (lib test) due to 1 previous error
'@

Test-Case 'the real link failure names the output file that could not be opened' {
    Assert-Equal 'Q:\github\windows-threadpool-sys\.scratch\sabotage\target\debug\deps\win_multi_file_tail_watcher-39ceb5c78e0fe780.exe' `
        (Get-LinkOutputOpenFailure $script:RealLinkFailure) 'the path in the error, not the one in the link line'
}

Test-Case 'an output is recognised in any case, with spaces in its path, and as a library' {
    Assert-Equal 'C:\My Dir\x.EXE' (Get-LinkOutputOpenFailure "LINK : fatal error LNK1104: cannot open file 'C:\My Dir\x.EXE'") 'upper case with a space'
    Assert-Equal 'C:\t\deps\x.dll' (Get-LinkOutputOpenFailure "LINK : fatal error LNK1104: cannot open file 'C:\t\deps\x.dll'") 'a dll'
    $two = "LINK : fatal error LNK1104: cannot open file 'C:\a.exe'`nLINK : fatal error LNK1104: cannot open file 'C:\b.exe'"
    Assert-Equal 'C:\a.exe' (Get-LinkOutputOpenFailure $two) 'the first of several'
    Assert-Equal 'C:\a.exe' (Get-LinkOutputOpenFailure ($two -replace "`n", "`r`n")) 'CRLF line ends'
}

Test-Case 'a path with an apostrophe is named whole, and still judged by its extension' {
    Assert-Equal "C:\O'Brien\target\fixture.exe" `
        (Get-LinkOutputOpenFailure "  = note: LINK : fatal error LNK1104: cannot open file 'C:\O'Brien\target\fixture.exe'") `
        'captured to the last quote, not the first'
    # Cut at the first quote this would be `C:\O`, which is not an output, and a
    # real held output would be misread as the build's own failure.
    if (Get-LinkOutputOpenFailure "LINK : fatal error LNK1104: cannot open file 'C:\O'Brien\lib\kernel32.lib'") {
        throw 'an input with an apostrophe in its path was taken for an output'
    }
}

Test-Case 'the message quoted inside a compiler diagnostic is not a linker diagnostic' {
    # A patch whose source spells the message: rustc echoes the line it points
    # at, behind a gutter, and an unrelated error must stay the build's own.
    $echoed = @'
error[E0308]: mismatched types
 --> src\lib.rs:3:18
  |
3 |     let x: u32 = "LINK : fatal error LNK1104: cannot open file 'fixture.exe'";
  |            ---   ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ expected `u32`, found `&str`
'@
    if (Get-LinkOutputOpenFailure $echoed) { throw 'quoted source was taken for the linker' }
    if (Get-LinkOutputOpenFailure 'let m = "fatal error LNK1104: cannot open file ''fixture.exe''";') { throw 'mid-line text was taken for the linker' }
    # And the real line, with the same words, is still found beside it.
    if (-not (Get-LinkOutputOpenFailure ($echoed + "`n" + $script:RealLinkFailure))) { throw 'the real failure was hidden by the echo' }
}

Test-Case 'an input the linker cannot find is not an output that was held open' {
    # LNK1104 is also what a missing library or object says, and that is the
    # build's own failure, which a retry would only repeat.
    foreach ($text in @(
            "LINK : fatal error LNK1104: cannot open file 'kernel32.lib'",
            "LINK : fatal error LNK1104: cannot open file 'C:\t\deps\libfoo-1a2b.rlib'",
            "LINK : fatal error LNK1104: cannot open file 'C:\t\x.obj'",
            "LINK : fatal error LNK1104: cannot open file 'C:\a.exe\inner.lib'")) {
        if (Get-LinkOutputOpenFailure $text) { throw "an input was taken for an output: $text" }
    }
}

Test-Case 'one input among outputs makes the failure the build''s own' {
    $mixed = "LINK : fatal error LNK1104: cannot open file 'C:\a.exe'`nLINK : fatal error LNK1104: cannot open file 'kernel32.lib'"
    if (Get-LinkOutputOpenFailure $mixed) { throw 'a missing library was ignored because an output was also named' }
    $reversed = "LINK : fatal error LNK1104: cannot open file 'kernel32.lib'`nLINK : fatal error LNK1104: cannot open file 'C:\a.exe'"
    if (Get-LinkOutputOpenFailure $reversed) { throw 'order mattered' }
}

Test-Case 'other linker errors, and text without the error, are not this failure' {
    foreach ($text in @(
            "LINK : fatal error LNK1181: cannot open input file 'C:\t\x.obj'",
            'LINK : fatal error LNK1120: 1 unresolved externals',
            'x.obj : error LNK2019: unresolved external symbol main',
            "cannot open file 'C:\a.exe'",
            'LNK1104',
            "warning LNK1104: cannot open file 'C:\a.exe'",
            '', $null)) {
        if (Get-LinkOutputOpenFailure $text) { throw "false alarm: $text" }
    }
    # The command line cargo prints names the output too, and is not the error.
    if (Get-LinkOutputOpenFailure '"/OUT:C:\\t\\deps\\x.exe" "/DEBUG"') { throw 'the link line was taken for the error' }
}

# --- Sabotage patches and manifests -------------------------------------------
#
# The rule a sweep applies before it builds, and the no-build check applies to
# every entry. Both directions: a patch that applies is accepted, and each way one
# does not is told apart.

Test-Case 'a patch whose find text occurs once and changes the file applies' {
    $check = Test-SabotagePatch -Original "a`nb`nc`n" -Find @('b') -Replace @('B')
    Assert-Equal 'ok' $check.Outcome 'once, and a change'
    Assert-Equal "a`nB`nc`n" $check.Patched 'the patched text'
    Assert-Equal 1 $check.Occurrences 'counted'
}

Test-Case 'a patch whose find text is gone, or occurs twice, is stale' {
    $gone = Test-SabotagePatch -Original "a`nc`n" -Find @('b') -Replace @('B')
    Assert-Equal 'stale' $gone.Outcome 'not found'
    Assert-Equal 0 $gone.Occurrences 'none'
    if ($null -ne $gone.Patched) { throw 'a stale patch has no patched text' }
    $twice = Test-SabotagePatch -Original "b`nb`n" -Find @('b') -Replace @('B')
    Assert-Equal 'stale' $twice.Outcome 'found twice'
    Assert-Equal 2 $twice.Occurrences 'two'
}

Test-Case 'a patch that changes nothing is inert, not applied' {
    $check = Test-SabotagePatch -Original "a`nb`n" -Find @('b') -Replace @('b')
    Assert-Equal 'inert' $check.Outcome 'no change'
    if ($null -ne $check.Patched) { throw 'an inert patch has no patched text' }
}

Test-Case 'the lines of a patch are joined with LF, and a deletion is an empty replace' {
    $check = Test-SabotagePatch -Original "x`n    a;`n    b;`ny" -Find @('    a;', '    b;') -Replace @('')
    Assert-Equal 'ok' $check.Outcome 'two lines found as one block'
    Assert-Equal "x`n`ny" $check.Patched 'deleted'
    Assert-Equal "    a;`n    b;" $check.FindText 'joined with LF'
    # Lines that occur in the file but not as one block are not the block.
    Assert-Equal 'stale' (Test-SabotagePatch -Original "    a;`nz`n    b;" -Find @('    a;', '    b;') -Replace @('')).Outcome 'apart'
}

Test-Case 'manifests are found under crates, in order, and nowhere else' {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ('manifests-' + [guid]::NewGuid().ToString('N'))
    try {
        foreach ($rel in 'crates\b\sabotage.json', 'crates\a\sabotage-doctests.json', 'crates\a\sabotage.json',
            'crates\a\not-sabotage.json', 'crates\a\src\sabotage.json', 'sabotage.json') {
            $path = Join-Path $root $rel
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
            [System.IO.File]::WriteAllText($path, '{}')
        }
        $found = @(Get-SabotageManifest -RepositoryRoot $root | ForEach-Object { $_.Substring($root.Length + 1) })
        Assert-Equal 'crates\a\sabotage-doctests.json|crates\a\sabotage.json|crates\b\sabotage.json' ($found -join '|') 'a crate''s own manifests only'
        [System.IO.File]::WriteAllText((New-Item -ItemType File -Force -Path (Join-Path $root 'tools\sabotage2.json')).FullName, '{}')
        $found = @(Get-SabotageManifest -RepositoryRoot $root | ForEach-Object { $_.Substring($root.Length + 1) })
        Assert-Equal 3 $found.Count 'the tools manifest only when asked for'
        $found = @(Get-SabotageManifest -RepositoryRoot $root -IncludeTools | ForEach-Object { $_.Substring($root.Length + 1) })
        Assert-Equal 'crates\a\sabotage-doctests.json|crates\a\sabotage.json|crates\b\sabotage.json|tools\sabotage2.json' ($found -join '|') 'with -IncludeTools'
        Assert-Equal 0 @(Get-SabotageManifest -RepositoryRoot (Join-Path $root 'nowhere')).Count 'no crates directory, none'
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# --- Which manifests a change could have affected -----------------------------

Test-Case 'the inputs every sweep shares are recognised, and a crate''s own files are not' {
    foreach ($path in 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml', '.cargo/config.toml', 'tools/common.ps1',
        'tools/run-sabotage.ps1', 'tools\plan-sabotage-matrix.ps1', 'tools/check-sabotage-patches.ps1',
        '.github/workflows/sabotage-sweep.yml') {
        if (-not (Test-SabotageSharedInput $path)) { throw "not taken as shared: $path" }
    }
    foreach ($path in 'crates/a/Cargo.toml', 'crates/a/sabotage.json', 'tools/check-encoding.ps1', 'README.md',
        '.github/workflows/ci.yml', 'docs/Cargo.toml') {
        if (Test-SabotageSharedInput $path) { throw "taken as shared: $path" }
    }
}

# Crates a <- b <- c (c only as a dev-dependency), d on its own, e on c.
$script:Deps = @{ 'crates/a' = @(); 'crates/b' = @('crates/a'); 'crates/c' = @('crates/b'); 'crates/d' = @(); 'crates/e' = @('crates/c') }

function Get-Reach {
    param([string] $Crate, [hashtable] $Dependencies)
    [string[]] $reach = @(Get-SabotageCrateReach -Crate $Crate -Dependencies $Dependencies)
    [Array]::Sort($reach, [System.StringComparer]::Ordinal)
    return ($reach -join '|')
}

Test-Case 'a crate reaches itself and every crate it builds on, through any number of hops, and no others' {
    Assert-Equal 'crates/a|crates/b|crates/c|crates/e' (Get-Reach 'crates/e' $script:Deps) 'e, c, b, a'
    Assert-Equal 'crates/a|crates/b' (Get-Reach 'crates/b' $script:Deps) 'b and a, not what builds on b'
    Assert-Equal 'crates/d' (Get-Reach 'crates/d' $script:Deps) 'd alone'
}

Test-Case 'a dependency cycle is walked once, and a crate cargo does not know reaches only itself' {
    $cycle = @{ 'crates/a' = @('crates/b'); 'crates/b' = @('crates/a') }
    Assert-Equal 'crates/a|crates/b' (Get-Reach 'crates/a' $cycle) 'both, once'
    Assert-Equal 'crates/x' (Get-Reach 'crates/x' $cycle) 'unknown'
    Assert-Equal '' (Get-Reach '' @{}) 'the root crate'
}

Test-Case 'a manifest''s crate is its directory, and a path is in a crate only past its separator' {
    Assert-Equal '' (Get-SabotageManifestCrate 'sabotage.json') 'a manifest at the root'
    Assert-Equal 'crates/a' (Get-SabotageManifestCrate 'crates/a/sabotage-doctests.json') 'in a crate'
    if (-not (Test-SabotagePathInCrate 'crates/a/src/x.rs' 'crates/a')) { throw 'inside' }
    if (Test-SabotagePathInCrate 'crates/ab/src/x.rs' 'crates/a') { throw 'a sibling sharing a prefix is not inside' }
    if (Test-SabotagePathInCrate 'crates/a' 'crates/a') { throw 'the directory itself is no file in it' }
    if (-not (Test-SabotagePathInCrate 'README.md' '')) { throw 'the root holds every path' }
    if (-not (Test-SabotagePathInCrate 'crates/a/src/x.rs' 'CRATES/A')) { throw 'a crate named in another case is the same crate' }
}

Test-Case 'a manifest''s source root is its directory, or its root relative to that' {
    $dir = [System.IO.Path]::Combine('C:\', 'r', 'crates', 'a')
    Assert-Equal $dir (Get-SabotageSourceRoot -ManifestPath (Join-Path $dir 'sabotage.json') -Spec ([pscustomobject]@{ sabotages = @() })) 'its own'
    Assert-Equal (Join-Path $dir '..') (Get-SabotageSourceRoot -ManifestPath (Join-Path $dir 'sabotage.json') -Spec ([pscustomobject]@{ root = '..' })) 'its root'
}
function New-GitFixture {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ('scope-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    Invoke-Native { git -C $root init --quiet } | Out-Null
    return $root
}

function Save-GitFixture {
    param([string] $Root, [string] $Message)
    Invoke-Native { git -C $Root add -A } | Out-Null
    Invoke-Native { git -C $Root -c user.name=t -c user.email=t@t -c commit.gpgsign=false commit --quiet -m $Message } | Out-Null
    return ((Invoke-NativeStdout { git -C $Root rev-parse HEAD }) | Select-Object -First 1).Trim()
}

Test-Case 'what changed takes in commits, edits, and untracked files, measured from the merge base' {
    $root = New-GitFixture
    try {
        foreach ($f in 'crates/a/src/lib.rs', 'crates/b/src/lib.rs', 'README.md') {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent (Join-Path $root $f)) | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root $f), "x`n")
        }
        $base = Save-GitFixture $root 'base'
        [System.IO.File]::WriteAllText((Join-Path $root 'crates/a/src/lib.rs'), "changed`n")
        Save-GitFixture $root 'commit' | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $root 'README.md'), "edited, not committed`n")
        [System.IO.File]::WriteAllText((Join-Path $root 'crates/b/src/new.rs'), "untracked`n")
        $change = Get-SabotageChange -RepositoryRoot $root -Since $base
        Assert-Equal $true $change.Known $change.Reason
        Assert-Equal 'README.md|crates/a/src/lib.rs|crates/b/src/new.rs' ($change.Paths -join '|') 'committed, edited, untracked'

        $none = Get-SabotageChange -RepositoryRoot $root -Since 'HEAD'
        Assert-Equal $true $none.Known 'known'
        Assert-Equal 'README.md|crates/b/src/new.rs' ($none.Paths -join '|') 'only the working tree against HEAD'
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a base revision that is blank, all zeros, or unknown is reported as unknown, never as no change' {
    $root = New-GitFixture
    try {
        [System.IO.File]::WriteAllText((Join-Path $root 'f'), "x`n")
        Save-GitFixture $root 'base' | Out-Null
        foreach ($since in '', '0000000000000000000000000000000000000000', 'no-such-revision') {
            $change = Get-SabotageChange -RepositoryRoot $root -Since $since
            Assert-Equal $false $change.Known "'$since' cannot be measured from"
            if (-not $change.Reason) { throw "no reason for '$since'" }
            if ($since -match '^0+$' -and $change.Reason -notlike '*all-zero*') { throw "the reason says it is the all-zero revision: $($change.Reason)" }
        }
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'crate dependencies come from cargo, of every kind, keyed by directory' {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ('deps-' + [guid]::NewGuid().ToString('N'))
    try {
        New-Item -ItemType Directory -Force -Path $root | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $root 'Cargo.toml'), "[workspace]`nmembers = [`"crates/*`"]`nresolver = `"2`"`n")
        $crates = [ordered]@{
            a = ''
            b = "[dependencies]`na = { path = `"../a`" }`n"
            c = "[dev-dependencies]`nb = { path = `"../b`" }`n"
            d = "[build-dependencies]`na = { path = `"../a`" }`n"
        }
        foreach ($name in $crates.Keys) {
            New-Item -ItemType Directory -Force -Path (Join-Path $root "crates\$name\src") | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root "crates\$name\Cargo.toml"),
                "[package]`nname = `"$name`"`nversion = `"0.1.0`"`nedition = `"2021`"`n`n$($crates[$name])")
            [System.IO.File]::WriteAllText((Join-Path $root "crates\$name\src\lib.rs"), "`n")
        }
        $map = Get-SabotageCrateDependency -RepositoryRoot $root
        if ($null -eq $map) { throw 'cargo metadata did not answer' }
        Assert-Equal 'crates/a|crates/b|crates/c|crates/d' (($map.Keys | Sort-Object) -join '|') 'every crate'
        Assert-Equal 0 @($map['crates/a']).Count 'a depends on nothing'
        Assert-Equal 'crates/a' (@($map['crates/b']) -join '|') 'normal'
        Assert-Equal 'crates/b' (@($map['crates/c']) -join '|') 'dev'
        Assert-Equal 'crates/a' (@($map['crates/d']) -join '|') 'build'
        if ($null -ne (Get-SabotageCrateDependency -RepositoryRoot (Join-Path $root 'nowhere'))) { throw 'no workspace is no answer' }
        $packages = Get-SabotageCratePackage -RepositoryRoot $root
        Assert-Equal 'crates/a|crates/b|crates/c|crates/d' ((@('a', 'b', 'c', 'd') | ForEach-Object { $packages[$_] }) -join '|') 'each package''s directory'
        if ($null -ne (Get-SabotageCratePackage -RepositoryRoot (Join-Path $root 'nowhere'))) { throw 'no workspace, no packages' }
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# --- Which tests caught each entry ---------------------------------------------

Test-Case 'failed tests are named once each, in order, with doctests and should-panic tests named as their functions' {
    $text = @(
        'running 4 tests'
        'test a::b ... ok'
        'test a::c ... FAILED'
        'test a::d - should panic ... FAILED'
        "test src\lib.rs - Reader (line 12) ... FAILED`r"
        'test a::c ... FAILED'
        'test a::e ... ignored'
        'failures:'
        '    a::c'
        'test result: FAILED. 1 passed; 3 failed'
    ) -join "`n"
    Assert-Equal 'a::c|a::d|src\lib.rs - Reader (line 12)' (@(Get-FailedTestName $text) -join '|') 'failed ones, deduplicated'
    Assert-Equal 0 @(Get-FailedTestName "test a::b ... ok`ntest result: ok.").Count 'none failed'
    Assert-Equal 0 @(Get-FailedTestName $null).Count 'no text'
}

Test-Case 'a record round-trips through its file, and writes the same bytes whatever order it was built in' {
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ('catchers-' + [guid]::NewGuid().ToString('N'))
    try {
        $one = New-SabotageCatcherRecord
        Set-SabotageCatcher -Record $one -Manifest 'crates/b/sabotage.json' -Entry 'z' -Tests @() -At '2026-01-01T00:00:00.0000000Z'
        Set-SabotageCatcher -Record $one -Manifest 'crates/a/sabotage.json' -Entry 'y' -Tests @('t::one') -At '2026-01-02T00:00:00.0000000Z'
        Set-SabotageCatcher -Record $one -Manifest 'crates/a/sabotage.json' -Entry 'x' -Tests @('t::two', 't::three') -At '2026-01-03T00:00:00.0000000Z'
        $two = New-SabotageCatcherRecord
        Set-SabotageCatcher -Record $two -Manifest 'crates/a/sabotage.json' -Entry 'x' -Tests @('t::two', 't::three') -At '2026-01-03T00:00:00.0000000Z'
        Set-SabotageCatcher -Record $two -Manifest 'crates/a/sabotage.json' -Entry 'y' -Tests @('t::one') -At '2026-01-02T00:00:00.0000000Z'
        Set-SabotageCatcher -Record $two -Manifest 'crates/b/sabotage.json' -Entry 'z' -Tests @() -At '2026-01-01T00:00:00.0000000Z'
        $p1 = Join-Path $dir 'one.json'; $p2 = Join-Path $dir 'nested\two.json'
        Write-SabotageCatcherRecord -Path $p1 -Record $one
        Write-SabotageCatcherRecord -Path $p2 -Record $two
        Assert-Equal ([System.IO.File]::ReadAllText($p1)) ([System.IO.File]::ReadAllText($p2)) 'same bytes'
        $back = Read-SabotageCatcherRecord -Path $p1
        Assert-Equal 't::two|t::three' ($back['crates/a/sabotage.json']['x'].Tests -join '|') 'two tests, in order'
        Assert-Equal 't::one' ($back['crates/a/sabotage.json']['y'].Tests -join '|') 'one test stays a list'
        Assert-Equal 0 @($back['crates/b/sabotage.json']['z'].Tests).Count 'no tests'
        Assert-Equal '2026-01-03T00:00:00.0000000Z' $back['crates/a/sabotage.json']['x'].At 'when'
    }
    finally { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a record that is missing, not JSON, or of another version reads as empty' {
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ('catchers-' + [guid]::NewGuid().ToString('N'))
    try {
        New-Item -ItemType Directory -Force -Path $dir | Out-Null
        Assert-Equal 0 (Read-SabotageCatcherRecord -Path (Join-Path $dir 'none.json')).Count 'missing'
        foreach ($text in 'not json', '{"version":2,"manifests":{"m":{"e":{"tests":[],"at":"x"}}}}', '{"manifests":{}}', '[]') {
            $path = Join-Path $dir 'r.json'
            [System.IO.File]::WriteAllText($path, $text)
            Assert-Equal 0 (Read-SabotageCatcherRecord -Path $path).Count "read as empty: $text"
        }
        [System.IO.File]::WriteAllText($path, '{"version":1,"manifests":{"m":{"e":{"tests":["a"],"at":"x"}}}}')
        Assert-Equal 1 (Read-SabotageCatcherRecord -Path $path).Count 'and one of this version is read'
    }
    finally { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'merging keeps every entry any record has, and the latest of each' {
    $old = New-SabotageCatcherRecord
    Set-SabotageCatcher -Record $old -Manifest 'm' -Entry 'shared' -Tests @('old') -At '2026-01-01T00:00:00.0000000Z'
    Set-SabotageCatcher -Record $old -Manifest 'm' -Entry 'only-old' -Tests @('o') -At '2026-01-01T00:00:00.0000000Z'
    $new = New-SabotageCatcherRecord
    Set-SabotageCatcher -Record $new -Manifest 'm' -Entry 'shared' -Tests @('new') -At '2026-01-02T00:00:00.0000000Z'
    Set-SabotageCatcher -Record $new -Manifest 'n' -Entry 'only-new' -Tests @() -At '2026-01-02T00:00:00.0000000Z'
    foreach ($order in @(@($old, $new), @($new, $old))) {
        $merged = Merge-SabotageCatcherRecord -Records $order
        Assert-Equal 'new' ($merged['m']['shared'].Tests -join '|') 'the latest, whichever came first'
        Assert-Equal 'o' ($merged['m']['only-old'].Tests -join '|') 'kept from the old'
        if (-not ($merged.ContainsKey('n') -and $merged['n'].ContainsKey('only-new'))) { throw 'kept from the new' }
    }
}

# --- Which entries of a manifest a change could have affected -------------------

# A committed crate, crates/a, with a manifest of six entries: E1 patches lib.rs
# and was caught by a unit test in src/tests.rs; E2 patches other.rs and was
# caught by an integration test in tests/it.rs; E3 has no recorded catcher; E4 was
# caught by a doctest; E5 by a test no file defines any more; E6 is a survived
# control patching lib.rs.
function New-EntryScopeFixture {
    param([hashtable] $Extra = @{})
    $root = New-GitFixture
    $files = [ordered]@{
        'crates/a/src/lib.rs'   = "fn api() {}`n"
        'crates/a/src/other.rs' = "fn helper() {}`n"
        'crates/a/src/spare.rs' = "fn spare() {}`n"
        'crates/a/src/tests.rs' = "#[test]`nfn t_one() {}`n"
        'crates/a/tests/it.rs'  = "#[test]`nfn it_two() {}`n"
        'crates/a/README.md'    = "docs`n"
        'crates/z/src/lib.rs'   = "fn below() {}`n"
    }
    foreach ($rel in $Extra.Keys) { $files[$rel] = $Extra[$rel] }
    foreach ($rel in $files.Keys) {
        $path = Join-Path $root $rel
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
        [System.IO.File]::WriteAllText($path, $files[$rel])
    }
    $entry = { param($name, $file) [ordered]@{ name = $name; file = $file; expect = 'caught'; why = 'w'; find = @('fn'); replace = @('') } }
    $spec = [ordered]@{ package = 'a'; sabotages = @(
            (& $entry 'E1' 'src/lib.rs'), (& $entry 'E2' 'src/other.rs'), (& $entry 'E3' 'src/other.rs'),
            (& $entry 'E4' 'src/other.rs'), (& $entry 'E5' 'src/other.rs'), (& $entry 'E6' 'src/lib.rs')) }
    [System.IO.File]::WriteAllText((Join-Path $root 'crates/a/sabotage.json'), ($spec | ConvertTo-Json -Depth 8))
    $base = Save-GitFixture $root 'base'
    return [pscustomobject]@{ Root = $root; Base = $base }
}

# The record as learned at `Commit`, the fixture's base unless a case says otherwise.
function Get-ScopeRecord {
    param([string] $Commit)
    $record = New-SabotageCatcherRecord
    $at = '2026-01-01T00:00:00.0000000Z'
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E1' -Tests @('tests::t_one') -At $at -Commit $Commit
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E2' -Tests @('it_two') -At $at -Commit $Commit
    # Swept, and caught by no test it could name: what the selection knows least of.
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E3' -Tests @() -At $at -Commit $Commit
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E4' -Tests @('crates\a\src\lib.rs - api (line 1)') -At $at -Commit $Commit
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E5' -Tests @('tests::gone_test') -At $at -Commit $Commit
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E6' -Tests @() -At $at -Commit $Commit
    return $record
}

# Which entries are selected after `Edit` changes the fixture, joined.
function Get-ScopedEntries {
    param($Fixture, [scriptblock] $Edit, [hashtable] $Dependencies = @{ 'crates/a' = @('crates/z'); 'crates/z' = @() },
        [string] $RecordCommit, [string] $SourceRoot = 'crates/a',
        [hashtable] $Packages = @{ a = 'crates/a'; b = 'crates/b'; z = 'crates/z' },
        [scriptblock] $Record = { param($commit) Get-ScopeRecord $commit }, [string] $Manifest = 'crates/a/sabotage.json')
    & $Edit $Fixture.Root
    if (-not $RecordCommit) { $RecordCommit = $Fixture.Base }
    $change = Get-SabotageChange -RepositoryRoot $Fixture.Root -Since $Fixture.Base
    $spec = [System.IO.File]::ReadAllText((Join-Path $Fixture.Root $Manifest)) | ConvertFrom-Json
    $selection = Select-SabotageEntry -RepositoryRoot $Fixture.Root -Manifest $Manifest -Entries $spec.sabotages `
        -SourceRoot (Join-Path $Fixture.Root $SourceRoot) -Change $change -Dependencies $Dependencies -Record (& $Record $RecordCommit) `
        -Spec $spec -Packages $Packages
    return $selection
}

# The usual record, but with E2 (which patches other.rs) caught by
# backend::tests::deep: a test in a module, not at a target's root.
function Get-ModuleScopeRecord {
    param([string] $Commit)
    $record = Get-ScopeRecord $Commit
    Set-SabotageCatcher -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E2' -Tests @('backend::tests::deep') `
        -At '2026-01-01T00:00:00.0000000Z' -Commit $Commit
    return $record
}

# Which file, if any, selected E2 for its catching test's module path after
# `Edit`, in a fixture with `Files` added.
function Get-ModulePathReason {
    param([hashtable] $Files, [scriptblock] $Edit)
    $fixture = New-EntryScopeFixture -Extra $Files
    try {
        $sel = Get-ScopedEntries $fixture $Edit -Record { param($commit) Get-ModuleScopeRecord $commit }
        return [string]$sel.Reasons['E2']
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a change to a test file selects the entries it caught and those with nothing better known, and no others' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/tests.rs') -Value '// x' }
        Assert-Equal 'E1|E3|E4|E5|E6' (@($sel.Entries | ForEach-Object { $_.name }) -join '|') 'not E2, whose test is elsewhere'
        Assert-Equal "its catching test's file changed: crates/a/src/tests.rs" $sel.Reasons['E1'] 'E1, by its test'
        Assert-Equal 'no test is recorded as catching it' $sel.Reasons['E3'] 'E3, unknown'
        Assert-Equal 'a doctest caught it' $sel.Reasons['E4'] 'E4, doctest'
        Assert-Equal 'its catching test is gone: tests::gone_test' $sel.Reasons['E5'] 'E5, gone'
        Assert-Equal 'no test is recorded as catching it' $sel.Reasons['E6'] 'E6, caught by none'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a change to a patched file selects its entries, and one elsewhere in the crate only what is always selected' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/tests/it.rs') -Value '// x' }
        Assert-Equal 'E2|E3|E4|E5|E6' (@($sel.Entries | ForEach-Object { $_.name }) -join '|') 'E2 by its integration test, not E1'
        $fixture2 = New-EntryScopeFixture
        try {
            $sel = Get-ScopedEntries $fixture2 { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/lib.rs') -Value '// x' }
            Assert-Equal 'E1|E3|E4|E5|E6' (@($sel.Entries | ForEach-Object { $_.name }) -join '|') 'E1 by its file'
            Assert-Equal 'the file it patches changed: src/lib.rs' $sel.Reasons['E1'] 'why'
        }
        finally { Remove-Item -LiteralPath $fixture2.Root -Recurse -Force -ErrorAction SilentlyContinue }
        $fixture3 = New-EntryScopeFixture
        try {
            $sel = Get-ScopedEntries $fixture3 { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/spare.rs') -Value '// x' }
            Assert-Equal 'E3|E4|E5|E6' (@($sel.Entries | ForEach-Object { $_.name }) -join '|') 'nothing that has a better answer'
            Assert-Equal $false $sel.All 'and not all'
        }
        finally { Remove-Item -LiteralPath $fixture3.Root -Recurse -Force -ErrorAction SilentlyContinue }
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'an entry that is new or changed in the manifest is selected, and an unchanged one is not' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture {
            param($r)
            $path = Join-Path $r 'crates/a/sabotage.json'
            $spec = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
            $spec.sabotages[1].why = 'reworded'
            $spec.sabotages += [pscustomobject]@{ name = 'E7'; file = 'src/spare.rs'; expect = 'caught'; why = 'w'; find = @('fn'); replace = @('') }
            [System.IO.File]::WriteAllText($path, ($spec | ConvertTo-Json -Depth 8))
        }
        Assert-Equal 'the entry changed' $sel.Reasons['E2'] 'E2 changed'
        Assert-Equal 'the entry is new' $sel.Reasons['E7'] 'E7 new'
        if ($sel.Reasons.ContainsKey('E1')) { throw "E1 is unchanged and its file and test are untouched: $($sel.Reasons['E1'])" }
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a base that is not an ancestor of HEAD counts what it changed, so a force-push''s reverted paths are not lost' {
    $root = New-GitFixture
    try {
        foreach ($f in 'crates/a/src/lib.rs', 'README.md') {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent (Join-Path $root $f)) | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root $f), "x`n")
        }
        $base = Save-GitFixture $root 'base'
        [System.IO.File]::WriteAllText((Join-Path $root 'crates/a/src/lib.rs'), "pushed, then dropped`n")
        $dropped = Save-GitFixture $root 'the commit a force-push drops'
        Invoke-Native { git -C $root reset --quiet --hard $base } | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $root 'README.md'), "the new head`n")
        Save-GitFixture $root 'the new head' | Out-Null

        $change = Get-SabotageChange -RepositoryRoot $root -Since $dropped
        Assert-Equal $true $change.Known $change.Reason
        Assert-Equal $true $change.Divergent 'divergent'
        Assert-Equal 'README.md|crates/a/src/lib.rs' ($change.Paths -join '|') 'the reverted path counts'
        if ($change.Reason -notlike '*not an ancestor*') { throw "the reason says why: $($change.Reason)" }

        $ancestor = Get-SabotageChange -RepositoryRoot $root -Since $base
        Assert-Equal $false $ancestor.Divergent 'an ancestor is not divergent'
        Assert-Equal 'README.md' ($ancestor.Paths -join '|') 'and counts only what HEAD changed'
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a change to a manifest''s own settings selects every entry, and a reworded entry does not' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture {
            param($r)
            $path = Join-Path $r 'crates/a/sabotage.json'
            $spec = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
            $spec | Add-Member -NotePropertyName testArgs -NotePropertyValue @('test', '-p', 'a', '--release')
            [System.IO.File]::WriteAllText($path, ($spec | ConvertTo-Json -Depth 8))
        }
        Assert-Equal $true $sel.All 'every entry'
        Assert-Equal "every entry: the manifest's own settings changed" $sel.Reason 'why'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a manifest whose root is another crate sees a change to the files it patches there' {
    $edit = {
        param($r)
        New-Item -ItemType Directory -Force -Path (Join-Path $r 'crates/b/src') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $r 'crates/b/src/lib.rs'), "fn api() {}`n")
    }
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture $edit -Dependencies @{ 'crates/a' = @() } -SourceRoot 'crates/b'
        Assert-Equal 'the file it patches changed: src/lib.rs' $sel.Reasons['E1'] 'E1 patches crates/b/src/lib.rs'
        Assert-Equal 'the file it patches changed: src/lib.rs' $sel.Reasons['E6'] 'and so does E6'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture $edit -Dependencies @{ 'crates/a' = @() }
        Assert-Equal 0 @($sel.Entries).Count 'with its root its own crate, crates/b is nothing to it'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a record learned at a commit this checkout does not contain is not used' {
    $fixture = New-EntryScopeFixture
    try {
        # A commit made and then left behind: in the clone, not an ancestor of HEAD.
        [System.IO.File]::WriteAllText((Join-Path $fixture.Root 'crates/a/src/spare.rs'), "fn spare() { /* later */ }`n")
        $later = Save-GitFixture $fixture.Root 'a later commit'
        Invoke-Native { git -C $fixture.Root reset --quiet --hard $fixture.Base } | Out-Null
        $edit = { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/spare.rs') -Value '// x' }
        $sel = Get-ScopedEntries $fixture $edit -RecordCommit $later
        Assert-Equal 'no usable record of when it was last swept' $sel.Reasons['E1'] 'E1''s record is from a commit not here'
        Assert-Equal 'no usable record of when it was last swept' $sel.Reasons['E2'] 'nor E2''s'
        $sel = Get-ScopedEntries $fixture { param($r) } -RecordCommit $fixture.Base
        if ($sel.Reasons.ContainsKey('E1')) { throw "a record from the base is used: $($sel.Reasons['E1'])" }

        $record = Get-ScopeRecord $fixture.Base
        $usable = Get-SabotageCatcherTests -RepositoryRoot $fixture.Root -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E1'
        Assert-Equal $true $usable.Usable 'usable'
        Assert-Equal 'tests::t_one' ($usable.Tests -join '|') 'with its tests'
        foreach ($commit in '', $later, '0123456789abcdef0123456789abcdef01234567') {
            $record = Get-ScopeRecord $commit
            $none = Get-SabotageCatcherTests -RepositoryRoot $fixture.Root -Record $record -Manifest 'crates/a/sabotage.json' -Entry 'E1'
            Assert-Equal $false $none.Usable "not usable at '$commit'"
            Assert-Equal 0 @($none.Tests).Count 'and has no tests'
        }
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a recorded test deleted from a changed file is not hidden by a function of the same name elsewhere' {
    $fixture = New-EntryScopeFixture -Extra @{ 'crates/a/src/mask.rs' = "fn t_one() {}`n" }
    try {
        $sel = Get-ScopedEntries $fixture { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/src/tests.rs'), "// t_one is gone`n") }
        Assert-Equal "its catching test's file changed: crates/a/src/tests.rs" $sel.Reasons['E1'] 'E1, whose test was in tests.rs at the base'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}
Test-Case 'the packages a manifest tests are its package and every one its testArgs names before --' {
    $names = { param($spec) $r = Get-SabotageTestedPackage -Spec ([pscustomobject]$spec); ($r.Names -join '|') }
    Assert-Equal 'a' (& $names @{ package = 'a' }) 'the package alone'
    Assert-Equal 'a|b|c|d|e' (& $names @{ package = 'a'; testArgs = @('-p', 'b', '--package', 'c', '--package=d', '-pe', '--no-fail-fast') }) 'every spelling'
    Assert-Equal 'a|b' (& $names @{ package = 'a'; testArgs = @('test', '-p', 'a', '-p', 'b@1.2.3') }) 'once each, without a version'
    Assert-Equal 'a' (& $names @{ package = 'a'; testArgs = @('-p', 'a', '--', '-p', 'b') }) 'not past --, which are the test binary''s'
    Assert-Equal 'a' (& $names @{ package = 'a'; testArgs = @('--features', 'b/x', '--lib') }) 'a dependency''s feature is not a package tested'
    # testArgs replace `test -p <package>`; with no package of their own, cargo
    # tests the workspace's default members.
    Assert-Equal $true (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('--features', 'b/x') })).Workspace 'no package named'
    Assert-Equal $true (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('test', '--lib', '--', '-p', 'a') })).Workspace 'a -p past -- is not cargo''s'
    Assert-Equal $false (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('-p', 'a', '--lib') })).Workspace 'its own package named'
    $workspace = Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('--workspace') })
    Assert-Equal $true $workspace.Workspace '--workspace'
    Assert-Equal $true (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ testArgs = @('--all') })).Workspace '--all'
    Assert-Equal $false (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a' })).Workspace 'neither'
    $pattern = Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('-p', 'win-*') })
    if (-not $pattern.Unknown) { throw 'a pattern cannot be resolved to crates here' }
    if ($null -ne (Get-SabotageTestedPackage -Spec ([pscustomobject]@{ package = 'a'; testArgs = @('-p', 'b') })).Unknown) { throw 'a name is known' }
}

Test-Case 'a change to another package a manifest tests selects every entry, and an unknown one selects every entry' {
    $setB = {
        param($r)
        $path = Join-Path $r 'crates/a/sabotage.json'
        $spec = [System.IO.File]::ReadAllText($path) | ConvertFrom-Json
        $spec | Add-Member -NotePropertyName testArgs -NotePropertyValue @('-p', 'a', '-p', 'b')
        [System.IO.File]::WriteAllText($path, ($spec | ConvertTo-Json -Depth 8))
    }
    $addB = {
        param($r)
        New-Item -ItemType Directory -Force -Path (Join-Path $r 'crates/b/tests') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $r 'crates/b/tests/driver.rs'), "fn weakened() {}`n")
    }
    # Crate b builds on a, so a's own reach does not include it.
    $deps = @{ 'crates/a' = @(); 'crates/b' = @('crates/a') }
    $fixture = New-EntryScopeFixture
    try {
        & $setB $fixture.Root
        Save-GitFixture $fixture.Root 'tests b too' | Out-Null
        $fixture.Base = ((Invoke-NativeStdout { git -C $fixture.Root rev-parse HEAD }) | Select-Object -First 1).Trim()
        $sel = Get-ScopedEntries $fixture $addB -Dependencies $deps
        Assert-Equal $true $sel.All 'every entry'
        Assert-Equal 'every entry: a crate it builds on or tests changed: crates/b' $sel.Reason 'why'
        $unlisted = Get-ScopedEntries $fixture { param($r) } -Dependencies $deps -Packages @{ a = 'crates/a' }
        Assert-Equal "every entry: it tests package 'b', which cargo does not list" $unlisted.Reason 'b unknown'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture $addB -Dependencies $deps
        Assert-Equal 0 @($sel.Entries).Count 'a manifest that does not test b is not reached by it'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a manifest that tests the whole workspace is reached by a change to any crate' {
    $change = [pscustomobject]@{ Known = $true; Base = $null; Divergent = $false; Paths = @('crates/b/src/x.rs'); Reason = 'r' }
    $deps = @{ 'crates/a' = @(); 'crates/b' = @() }
    $entries = @([pscustomobject]@{ name = 'E'; file = 'src/lib.rs' })
    $select = {
        param($testArgs)
        Select-SabotageEntry -RepositoryRoot 'C:\r' -Manifest 'crates/a/sabotage.json' -Entries $entries -SourceRoot 'C:\r\crates\a' `
            -Change $change -Dependencies $deps -Record @{} -Spec ([pscustomobject]@{ package = 'a'; testArgs = $testArgs }) -Packages @{ a = 'crates/a' }
    }
    Assert-Equal $true (& $select @('--workspace')).All 'b is tested'
    # No repository here, so no record is usable, and E is taken for that alone.
    $notB = & $select @('-p', 'a')
    Assert-Equal $false $notB.All 'b is not'
    Assert-Equal 'no usable record of when it was last swept' $notB.Reasons['E'] 'and nothing about b'
    Assert-Equal $true (Select-SabotageEntry -RepositoryRoot 'C:\r' -Manifest 'crates/a/sabotage.json' -Entries $entries -SourceRoot 'C:\r\crates\a' `
            -Change $change -Dependencies $deps -Record @{} -Spec $null -Packages @{}).All 'no manifest settings given, every entry'
}
Test-Case 'git''s quoted paths are unquoted to the bytes they name, whatever the host decodes' {
    $e = [string][char]0xE9
    Assert-Equal "crates/x/caf$e.rs" (ConvertFrom-GitQuotedPath '"crates/x/caf\303\251.rs"') 'octal UTF-8'
    Assert-Equal "a`"b\c`td" (ConvertFrom-GitQuotedPath '"a\"b\\c\td"') 'C escapes'
    Assert-Equal "line`nbreak" (ConvertFrom-GitQuotedPath '"line\nbreak"') 'a newline in a name'
    foreach ($bad in '"\777"', '"unterminated', '"a\q"', '"\30"', '"\377"', 'plain', '"a"b"') {
        if ($null -ne (ConvertFrom-GitQuotedPath $bad)) { throw "not a quoting git writes: $bad" }
    }
    $split = Split-SabotageGitPaths @('plain/a.rs', '"crates/x/caf\303\251.rs"', '', 'b.rs')
    Assert-Equal $true $split.Ok 'ok'
    Assert-Equal "plain/a.rs|crates/x/caf$e.rs|b.rs" ($split.Paths -join '|') 'plain lines kept, quoted ones unquoted, blanks skipped'
    Assert-Equal 1 @((Split-SabotageGitPaths @('one.rs')).Paths).Count 'one path is still a list of one'
    $none = Split-SabotageGitPaths @()
    Assert-Equal $true $none.Ok 'nothing is a good answer'
    Assert-Equal 0 @($none.Paths).Count 'none'
}

Test-Case 'a git path this host decoded wrongly is not taken for a path' {
    # What a non-UTF-8 code page makes of an e-acute in a raw path: two other
    # characters, neither of them a replacement character.
    $mojibake = 'crates/x/caf' + [char]0x251C + [char]0x00AE + '.rs'
    Assert-Equal $false (Split-SabotageGitPaths @('crates/ok.rs', $mojibake)).Ok 'a path outside ASCII from git is not one git quoted'
    Assert-Equal $false (Split-SabotageGitPaths @('"\377"')).Ok 'a quoting that is not UTF-8'
}
Test-Case 'a path with a character git would quote is reported as it is, committed or untracked' {
    $root = New-GitFixture
    $name = 'caf' + [char]0xE9 + '.rs'
    try {
        foreach ($f in 'crates/a/src/lib.rs', "crates/a/src/$name") {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent (Join-Path $root $f)) | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root $f), "x`n")
        }
        $base = Save-GitFixture $root 'base'
        [System.IO.File]::WriteAllText((Join-Path $root "crates/a/src/$name"), "changed`n")
        Save-GitFixture $root 'change it' | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $root ('crates/a/src/new-' + $name)), "untracked`n")
        $change = Get-SabotageChange -RepositoryRoot $root -Since $base
        Assert-Equal $true $change.Known $change.Reason
        Assert-Equal "crates/a/src/$name|crates/a/src/new-$name" ($change.Paths -join '|') 'unquoted, exact'
        $sel = Select-SabotageEntry -RepositoryRoot $root -Manifest 'crates/b/sabotage.json' -Entries @() -SourceRoot (Join-Path $root 'crates/b') `
            -Change $change -Dependencies @{ 'crates/b' = @('crates/a') } -Record @{} -Spec ([pscustomobject]@{ package = 'b' }) -Packages @{ b = 'crates/b' }
        Assert-Equal $true $sel.All 'and it reaches what builds on its crate'
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}
Test-Case 'an entry with no usable record is selected whatever the change, and one with a record is not' {
    $fixture = New-EntryScopeFixture
    try {
        # The reviewer's case: a change that reaches nothing in the crate.
        $edit = { param($r) Add-Content -LiteralPath (Join-Path $r 'README.md') -Value 'x' }
        $sel = Get-ScopedEntries $fixture $edit -Dependencies @{ 'crates/a' = @() }
        Assert-Equal 0 @($sel.Entries).Count 'every entry recorded at the base, and nothing reached'
        $spec = [System.IO.File]::ReadAllText((Join-Path $fixture.Root 'crates/a/sabotage.json')) | ConvertFrom-Json
        $change = Get-SabotageChange -RepositoryRoot $fixture.Root -Since $fixture.Base
        $lost = Select-SabotageEntry -RepositoryRoot $fixture.Root -Manifest 'crates/a/sabotage.json' -Entries $spec.sabotages `
            -SourceRoot (Join-Path $fixture.Root 'crates/a') -Change $change -Dependencies @{ 'crates/a' = @() } `
            -Record (New-SabotageCatcherRecord) -Spec $spec -Packages @{ a = 'crates/a' }
        Assert-Equal 6 @($lost.Entries).Count 'the record lost: every entry'
        Assert-Equal 'no usable record of when it was last swept' $lost.Reasons['E1'] 'why'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'git''s view is not trusted when a tracked file is marked assume-unchanged or skip-worktree' {
    foreach ($flag in '--assume-unchanged', '--skip-worktree') {
        $root = New-GitFixture
        try {
            [System.IO.File]::WriteAllText((Join-Path $root 'tracked.rs'), "x`n")
            $base = Save-GitFixture $root 'base'
            Assert-Equal $null (Get-SabotageHiddenIndexState -RepositoryRoot $root) 'none yet'
            Invoke-Native { git -C $root update-index $flag tracked.rs } | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root 'tracked.rs'), "edited, and git does not say`n")
            if (-not (Get-SabotageHiddenIndexState -RepositoryRoot $root)) { throw "$flag is seen" }
            $change = Get-SabotageChange -RepositoryRoot $root -Since $base
            Assert-Equal $false $change.Known "$flag makes the change unmeasurable"
        }
        finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

Test-Case 'a copy is a commit only when every file it tracks is there, the same, with nothing else' {
    $root = New-GitFixture
    $copy = Join-Path ([System.IO.Path]::GetTempPath()) ('copy-' + [guid]::NewGuid().ToString('N'))
    try {
        foreach ($f in 'a.rs', 'dir/b.rs', '.gitignore') {
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent (Join-Path $root $f)) | Out-Null
            [System.IO.File]::WriteAllText((Join-Path $root $f), $(if ($f -eq '.gitignore') { "ignored/`n" } else { "x`n" }))
        }
        $commit = Save-GitFixture $root 'base'
        $fill = {
            Remove-Item -LiteralPath $copy -Recurse -Force -ErrorAction SilentlyContinue
            foreach ($f in 'a.rs', 'dir/b.rs', '.gitignore') {
                New-Item -ItemType Directory -Force -Path (Split-Path -Parent (Join-Path $copy $f)) | Out-Null
                Copy-Item -LiteralPath (Join-Path $root $f) -Destination (Join-Path $copy $f)
            }
        }
        & $fill
        Assert-Equal $true (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'the same'
        New-Item -ItemType Directory -Force -Path (Join-Path $copy 'ignored') | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $copy 'ignored/x'), 'x')
        Assert-Equal $true (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'an ignored file is no difference'
        [System.IO.File]::WriteAllText((Join-Path $copy 'a.rs'), "edited`n")
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'an edited file'
        & $fill; Remove-Item -LiteralPath (Join-Path $copy 'dir/b.rs')
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'a missing file'
        & $fill; [System.IO.File]::WriteAllText((Join-Path $copy 'extra.rs'), 'x')
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'a file the commit lacks'
        # What the repository's own index says is not what is checked.
        & $fill; Invoke-Native { git -C $root update-index --assume-unchanged a.rs } | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $copy 'a.rs'), "edited`n")
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit $commit) 'assume-unchanged hides nothing here'
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit '') 'no commit'
        Assert-Equal $false (Test-SabotageTreeIsCommit -RepositoryRoot $root -Tree $copy -Commit 'no-such') 'an unknown commit'
        Assert-Equal $true ($null -eq $env:GIT_INDEX_FILE) 'the index it used is not left behind'
    }
    finally {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $copy -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Test-Case 'a run is narrowed only when nothing after -- names tests already' {
    Assert-Equal $true (Test-SabotageNarrowable @('test', '-p', 'x')) 'no --'
    Assert-Equal $true (Test-SabotageNarrowable @('test', '-p', 'x', '--', '--exact', '--test-threads=1')) 'flags only'
    Assert-Equal $false (Test-SabotageNarrowable @('test', '-p', 'x', '--', 'only_this')) 'a filter'
    Assert-Equal $false (Test-SabotageNarrowable @('test', '--', '--test-threads', '1')) 'a value as its own word counts too'
}

Test-Case 'a run is narrowed only when nothing before -- is a TESTNAME for cargo test to pass on' {
    Assert-Equal $false (Test-SabotageNarrowable @('test', '-p', 'x', 'some_filter', '--locked')) 'a TESTNAME'
    Assert-Equal $false (Test-SabotageNarrowable @('test', 'some_filter', '--', '--exact')) 'a TESTNAME, with flags after --'
    Assert-Equal $false (Test-SabotageNarrowable @('test', '--lib', 'some_filter')) 'after a flag that takes no value'
    Assert-Equal $true (Test-SabotageNarrowable @('test', '-p', 'x', '--features', 'a b', '--test', 'it', '--target-dir', 'C:\t', '--locked')) `
        'option values are not names'
    Assert-Equal $true (Test-SabotageNarrowable @('test', '--package=x', '-Fa', '--lib')) 'values joined to their options'
    Assert-Equal $true (Test-SabotageNarrowable @('-p', 'x', '--no-fail-fast')) 'with no subcommand'
    Assert-Equal $false (Test-SabotageNarrowable @('test', 'test')) 'the subcommand once, then a filter that happens to say test'
}

Test-Case 'a manifest named in another case than git''s still sees the change to the file its entry patches' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/lib.rs') -Value '// x' } `
            -Manifest 'CRATES/A/sabotage.json' -SourceRoot 'CRATES/A'
        Assert-Equal 'the file it patches changed: src/lib.rs' $sel.Reasons['E1'] 'E1, by its file'
        Assert-Equal $false $sel.All 'and only what the change reached'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'a path typed in another case is named in the case it has on disk, and a missing part as given' {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ('case-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'Crates\Mixed') | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $root 'Crates\Mixed\Sabotage.json'), '{}')
    try {
        $got = Get-SabotageOnDiskPath (Join-Path $root 'CRATES\mixed\SABOTAGE.JSON')
        if (-not $got.EndsWith('\Crates\Mixed\Sabotage.json', [System.StringComparison]::Ordinal)) { throw "on disk: got '$got'" }
        if (-not (Test-Path -LiteralPath $got -PathType Leaf)) { throw "and it is that file: '$got'" }
        $missing = Get-SabotageOnDiskPath (Join-Path $root 'crates\MIXED\Nope.TXT')
        if (-not $missing.EndsWith('\Crates\Mixed\Nope.TXT', [System.StringComparison]::Ordinal)) { throw "missing: got '$missing'" }
    }
    finally { Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue }
}

# lib.rs declares backend; backend.rs declares its tests, in backend/tests.rs. Two
# controls declare modules of the same names in other places.
$script:ModuleFiles = @{
    'crates/a/src/lib.rs'               = "mod backend;`nfn api() {}`n"
    'crates/a/src/backend.rs'           = "#[cfg(test)]`nmod tests;`n"
    'crates/a/src/backend/tests.rs'     = "#[test]`nfn deep() {}`n"
    'crates/a/src/spare.rs'             = "#[cfg(test)]`nmod tests;`nfn spare() {}`n"
    'crates/a/src/elsewhere/backend.rs' = "#[cfg(test)]`nmod tests;`n"
}

Test-Case 'a change to a file declaring a module a catching test is in selects the entry, and one of the same name elsewhere does not' {
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/src/backend.rs'), "`n") }
    Assert-Equal 'a module its catching test is in changed: crates/a/src/backend.rs' $why 'its module''s file dropped `mod tests;`'
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/src/backend.rs'), "#[cfg(any())]`nmod tests;`n") }
    Assert-Equal 'a module its catching test is in changed: crates/a/src/backend.rs' $why 'its module''s file gated it'
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/lib.rs') -Value '// x' }
    Assert-Equal 'a module its catching test is in changed: crates/a/src/lib.rs' $why 'the root declaring its first module'
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/spare.rs') -Value '// x' }
    Assert-Equal '' $why 'a file declaring a module of the same name, not on its path'
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/src/elsewhere/backend.rs') -Value '// x' }
    Assert-Equal '' $why 'a file named for its module, not beside a root'
    $why = Get-ModulePathReason $script:ModuleFiles { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/tests/it.rs') -Value '// x' }
    Assert-Equal '' $why 'a root not declaring its first module'
}

Test-Case 'a module''s mod.rs, and a root Cargo.toml names, are on the path too' {
    $files = @{
        'crates/a/src/lib.rs'                = "mod backend;`n"
        'crates/a/src/backend/mod.rs'        = "#[cfg(test)]`nmod tests;`n"
        'crates/a/src/backend/tests.rs'      = "#[test]`nfn deep() {}`n"
    }
    $why = Get-ModulePathReason $files { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/src/backend/mod.rs'), "`n") }
    Assert-Equal 'a module its catching test is in changed: crates/a/src/backend/mod.rs' $why 'mod.rs'
    $files = @{
        'crates/a/Cargo.toml'            = "[package]`nname = `"a`"`n[lib]`npath = `"lib/root.rs`"`n"
        'crates/a/lib/root.rs'           = "mod backend;`n"
        'crates/a/lib/backend.rs'        = "#[cfg(test)]`nmod tests;`n"
        'crates/a/lib/backend/tests.rs'  = "#[test]`nfn deep() {}`n"
    }
    $why = Get-ModulePathReason $files { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/a/lib/root.rs') -Value '// x' }
    Assert-Equal 'a module its catching test is in changed: crates/a/lib/root.rs' $why 'the root Cargo.toml names'
    $why = Get-ModulePathReason $files { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/lib/backend.rs'), "`n") }
    Assert-Equal 'a module its catching test is in changed: crates/a/lib/backend.rs' $why 'a module beside that root'
}

Test-Case 'in a crate that places a module''s file with #[path], any changed file declaring a module of the path counts' {
    $files = @{
        'crates/a/src/lib.rs'            = "#[path = `"impl/b.rs`"]`nmod backend;`n"
        'crates/a/src/impl/b.rs'         = "#[cfg(test)]`nmod tests;`n"
        'crates/a/src/impl/b/tests.rs'   = "#[test]`nfn deep() {}`n"
    }
    $why = Get-ModulePathReason $files { param($r) [System.IO.File]::WriteAllText((Join-Path $r 'crates/a/src/impl/b.rs'), "`n") }
    Assert-Equal 'a module its catching test is in changed: crates/a/src/impl/b.rs' $why 'the file #[path] names, by what it declared'
}
Test-Case 'a change to a crate''s own build settings selects every entry' {
    foreach ($path in 'crates/a/Cargo.toml', 'crates/a/build.rs', 'crates/a/.cargo/config.toml') {
        $fixture = New-EntryScopeFixture
        try {
            $sel = Get-ScopedEntries $fixture {
                param($r)
                $full = Join-Path $r $path
                New-Item -ItemType Directory -Force -Path (Split-Path -Parent $full) | Out-Null
                [System.IO.File]::WriteAllText($full, "[lib]`ntest = false`n")
            }.GetNewClosure()
            Assert-Equal $true $sel.All "every entry for $path"
            Assert-Equal "every entry: the crate's build settings changed: $path" $sel.Reason 'why'
        }
        finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
    }
}

Test-Case 'a record older than the base is judged against everything since it, so a missed run cannot hide a change' {
    $fixture = New-EntryScopeFixture
    try {
        # The records are learned at the fixture's base, A. Commit B weakens E1's
        # catching test, and its own sweep never saved a record. A push from B,
        # touching nothing, must still find E1 changed since it was last swept.
        $a = $fixture.Base
        [System.IO.File]::WriteAllText((Join-Path $fixture.Root 'crates/a/src/tests.rs'), "#[test]`nfn t_one() { /* weakened */ }`n")
        $b = Save-GitFixture $fixture.Root 'weaken t_one'
        $fixture.Base = $b
        $sel = Get-ScopedEntries $fixture { param($r) } -RecordCommit $a
        $short = $a.Substring(0, 8)
        Assert-Equal "since it was last swept, at ${short}: its catching test's file changed: crates/a/src/tests.rs" $sel.Reasons['E1'] 'E1'
        if ($sel.Reasons.ContainsKey('E2')) { throw "E2's test and file are untouched since A: $($sel.Reasons['E2'])" }
        # E3 was caught by no test it could name, so any change in its crate since A counts.
        Assert-Equal "since it was last swept, at ${short}: no test is recorded as catching it" $sel.Reasons['E3'] 'E3'
        # A record from B itself has nothing to answer for since.
        $current = Get-ScopedEntries $fixture { param($r) } -RecordCommit $b
        Assert-Equal 0 @($current.Entries).Count 'nothing since B'
        # A crate below changing between A and B reaches every recorded entry.
        $fixture2 = New-EntryScopeFixture
        try {
            [System.IO.File]::WriteAllText((Join-Path $fixture2.Root 'crates/z/src/lib.rs'), "fn below() { /* changed */ }`n")
            $a2 = $fixture2.Base
            $fixture2.Base = Save-GitFixture $fixture2.Root 'change z'
            $sel = Get-ScopedEntries $fixture2 { param($r) } -RecordCommit $a2
            Assert-Equal "since it was last swept, at $($a2.Substring(0, 8)): a crate it builds on or tests changed: crates/z" $sel.Reasons['E2'] 'E2'
            Assert-Equal "since it was last swept, at $($a2.Substring(0, 8)): a crate it builds on or tests changed: crates/z" $sel.Reasons['E3'] 'E3 too'
        }
        finally { Remove-Item -LiteralPath $fixture2.Root -Recurse -Force -ErrorAction SilentlyContinue }
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}

Test-Case 'win-job-launcher, and every workspace crate it builds on, is an input every sweep shares' {
    if (-not (Test-SabotageSharedInput 'crates/win-job-launcher/src/main.rs')) { throw 'the launcher' }
    if (Test-SabotageSharedInput 'crates/win-job-launcherx/src/main.rs') { throw 'a sibling sharing its prefix' }
    # Against cargo, so a path dependency the launcher gains later is not missed.
    $root = Split-Path -Parent $PSScriptRoot
    $deps = Get-SabotageCrateDependency -RepositoryRoot $root
    if ($null -eq $deps) { throw 'cargo metadata did not answer' }
    foreach ($dir in @(Get-SabotageCrateReach -Crate 'crates/win-job-launcher' -Dependencies $deps)) {
        if (-not (Test-SabotageSharedInput "$dir/src/lib.rs")) { throw "the launcher builds on $dir, which is not a shared input" }
    }
}
Test-Case 'every entry for a change below, a shared input, or a change that cannot be measured; none for nothing reached' {
    $fixture = New-EntryScopeFixture
    try {
        $sel = Get-ScopedEntries $fixture { param($r) Add-Content -LiteralPath (Join-Path $r 'crates/z/src/lib.rs') -Value '// x' }
        Assert-Equal $true $sel.All 'a crate it builds on'
        Assert-Equal 6 @($sel.Entries).Count 'all six'
        Assert-Equal 'every entry: a crate it builds on or tests changed: crates/z' $sel.Reason 'why'

        $none = Get-ScopedEntries $fixture { param($r) } -Dependencies @{ 'crates/a' = @() }
        Assert-Equal 0 @($none.Entries).Count 'z changed, but a does not build on it here'

        Add-Content -LiteralPath (Join-Path $fixture.Root 'Cargo.lock') -Value 'x'
        $shared = Get-ScopedEntries $fixture { param($r) } -Dependencies @{ 'crates/a' = @() }
        Assert-Equal $true $shared.All 'a shared input'

        $spec = [System.IO.File]::ReadAllText((Join-Path $fixture.Root 'crates/a/sabotage.json')) | ConvertFrom-Json
        $unknown = Select-SabotageEntry -RepositoryRoot $fixture.Root -Manifest 'crates/a/sabotage.json' -Entries $spec.sabotages `
            -SourceRoot (Join-Path $fixture.Root 'crates/a') -Change (Get-SabotageChange -RepositoryRoot $fixture.Root -Since 'no-such') `
            -Dependencies @{} -Record (Get-ScopeRecord $fixture.Base) -Spec $spec -Packages @{ a = 'crates/a' }
        Assert-Equal $true $unknown.All 'unmeasurable'
        $nomap = Select-SabotageEntry -RepositoryRoot $fixture.Root -Manifest 'crates/a/sabotage.json' -Entries $spec.sabotages `
            -SourceRoot (Join-Path $fixture.Root 'crates/a') -Change (Get-SabotageChange -RepositoryRoot $fixture.Root -Since $fixture.Base) `
            -Dependencies $null -Record (Get-ScopeRecord $fixture.Base) -Spec $spec -Packages @{ a = 'crates/a' }
        Assert-Equal $true $nomap.All 'no crate map'
    }
    finally { Remove-Item -LiteralPath $fixture.Root -Recurse -Force -ErrorAction SilentlyContinue }
}
# --- ConvertTo-NativeArgument: one argument, quoted for a command line --------

Test-Case 'a plain argument is left unquoted' {
    Assert-Equal 'abc' (ConvertTo-NativeArgument 'abc') 'plain'
    Assert-Equal '--flag=value' (ConvertTo-NativeArgument '--flag=value') 'flag'
    Assert-Equal 'C:\a\b' (ConvertTo-NativeArgument 'C:\a\b') 'backslashes not before a quote'
}

Test-Case 'an empty argument is an empty pair of quotes' {
    Assert-Equal '""' (ConvertTo-NativeArgument '') 'empty'
}

Test-Case 'whitespace and quotes are quoted and escaped' {
    Assert-Equal '"a b"' (ConvertTo-NativeArgument 'a b') 'space'
    Assert-Equal "`"a`tb`"" (ConvertTo-NativeArgument "a`tb") 'tab'
    Assert-Equal '"a\"b"' (ConvertTo-NativeArgument 'a"b') 'embedded quote'
    Assert-Equal '"a\\\"b"' (ConvertTo-NativeArgument 'a\"b') 'backslash before a quote'
    Assert-Equal '"C:\a b\\"' (ConvertTo-NativeArgument 'C:\a b\') 'trailing backslash'
    Assert-Equal '"C:\a b\\\\"' (ConvertTo-NativeArgument 'C:\a b\\') 'two trailing backslashes'
}

# The property that matters is the round trip, so it goes through a real host.
Test-Case 'arguments with spaces, quotes and trailing backslashes survive Start-Process whole' {
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) "native arg $PID"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    try {
        $script = Join-Path $dir 'echo args.ps1'
        [System.IO.File]::WriteAllText($script, '$args | ForEach-Object { "[$_]" }' + "`r`n")
        $values = @('plain', 'with space', 'a"b', 'C:\dir with space\', 'two  spaces', '')
        $shell = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell' }
        $out = Join-Path $dir 'out.txt'
        $arguments = @('-NoProfile', '-File', $script) + $values |
            ForEach-Object { ConvertTo-NativeArgument ([string]$_) }
        $process = Start-Process -FilePath $shell -ArgumentList $arguments -PassThru -NoNewWindow `
            -RedirectStandardOutput $out -Wait
        $null = $process.Handle
        $got = @(Get-Content -LiteralPath $out)
        Assert-Equal ($values.Count) $got.Count "argument count, got: $($got -join ' ')"
        for ($i = 0; $i -lt $values.Count; $i++) {
            Assert-Equal "[$($values[$i])]" $got[$i] "argument $i"
        }
    }
    finally { Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue }
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
