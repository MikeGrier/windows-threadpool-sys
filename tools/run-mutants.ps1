# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Runs cargo-mutants with the settings this workspace needs, and without a
    crashing mutant wedging the run behind a modal dialog.

.DESCRIPTION
    Three things go wrong when cargo-mutants is invoked directly here, and this
    wrapper exists because each one has already cost a run.

    **A crashing mutant pops a modal dialog and stalls the entire run.** Mutation
    testing is expected to produce weird programs, and some of them are not
    merely wrong but memory-unsafe -- inverting the `ERROR_IO_PENDING` check in
    `arm_detailed_read` makes the thread pool cancel its accounting for an I/O
    the kernel is still going to complete, so the completion lands in a freed
    buffer. The crash is fine and expected. What is not fine is the operating
    system's response: a modal "Application Error" box offering to debug, which
    nothing in an automated run will ever click.

    Two dialogs can appear, and they have different switches:

    - The **WER "Application Error" box**, controlled by `DontShowUI` under
      HKCU. That is what this wrapper sets and restores.
    - The **JIT debugger prompt** ("Click on CANCEL to debug the program"),
      controlled by `AeDebug\Debugger` and `AeDebug\Auto` under HKLM. On this
      machine `vsjitdebugger.exe` is registered with `Auto` unset, which means
      prompt. `DontShowUI` suppressed it in practice -- a 144-mutant sweep with
      sixteen crashes ran to completion -- but the key is HKLM and would need
      elevation to change, so if a run ever stalls again with a debugger prompt,
      that is the knob to look at rather than this one.

    **Measured cost, which is the reason this is not optional.** With the dialog
    suppressed, a crashing mutant costs 7.9s and 10.5s (the two that crashed in
    the watcher.rs sweep) and is recorded as `CaughtMutant`. Without it the run
    does not merely slow down: it stops advancing entirely and never resumes,
    because `--timeout` bounds the *test*, not a process sitting on a message
    pump. The first watcher.rs attempt wedged after 33 mutants and had to be
    killed by hand.

    Deliberately **not** setting WER's `Disabled` as well: with `DontShowUI` on,
    zero crash dumps were collected during that sweep, so there is no report
    collection left to switch off and the extra registry write would be
    unmeasured complexity.

    **Features go to cargo-mutants itself, not after `--`, and never both.**
    cargo-mutants mutates source, so it happily mutates a module behind a
    feature that is off -- the mutation lands in code that is never compiled,
    the suite passes trivially, and the result is recorded as `missed`.
    Measured here: 57 of 61 survivors in one crate and 147 of 247 in another
    were this and nothing else. Passing it on *both* sides does not work at all:
    cargo-mutants forwards its trailing arguments onto the same `cargo test`, so
    the flag lands twice on one command line and cargo refuses it outright
    (`the argument '--all-features' cannot be used multiple times`), killing the
    run in the baseline. Passing it only *after* `--` reaches the test run but
    not the `--no-run` build, so the first build is thrown away and rebuilt with
    features on, and the build/test split cargo-mutants reports times a
    configuration it never tested.

    **`-j 2`, not more.** This workspace has timing-sensitive tests; under heavy
    parallel load one can fail for want of a CPU rather than because it detected
    the mutant, which cargo-mutants records as a *false* caught.

.PARAMETER Package
    Crate to mutate. Defaults to the file-watcher, the crate this was built for.

.PARAMETER File
    Repository-relative source file to scope to. Strongly recommended: a
    whole-crate sweep here takes hours, where one file takes about fifteen
    minutes and gives a result you can act on and re-verify the same day.

.PARAMETER Jobs
    Parallel jobs. See above before raising it.

.PARAMETER TimeoutSeconds
    Fixed per-mutant test timeout, in seconds. Leave at 0 to derive it from the
    measured baseline instead, which is the default and the better option.

    **Timeouts used to dominate the wall clock here, and no longer do.** Two
    changes removed that, both driven by measurement rather than by tuning this
    knob. M15.6 converted the queue tests to bounded waiting, so a broken wake
    fails instead of hanging. M15.7 then found the budget those bounded waits
    spend was 30s while 45 of 46 waits complete in 2.5ms or less -- so a mutation
    that broke delivery made dozens of tests each burn 30s, overrunning the
    deadline. Lowering `NOTIFY_TIMEOUT` to 5s took one such mutant from 93.6s
    (killed, filed `timeout`) to 31.8s (a clean red test).

    The lesson generalises: when mutants pile up in the `timeout` column, suspect
    the *suite's* wait budget before reaching for this parameter. A timeout is
    usually a detection that was not allowed to finish.

    A fixed number is still the wrong shape. `--timeout-multiplier` scales the
    deadline from the baseline test time cargo-mutants already measures, so it
    adapts to the machine instead of encoding one. The baseline here is about 30s
    for the full `--all-features` suite, so the default multiplier of 3 gives
    ~90s: comfortably above any legitimate run, and it shrinks automatically on a
    faster host.

    Lower it only with the false-timeout risk in mind. A mutant that is recorded
    `timeout` because the deadline was too tight is misattributed twice over --
    it is not a hang, and it is not necessarily caught either. Adjudicate any
    `timeout` by counting `FAILED` lines in its log before treating it as a gap.

.PARAMETER TimeoutMultiplier
    Test timeout as a multiple of the measured baseline. Ignored when
    `-TimeoutSeconds` is non-zero.

.PARAMETER OutputDirectory
    Where to write `mutants.out`. Defaults under `.scratch/`, and carries a
    per-run timestamp, so a run never overwrites a previous run's results --
    neither the repository root's `mutants.out`, which has already lost one
    analysis, nor an earlier sweep of the same scope.

.EXAMPLE
    .\tools\run-mutants.ps1 -File crates/windows-file-watcher/src/watcher.rs
#>
[CmdletBinding()]
param(
    [string] $Package = 'windows-file-watcher',
    [string] $File,
    [int] $Jobs = 2,
    [int] $TimeoutSeconds = 0,

    [double] $TimeoutMultiplier = 3,
    [string] $OutputDirectory
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# The single output sink. Every message this tool emits goes through here, so
# the destination and the formatting stay separable from the call sites that
# produce the content -- the repository's one-output-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'detail', 'note', 'warn')][string] $Level = 'info'
    )
    $colour = switch ($Level) {
        'detail' { 'DarkGray' }
        'note' { 'Cyan' }
        'warn' { 'Yellow' }
        default { 'Gray' }
    }
    Write-Host $Message -ForegroundColor $colour
}

$repo = (git rev-parse --show-toplevel).Replace('/', '\')
if (-not $OutputDirectory) {
    $leaf = if ($File) { [System.IO.Path]::GetFileNameWithoutExtension($File) } else { $Package }
    # Stamped per run, not merely per scope. A path derived from the package or
    # file alone is the same path every time, so a second run of the same scope
    # overwrites the analysis this parameter promises to preserve -- and two
    # concurrent runs write into one directory.
    #
    # The timestamp sorts chronologically so the most recent run reads last, and
    # the short random suffix is what actually makes it unique: the stamp has
    # one-second resolution, so two runs launched inside the same second -- a
    # script starting several scopes at once, which is the case that most wants
    # separate output -- would otherwise select the same directory and interleave
    # their results.
    $stamp = (Get-Date).ToString('yyyyMMdd-HHmmss')
    $unique = [guid]::NewGuid().ToString('N').Substring(0, 6)
    $OutputDirectory = Join-Path $repo ".scratch\mutants-$leaf-$stamp-$unique"
}

$werKey = 'HKCU:\Software\Microsoft\Windows\Windows Error Reporting'
$hadKey = Test-Path $werKey
# Anything WER or the JIT debugger is already holding when this starts belongs
# to somebody else -- an unrelated crash report the user is reading, or a live
# debugging session. Recording them now is what lets the cleanup below kill only
# what this run produced. Matching by name alone would terminate those too.
$preexistingFaultHandlers = @(
    Get-Process -Name 'WerFault', 'WerFaultSecure', 'vsjitdebugger' -ErrorAction SilentlyContinue |
        ForEach-Object { $_.Id }
)
# `.GetValue(name, $null)` rather than `Get-ItemProperty -Name`: under
# `Set-StrictMode -Version Latest` the latter throws when the property is absent
# (which is the default state of this one), so the wrapper died before it could
# do anything -- reading the "is it already set?" question was itself the bug.
$previous = if ($hadKey) { (Get-Item $werKey).GetValue('DontShowUI', $null) } else { $null }

if ($previous -eq 1) {
    # Left set by a previous run that was killed before its `finally` could run
    # -- the one hole in the restore, since a hard kill runs no cleanup. Say so,
    # because silently treating it as the user's own preference would restore it
    # to 1 afterwards and leave the machine permanently changed by a crash.
    Write-Report "NOTE: DontShowUI was already 1. If a previous run was killed, clear it after:" -Level warn
    Write-Report "  Remove-ItemProperty '$werKey' -Name DontShowUI" -Level warn
}

try {
    if (-not $hadKey) { New-Item -Path $werKey -Force | Out-Null }
    Set-ItemProperty -Path $werKey -Name DontShowUI -Value 1 -Type DWord
    Write-Report "WER dialogs suppressed for this run (DontShowUI=1)." -Level note

    $argv = @('mutants', '-p', $Package, '-j', $Jobs,
        '--output', $OutputDirectory, '--all-features')
    if ($TimeoutSeconds -gt 0) {
        $argv += @('--timeout', $TimeoutSeconds)
    }
    else {
        $argv += @('--timeout-multiplier', $TimeoutMultiplier)
    }
    if ($File) { $argv += @('--file', $File) }

    Write-Report "cargo $($argv -join ' ')" -Level detail
    & cargo @argv
    $code = $LASTEXITCODE
}
finally {
    # Anything WER or the JIT debugger left holding a dead test process. With
    # the dialog suppressed these should not appear at all; killing them is
    # insurance against a stale one pinning a target file and failing the next
    # build with a confusing "access denied".
    #
    # **Only the ones this run produced.** An earlier version matched by name,
    # which also terminated a crash report the user was reading or a debugger
    # they had attached to something else entirely -- a wrapper for a mutation
    # sweep has no business doing that. Processes alive before this started are
    # excluded by id, and a pid recorded then cannot be confused with a later
    # one: Windows will not reuse it while the process object is still open,
    # and these are all still running when the snapshot is taken.
    foreach ($name in 'WerFault', 'WerFaultSecure', 'vsjitdebugger') {
        Get-Process -Name $name -ErrorAction SilentlyContinue |
            Where-Object { $preexistingFaultHandlers -notcontains $_.Id } |
            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    }

    if ($null -ne $previous) {
        Set-ItemProperty -Path $werKey -Name DontShowUI -Value $previous -Type DWord
    }
    else {
        Remove-ItemProperty -Path $werKey -Name DontShowUI -ErrorAction SilentlyContinue
        if (-not $hadKey) { Remove-Item -Path $werKey -ErrorAction SilentlyContinue }
    }
    Write-Report "WER dialog setting restored." -Level note
}

# cargo-mutants treats `--output` as the PARENT and creates `mutants.out` inside
# it, so this join is correct rather than a doubled path. Verified on this
# workspace: a run with `--output .scratch\mutants-encoding-<stamp>` produced
# `.scratch\mutants-encoding-<stamp>\mutants.out\caught.txt` with 22 lines, and
# the summary below reported 22. Written down because the path reads like a
# duplication and has already been challenged once.
$out = Join-Path $OutputDirectory 'mutants.out'
foreach ($name in 'caught', 'missed', 'timeout', 'unviable') {
    $path = Join-Path $out "$name.txt"
    $count = if (Test-Path $path) { (Get-Content $path | Measure-Object -Line).Lines } else { 0 }
    "{0,-9} {1}" -f $name, $count
}
Write-Report "results: $out" -Level detail

# cargo-mutants exits non-zero when anything survived, which is the normal
# outcome of an investigative run rather than a failure of this script.
exit $code
