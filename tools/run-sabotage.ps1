# Copyright (c) 2026 Mike Grier. All rights reserved.
<#
.SYNOPSIS
    Runs a sabotage sweep: injects deliberate defects one at a time and reports
    whether the test suite noticed.

.DESCRIPTION
    A green suite is evidence that the code passes its tests. It is not evidence
    that the tests would fail if the code were wrong, and those are different
    claims. This script measures the second one: for each defect in a manifest
    it patches the source, runs the suite, and records whether the suite went
    red.

    IT NEVER TOUCHES YOUR WORKING TREE. The sweep runs against a copy, kept
    under the output directory and refreshed from the real tree at the start of
    each run, with its own cargo target directory. This is the same approach
    cargo-mutants takes, and it is a premise rather than a precaution: patching
    the developer's own files means every sabotage needs a backup, a restore, a
    verification that the restore worked, a guard against running on a dirty
    tree, and a way to recover when any of that is interrupted -- and each of
    those is a chance to damage work that was never in a commit. Against a copy
    none of it exists, a dirty tree is swept exactly as it stands, and the worst
    outcome of a bug in here is a scratch directory to delete.

    Four rules are encoded here because each was learned by getting it wrong.

    JUDGE BY EXIT CODE, NEVER BY READING OUTPUT. A test process that dies of
    heap corruption prints no "test result: FAILED" line at all. A harness that
    greps for that string will report a hole in the tests where there is none,
    and the hours then spent looking for it are pure loss.

    A TIMEOUT COUNTS AS CAUGHT. A missing wakeup does not fail a test, it hangs
    it -- so a harness with no timeout hangs too, and a lost-wakeup defect that
    hangs the suite has been detected exactly as intended.

    THE BASELINE MUST BE GREEN FIRST. Against an already-red suite every
    sabotage "fails" and the whole sweep means nothing while looking like a
    clean bill of health. This script refuses to start until the unmodified
    suite passes.

    A sabotage that reports NOT CAUGHT is not automatically a hole in the
    tests -- it may be a defect in the sabotage. One that inserts unreachable
    code beside a live call, rather than removing the call, changes the file
    without changing the behaviour, and the suite then passes for the honest
    reason that nothing was broken. That failure mode is silent and it has
    happened here, so this script prints the applied patch for every unexpected
    result: check that the injected defect really is a defect before believing
    a hole exists.

    THE COPY'S BUILD MUST NOT SHARE THE REAL TARGET DIRECTORY. A sabotaged
    build written into the tree's own target/ would be left there for the next
    `cargo test` a person runs, which would then be testing sabotaged code
    without knowing it. The copy therefore gets its own target directory, which
    persists between runs so the builds stay warm.

    Expect one incremental rebuild per sabotage -- measured at a few seconds
    once the copy's target directory is warm; the first run after a fresh copy
    pays a cold build. This is a deliberate, occasional instrument -- run it
    when a guard is written or changed, not on every commit.

.PARAMETER Manifest
    Path to a sabotage manifest (JSON). See tools/README-sabotage.md for the
    format, and crates/windows-waitable-queues/sabotage.json for a worked
    example.

.PARAMETER Name
    Optional wildcard filter over sabotage names, to re-run just one.

.PARAMETER TimeoutSeconds
    Bound on TEST EXECUTION only. A run that exceeds it is killed and counted as
    caught, because a lost wakeup hangs rather than fails.

    DERIVED FROM THE BASELINE by default, not a fixed number: the baseline runs
    the unmodified suite first anyway, so its measured duration is the best
    available statement of how long this suite legitimately takes on THIS
    machine. The bound is `max(TimeoutFloorSeconds, TimeoutMultiplier x
    baseline)`, which adapts to a fast suite and to a slow machine without
    either being guessed at.

    This matters because hangs dominate the wall clock. Measured on the 39-entry
    waitable-queues manifest at the old fixed 60-second default: 853 seconds
    total, of which twelve hangs accounted for 720. The baseline's test phase
    there is about 12 seconds, so a derived bound is roughly half the fixed one
    and the sweep finishes materially sooner -- while a suite that genuinely
    takes longer gets MORE room than the old fixed number gave it, not less.

    Pass an explicit value to override the derivation entirely. A manifest may
    also set `timeoutSeconds` globally, and any single sabotage may set its own
    for the case this parameter cannot express: one entry that legitimately
    needs far longer than the rest.

    This is deliberately separate from -BuildTimeoutSeconds, and the split is
    what makes a tight bound safe here. Because a timeout counts as caught, a
    bound shorter than legitimate work manufactures a FALSE "caught" -- it
    credits the tests with detecting a defect they never ran against, which is
    the dangerous direction to be wrong in. Under a single combined bound the
    number had to be generous enough for the slowest imaginable cold build,
    which made every genuinely-hanging sabotage cost that same generous number.

    Measured on this workspace: rebuilding the crate in the working copy after a
    one-file edit takes about four seconds, and test execution takes about
    twelve, nearly all of it compiling doctests -- `cargo test --no-run` does not
    build those, and Cargo offers no `--doc --no-run` to pre-pay it. A sweep
    whose result you intend to believe should never have this tightened for
    speed: the bound exists to separate "hung" from "slow", and every second cut
    from it is headroom taken from that distinction.

.PARAMETER TimeoutMultiplier
    How many times the baseline's measured test duration a run may take before
    it is called hung. Defaults to 3. Ignored when -TimeoutSeconds is given.

.PARAMETER TimeoutFloorSeconds
    Lower bound on the derived timeout, defaulting to 15. Without it a suite
    that runs in a fraction of a second would derive a bound so tight that
    ordinary scheduling noise would read as a hang -- and a false hang is scored
    as CAUGHT, which is the direction that quietly inflates a result. Ignored
    when -TimeoutSeconds is given.

.PARAMETER BuildTimeoutSeconds
    Bound on the build phase, defaulting to 300. Generous on purpose: a slow
    cold build must never be mistaken for a hang, and it costs nothing when
    builds are fast. A sabotage that fails to build is reported as such rather
    than as caught -- the compiler rejecting a patch says nothing about whether
    the tests would have noticed it. The same holds for a patch that only breaks
    a doctest: those cannot be built in this phase, so they surface during the
    run and are reclassified there rather than being counted as caught.

.PARAMETER OutputDirectory
    Where to write per-sabotage transcripts, and where the working copy and its
    cargo target directory live. Defaults to .scratch/sabotage. Stale
    transcripts are cleared at startup; the `tree/` and `target/` subdirectories
    persist between runs so builds stay warm.

.PARAMETER List
    Print the manifest's sabotages and exit without running anything.

.PARAMETER CargoCommand
    The executable each phase runs, defaulting to `cargo`. Exists so this
    script's own tests can substitute a stub that exits, fails, or hangs on
    demand: without it every test of the timeout, kill, classification and
    transcript logic would have to pay a real cargo build, which is the
    difference between a suite that runs in seconds and one nobody runs. It also
    lets a manifest sweep something whose tests are not `cargo test`, given a
    `testArgs` that spells the whole command.

.OUTPUTS
    Exits 0 only if every sabotage matched its declared expectation.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Manifest,

    [string] $Name = '*',

    [int] $TimeoutSeconds = 0,

    [int] $TimeoutMultiplier = 3,

    [int] $TimeoutFloorSeconds = 15,

    [int] $BuildTimeoutSeconds = 300,

    [string] $OutputDirectory,

    [switch] $List,

    [string] $CargoCommand = 'cargo'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# `Invoke-Native`, which the git calls below go through. Dot-sourced rather than
# imported: a module copy of that guard does not reach the scriptblock it is
# handed, and silently fails on Windows PowerShell 5.1 alone -- see
# [common.ps1](common.ps1), and [test-common.ps1](test-common.ps1) for the
# cross-host proof.
. (Join-Path $PSScriptRoot 'common.ps1')

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

# Script scope so Invoke-Bounded reads it without threading it through three
# call layers that have no other reason to know about it.
$script:CargoExecutable = $CargoCommand

# The single output sink. Every message this tool emits goes through here, so
# the destination and the formatting stay separable from the call sites that
# produce the content -- the repository's one-output-sink rule. `Exit-WithMessage`
# below is deliberately NOT routed through it: that path writes to stderr and
# then exits, and is the one case where the destination is part of the meaning.
function Write-Report {
    param(
        [Parameter(Mandatory = $true, ValueFromPipeline = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'note', 'good', 'bad')][string] $Level = 'info'
    )
    process {
        $colour = switch ($Level) {
            'note' { 'Cyan' }
            'good' { 'Green' }
            'bad' { 'Red' }
            default { 'Gray' }
        }
        Write-Host $Message -ForegroundColor $colour
    }
}

# Writes to stderr and exits with a code, rather than Write-Error, which under
# $ErrorActionPreference = 'Stop' raises a terminating error that propagates out
# of this script and aborts whatever invoked it. A diagnostic tool reporting a
# bad manifest must not take the caller's session down with it.
function Exit-WithMessage {
    param([string] $Message, [int] $Code)
    [Console]::Error.WriteLine($Message)
    exit $Code
}

function Get-RepoRoot {
    # Through `Invoke-Native`, and `2>$null` is exactly why. Any stderr redirect
    # -- not just `2>&1` -- makes a native command's stderr a TERMINATING error
    # on Windows PowerShell 5.1 under `Stop`, and this is the one call in this
    # script whose failure mode IS stderr: outside a working tree git says
    # `fatal: not a git repository`. Measured on 5.1, unguarded, the line below
    # was unreachable -- the script died with NativeCommandError and exited 1,
    # which in this script means "sabotages did not behave as declared" rather
    # than "you ran me in the wrong directory". PowerShell 7 reached it either
    # way, which is why the deliberate exit-2 path looked fine.
    $root = Invoke-Native { git rev-parse --show-toplevel }
    if ($LASTEXITCODE -ne 0) {
        # Reported, not thrown. Under $ErrorActionPreference = 'Stop' a `throw`
        # here is a terminating error that prints a stack trace and propagates
        # into whatever invoked this script -- the same defect class as the
        # manifest failures below, and for the same reason it is wrong: the
        # problem is the caller's working directory, not a bug in this file.
        # Raised in the PR #64 review.
        Exit-WithMessage (@(
                "Not inside a git repository, so there is no repository root to"
                "resolve sabotage targets against or to check them for cleanliness."
                "Run this from within the working tree you mean to sweep."
            ) -join "`n") 2
    }
    return $root.Replace('/', '\')
}

# PSScriptAnalyzer asks for `SupportsShouldProcess` on a `Stop-` verb. Declined
# deliberately: that machinery exists to raise a confirmation prompt, and this
# function's whole job is to kill a build that has already hung. A tool built to
# detect hangs must not acquire a way to hang on a prompt. PSScriptAnalyzer is
# not a gate in this repository -- CI executes these scripts rather than linting
# them, and all four siblings in this directory carry the same class of warning.
function Stop-Tree {
    param([int] $ProcessId)
    Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Tree -ProcessId $_.ProcessId }
    Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
}

# Runs cargo under a wall-clock bound, and reports which of the three outcomes
# occurred. A hang is a distinct outcome from a failure because it is what a
# lost-wakeup defect looks like, and collapsing the two would hide that.
function Invoke-Bounded {
    param(
        [string[]] $CargoArgs,
        [string] $WorkingDirectory,
        [string] $TranscriptPath,
        [int] $Seconds
    )

    $process = Start-Process -FilePath $script:CargoExecutable -ArgumentList $CargoArgs `
        -WorkingDirectory $WorkingDirectory -PassThru -NoNewWindow `
        -RedirectStandardOutput $TranscriptPath `
        -RedirectStandardError "$TranscriptPath.err"

    # Reading .Handle before waiting, and discarding it, is load bearing on
    # Windows PowerShell 5.1. A Process object from `Start-Process -PassThru`
    # there does not cache the native handle; once the process exits the handle
    # is released and .ExitCode comes back $null -- for a process that exited 0
    # exactly as for one that failed. Every phase would then classify as
    # 'failed', so the baseline could never pass, and if it somehow did, every
    # sabotage would read as 'caught': a clean bill of health that proves
    # nothing, which is the one result this harness exists to make impossible.
    # Touching .Handle keeps it alive so the exit code survives the wait.
    #
    # Measured, not assumed: under 5.1 a `cmd /c exit 0` reports ExitCode $null
    # without this line and 0 with it. A no-op on PowerShell 7, which caches the
    # handle itself. Found by running the suite under 5.1 after the PR #64
    # review flagged the ternary on the line below -- fixing only that would
    # have turned a loud parse error into a silent wrong answer.
    $null = $process.Handle

    # Polled against a deadline rather than `WaitForExit($Seconds * 1000)`,
    # because that overload does NOT reliably return at the timeout here.
    #
    # cargo's stdout and stderr are redirected to files, and the test binary
    # cargo spawns INHERITS those handles. .NET's timed WaitForExit waits for
    # the redirected streams to reach end-of-file as well as for the process, so
    # the wait outlives the bound for exactly as long as the grandchild holds
    # the handles open -- which, for a hung test, is forever. Measured: a sweep
    # sat on a single hung sabotage for 31 MINUTES against a 60-second bound,
    # with the kill never reached, and resumed the moment that test binary was
    # killed by hand. A tool whose whole job is to detect hangs must not be
    # hangable by one.
    #
    # HasExited only asks the kernel whether the process object is signalled and
    # never touches the streams, so this loop cannot overrun its deadline.
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $deadline = (Get-Date).AddSeconds($Seconds)
    while (-not $process.HasExited -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    $clock.Stop()
    $elapsed = [int][Math]::Ceiling($clock.Elapsed.TotalSeconds)

    if ($process.HasExited) {
        # Spelled as an if/else rather than a ternary on purpose: `? :` is
        # PowerShell 7 syntax, and this is a PARSE error under Windows
        # PowerShell 5.1 -- so a single ternary anywhere makes the whole script
        # unrunnable on the shell that `powershell.exe` still starts by default,
        # failing before the first line executes rather than at this line. The
        # three sibling scripts in this directory are 5.1-clean; this one stays
        # that way too. Raised in the PR #64 review.
        $outcome = if ($process.ExitCode -eq 0) { 'passed' } else { 'failed' }
        return [pscustomobject]@{ Outcome = $outcome; Code = $process.ExitCode; Seconds = $elapsed }
    }

    Stop-Tree -ProcessId $process.Id
    return [pscustomobject]@{ Outcome = 'hung'; Code = $null; Seconds = $elapsed }
}

# Inserts a cargo flag BEFORE any `--` separator, rather than at the end.
#
# Everything after `--` belongs to the test binary, not to cargo. Appending
# `--no-run` to a vector that ends in test-binary arguments would hand it to
# libtest, which does not know it -- so the build phase would run the tests
# instead of merely building them, and then fail for a reason unrelated to the
# sabotage. The manifest format allows that vector (`testArgs`), so this is the
# manifest author's mistake to be immune to rather than to warn about.
function Add-CargoFlag {
    param([string[]] $CargoArgs, [string[]] $Flag)

    $separator = [array]::IndexOf($CargoArgs, '--')
    if ($separator -lt 0) { return @($CargoArgs) + $Flag }

    # Select-Object rather than a range, because a `--` at index 0 would make
    # the range $CargoArgs[0..-1], and a negative index counts from the end in
    # PowerShell -- silently reversing the vector instead of yielding nothing.
    $before = @($CargoArgs | Select-Object -First $separator)
    $after = @($CargoArgs | Select-Object -Skip $separator)
    return $before + $Flag + $after
}

# Builds, then runs, under two separate bounds.
#
# The phases are timed apart because they mean different things. A hang is what
# a lost wakeup looks like, and it happens in test EXECUTION -- so that phase
# gets a tight bound. A build is merely slow sometimes, and a slow build killed
# by a tight bound would be reported as a hang, crediting the tests with a
# detection that never happened. So the build gets a generous one, and its
# failure is reported as its own outcome rather than folded into "caught":
# the compiler rejecting a patch tells you nothing about your tests.
function Invoke-Sabotaged {
    param(
        [string[]] $CargoArgs,
        [string] $WorkingDirectory,
        [string] $TranscriptPath,
        [int] $BuildSeconds,
        [int] $TestSeconds
    )

    $build = Invoke-Bounded -CargoArgs (Add-CargoFlag -CargoArgs $CargoArgs -Flag '--no-run') `
        -WorkingDirectory $WorkingDirectory `
        -TranscriptPath "$TranscriptPath.build" -Seconds $BuildSeconds

    if ($build.Outcome -eq 'failed') {
        return [pscustomobject]@{ Outcome = 'build-failed'; Code = $build.Code; Seconds = 0 }
    }
    if ($build.Outcome -eq 'hung') {
        return [pscustomobject]@{ Outcome = 'build-hung'; Code = $null; Seconds = 0 }
    }

    $run = Invoke-Bounded -CargoArgs $CargoArgs -WorkingDirectory $WorkingDirectory `
        -TranscriptPath $TranscriptPath -Seconds $TestSeconds

    # The one hole the phase split above cannot close by itself: `--no-run` does
    # not build DOCTESTS (see the note at the top of this file -- `cargo test
    # --doc --no-run` is rejected outright with "can't skip running doc tests",
    # so there is no way to pre-pay them in the build phase). A patch that is
    # valid Rust in the crate but not in a doc example therefore sails through
    # the build and fails here, during the run, with a compile error -- and
    # would otherwise be reported as `caught`, which is exactly the weaker claim
    # this function exists to keep separate from the stronger one.
    #
    # Detected by reading the transcript rather than by an exit code, which is
    # a deliberate exception to how every other outcome here is judged: rustdoc
    # reports a doctest that would not compile as an ordinary test failure, so
    # the exit code is 101 either way and carries no way to tell them apart.
    # The marker is libtest's own fixed string, verified on this toolchain to
    # land on stdout (the transcript, not `.err`).
    if ($run.Outcome -eq 'failed' -and (Test-Path -LiteralPath $TranscriptPath)) {
        if (Select-String -LiteralPath $TranscriptPath -Pattern "Couldn't compile the test." -SimpleMatch -Quiet) {
            return [pscustomobject]@{ Outcome = 'doc-compile-failed'; Code = $run.Code; Seconds = $run.Seconds }
        }
    }

    return $run
}

function Format-Patch {
    param([string] $Find, [string] $Replace)
    $lines = @('    --- injected patch ---')
    foreach ($line in $Find -split "`n") { $lines += "    - $line" }
    # The empty case is `(removed)` ALONE, not an empty `+` line followed by it.
    # Splitting "" yields one empty element, so the old order printed both. Five
    # entries across the two shipped manifests use the deleting form, so this was
    # every deletion sabotage's patch display. Raised in the PR #64 review.
    if ([string]::IsNullOrEmpty($Replace)) {
        $lines += '    + (removed)'
    }
    else {
        foreach ($line in $Replace -split "`n") { $lines += "    + $line" }
    }
    return $lines -join "`n"
}

# Refreshes the working copy from the real tree, and returns nothing.
#
# WHICH FILES. Enumerated by git -- tracked files plus untracked ones that are
# not ignored -- and read from the WORKING TREE, not from a commit. So a sweep
# measures the code as it currently sits, uncommitted edits included, which is
# usually the code whose guards you are asking about. Using git's own view is
# also what keeps target/ (28 GB here) and the output directory out of the copy
# without maintaining a second exclusion list that could drift from
# .gitignore.
#
# WHY NOT COPY EVERY TIME. Cargo decides what to rebuild from mtimes, so
# re-copying an unchanged file would touch it and force a rebuild of everything
# on every sweep. Only files whose contents actually differ are copied, which is
# what keeps the copy's target directory warm between runs; measured here at
# roughly four seconds for an incremental sabotage against thirty for a cold
# one. Files that have disappeared from the source are deleted, so a rename
# cannot leave a stale twin behind for cargo to compile.
function Sync-Tree {
    param([string] $From, [string] $To)

    # core.quotePath=false is load bearing, not tidiness. With git's default,
    # any path containing a non-ASCII byte is emitted wrapped in double quotes
    # with octal escapes -- `"crates/.../zz-caf\303\251.txt"` -- and
    # Test-Path -LiteralPath on that string is false, so the file would be
    # silently neither copied NOR recorded as wanted. The copy would then differ
    # from the real tree by exactly the files nothing here can see, and the
    # symptom would be a red baseline blamed on the suite. Measured both ways on
    # a probe file.
    $tracked = @(git -c core.quotePath=false -C $From ls-files)
    if ($LASTEXITCODE -ne 0) { Exit-WithMessage 'Could not list tracked files.' 2 }
    $untracked = @(git -c core.quotePath=false -C $From ls-files --others --exclude-standard)
    if ($LASTEXITCODE -ne 0) { Exit-WithMessage 'Could not list untracked files.' 2 }

    $wanted = @{}
    foreach ($relative in ($tracked + $untracked)) {
        if (-not $relative) { continue }
        $source = Join-Path $From $relative
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { continue }

        $wanted[$relative.Replace('/', '\')] = $true
        $destination = Join-Path $To $relative

        $same = $false
        if (Test-Path -LiteralPath $destination -PathType Leaf) {
            $sourceInfo = Get-Item -LiteralPath $source
            $destinationInfo = Get-Item -LiteralPath $destination
            if ($sourceInfo.Length -eq $destinationInfo.Length) {
                $same = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -eq
                        (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
            }
        }
        if ($same) { continue }

        $parent = Split-Path -Parent $destination
        if ($parent -and -not (Test-Path -LiteralPath $parent)) {
            New-Item -ItemType Directory -Force -Path $parent | Out-Null
        }
        # WriteAllBytes rather than Copy-Item: byte-exact, and it stamps the
        # copy with the current time so cargo sees a changed input. Copy-Item
        # would carry the source's older timestamp across and cargo could then
        # judge an artifact built from different content to be up to date.
        [System.IO.File]::WriteAllBytes($destination, [System.IO.File]::ReadAllBytes($source))
    }

    if (-not (Test-Path -LiteralPath $To)) { return }
    $prefix = [System.IO.Path]::GetFullPath($To)
    if (-not $prefix.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
        $prefix += [System.IO.Path]::DirectorySeparatorChar
    }
    foreach ($existing in @(Get-ChildItem -LiteralPath $To -Recurse -File -ErrorAction SilentlyContinue)) {
        # The cargo target directory lives under the output directory, not under
        # the copy, so nothing here can reach it.
        $relative = $existing.FullName.Substring($prefix.Length)
        if (-not $wanted.ContainsKey($relative)) {
            Remove-Item -LiteralPath $existing.FullName -Force -ErrorAction SilentlyContinue
        }
    }
}

# Which transcript actually holds the evidence for an outcome.
#
# The two phases write different files, and cargo puts its diagnostics on
# stderr, so a build failure's evidence is in `.build.err` while a test
# failure's is in the transcript itself. Naming the wrong one sends the reader
# to a file that this run never wrote -- and, because a build-phase failure
# never reaches the test phase, that file may still hold a GREEN transcript
# from an earlier sweep, contradicting the message that points at it.
function Get-EvidencePath {
    param([string] $Outcome, [string] $TranscriptPath)
    switch ($Outcome) {
        'build-failed' { return "$TranscriptPath.build.err" }
        'build-hung' { return "$TranscriptPath.build.err" }
        default { return $TranscriptPath }
    }
}

# A bound at or below zero gets the answer wrong in the dangerous direction.
# The wait is a deadline poll, so zero or negative means the deadline has
# already passed: every phase is classified as a hang, and this tool scores a
# hang as `caught`. A sweep run that way would report every sabotage caught and
# exit 0 -- a fully green result proving nothing, which is the one outcome the
# harness exists to make impossible. Rejected here rather than allowed to reach
# the wait. Raised in the PR #64 review.
#
# (The original of this comment blamed WaitForExit's argument validation. That
# was true of the wait this replaced, not of the poll that is here now; the
# guard is still necessary, but for the reason above.)
$bounds = @(@{ Name = 'BuildTimeoutSeconds'; Value = $BuildTimeoutSeconds },
    @{ Name = 'TimeoutMultiplier'; Value = $TimeoutMultiplier },
    @{ Name = 'TimeoutFloorSeconds'; Value = $TimeoutFloorSeconds })
# Zero means "derive it" for -TimeoutSeconds alone, so it is checked only when
# the caller actually supplied a value.
if ($TimeoutSeconds -ne 0) {
    $bounds += @{ Name = 'TimeoutSeconds'; Value = $TimeoutSeconds }
}
foreach ($bound in $bounds) {
    if ($bound.Value -lt 1) {
        Exit-WithMessage (@(
                "-$($bound.Name) must be at least 1, and was $($bound.Value)."
                "A bound of zero does not disable the timeout; it makes every phase"
                "time out instantly, which this tool would report as every sabotage"
                "being caught -- a green sweep that proved nothing. Pass a real"
                "budget instead."
            ) -join "`n") 2
    }
}

$repoRoot = Get-RepoRoot

# The repository root as a prefix to test paths against: canonical, and ending
# in a separator so that a sibling directory whose name merely starts with the
# root's ("...\repo-notes" against "...\repo") cannot pass as being inside it.
$repoRootPrefix = [System.IO.Path]::GetFullPath($repoRoot)
if (-not $repoRootPrefix.EndsWith([System.IO.Path]::DirectorySeparatorChar)) {
    $repoRootPrefix += [System.IO.Path]::DirectorySeparatorChar
}

# Reading the manifest, with every failure REPORTED rather than thrown.
#
# This script sets $ErrorActionPreference = 'Stop', so an unguarded
# Resolve-Path or ConvertFrom-Json failure raises a terminating error that
# propagates out and takes the caller's session with it -- exactly what
# Exit-WithMessage exists to avoid, and stated in its own comment above. It also
# surfaces as a PowerShell stack frame naming a line of this script, when the
# thing that is wrong is the manifest. A diagnostic tool whose own failure mode
# needs diagnosing is not doing its job. Raised in the PR #64 review.
#
# Every path below therefore exits 2, the code this script already uses for "the
# manifest or the invocation is wrong", as distinct from 1 for "a sabotage did
# not behave as declared".
if (-not (Test-Path -LiteralPath $Manifest)) {
    Exit-WithMessage "No manifest at: $Manifest" 2
}
$manifestPath = (Resolve-Path -LiteralPath $Manifest).Path
$manifestDir = Split-Path -Parent $manifestPath

try {
    $spec = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
}
catch {
    Exit-WithMessage (@(
            "This manifest is not valid JSON:"
            "  $manifestPath"
            "The parser said:"
            "  $($_.Exception.Message)"
        ) -join "`n") 2
}

# The manifest's shape, checked before anything reads it.
#
# Each field below is documented as required in README-sabotage.md. Under
# Set-StrictMode a missing one otherwise surfaces as "The property 'x' cannot be
# found on this object", naming a line of this script rather than the manifest
# and the field. Verified against both shipped manifests, which satisfy all of
# it, so this rejects nothing that already works.
if (-not ($spec.PSObject.Properties.Name -contains 'sabotages') -or $null -eq $spec.sabotages) {
    Exit-WithMessage (@(
            "This manifest declares no 'sabotages' array:"
            "  $manifestPath"
        ) -join "`n") 2
}
if (@($spec.sabotages).Count -eq 0) {
    # Distinguished from "the -Name filter matched nothing", which is reported
    # separately below: an empty manifest is a manifest problem, and saying
    # "no sabotage matches '*'" about it sends the reader to look at the filter.
    Exit-WithMessage (@(
            "This manifest's 'sabotages' array is empty, so there is nothing to run:"
            "  $manifestPath"
        ) -join "`n") 2
}

# Every timeoutSeconds in the manifest is VALIDATED here, whatever the command
# line said, and the manifest's sweep-wide one is applied only if -TimeoutSeconds
# did not already name a number. Validation and use are deliberately separate:
# gating the check on "we are about to use this" meant a manifest could carry a
# nonsense bound and be told so only on the runs that did not override it, which
# is the run least likely to be the one an author is testing with.
#
# Presence is `$null -ne`, NOT truthiness. PowerShell counts 0 as false, so
# `-and $spec.timeoutSeconds` skipped the whole block for exactly the value the
# error message names -- 0 was silently treated as "not set" and fell back to the
# derived bound, while -5 was rejected, because a non-zero number is truthy.
# Measured both ways. Raised in the PR #64 review.
$timeoutFromCommandLine = $TimeoutSeconds -gt 0
if (($spec.PSObject.Properties.Name -contains 'timeoutSeconds') -and $null -ne $spec.timeoutSeconds) {
    if ([int]$spec.timeoutSeconds -lt 1) {
        Exit-WithMessage (@(
                "This manifest sets timeoutSeconds = $($spec.timeoutSeconds)."
                "It must be at least 1; a bound of zero reports every sabotage as caught."
            ) -join "`n") 2
    }
    # An explicit -TimeoutSeconds still wins: the command line is the more
    # specific statement of intent.
    if ($TimeoutSeconds -eq 0) { $TimeoutSeconds = [int]$spec.timeoutSeconds }
}

foreach ($entry in @($spec.sabotages)) {
    if (($entry.PSObject.Properties.Name -contains 'timeoutSeconds') -and
        $null -ne $entry.timeoutSeconds -and [int]$entry.timeoutSeconds -lt 1) {
        Exit-WithMessage (@(
                "The sabotage '$($entry.name)' sets timeoutSeconds = $($entry.timeoutSeconds)."
                "It must be at least 1; a bound of zero reports it as caught without running."
            ) -join "`n") 2
    }
}

# The sweep supplies --target-dir itself, aiming every build at the working
# copy's own directory. A manifest passing its own would be a duplicate flag,
# which cargo rejects outright -- and the failure would land mid-sweep, after
# the baseline, looking like a build problem. Refused here instead, where the
# message can say what it actually is.
if (($spec.PSObject.Properties.Name -contains 'testArgs') -and $spec.testArgs) {
    foreach ($supplied in @($spec.testArgs)) {
        if ($supplied -eq '--target-dir' -or $supplied -like '--target-dir=*') {
            Exit-WithMessage (@(
                    "This manifest's testArgs passes --target-dir."
                    "The sweep sets that itself, so that every build lands in the working"
                    "copy's own target directory rather than the developer's. Remove it."
                ) -join "`n") 2
        }
    }
}

$hasTestArgs = ($spec.PSObject.Properties.Name -contains 'testArgs') -and $spec.testArgs
if (-not $hasTestArgs -and -not (($spec.PSObject.Properties.Name -contains 'package') -and $spec.package)) {
    Exit-WithMessage (@(
            "This manifest declares no 'package':"
            "  $manifestPath"
            "It is required unless 'testArgs' supplies the cargo command instead."
            "Without it the sweep would run 'cargo test -p  --locked' and fail on"
            "an empty package name well after the baseline started."
        ) -join "`n") 2
}

$position = 0
foreach ($entry in @($spec.sabotages)) {
    $position++
    foreach ($field in @('name', 'file', 'expect', 'why', 'find', 'replace')) {
        if (-not ($entry.PSObject.Properties.Name -contains $field)) {
            $label = if ($entry.PSObject.Properties.Name -contains 'name') {
                "'$($entry.name)'"
            }
            else { "at position $position" }
            Exit-WithMessage (@(
                    "The sabotage $label is missing the required field '$field'."
                    "See the manifest format table in tools/README-sabotage.md."
                ) -join "`n") 2
        }
    }
    if (@('caught', 'survives') -notcontains $entry.expect) {
        # Checked because an unrecognised value is not inert: `expect` is
        # compared for equality when scoring, so anything else can never match
        # and the entry would be reported as misbehaving on every run, whatever
        # the suite actually did.
        Exit-WithMessage (@(
                "The sabotage '$($entry.name)' declares expect = '$($entry.expect)'."
                "It must be 'caught' or 'survives'."
            ) -join "`n") 2
    }
}

# Each sabotage's `file` is resolved against the manifest's own directory, which
# is what a manifest sitting in the crate it sabotages wants. An optional `root`
# redirects that, for a manifest kept somewhere other than the code it patches.
$sourceRoot = $manifestDir
if ($spec.PSObject.Properties.Name -contains 'root' -and $spec.root) {
    $rootCandidate = Join-Path $manifestDir $spec.root
    if (-not (Test-Path -LiteralPath $rootCandidate)) {
        Exit-WithMessage (@(
                "This manifest's 'root' does not exist:"
                "  $($spec.root)"
                "which resolves to:"
                "  $rootCandidate"
                "'root' is relative to the manifest's own directory."
            ) -join "`n") 2
    }
    $sourceRoot = (Resolve-Path -LiteralPath $rootCandidate).Path
}

# Read through the property check rather than directly: validation above allows
# `package` to be absent when `testArgs` supplies the command, and under
# Set-StrictMode reading an absent property throws.
$package = $null
if ($spec.PSObject.Properties.Name -contains 'package') { $package = $spec.package }
$testArgs = @('test', '-p', $package, '--locked')
if ($spec.PSObject.Properties.Name -contains 'testArgs' -and $spec.testArgs) {
    # The manifest may write the vector either way -- with the `test` subcommand
    # or starting at the flags -- and both spellings are in use. Normalising here
    # rather than prepending unconditionally is what keeps a manifest that does
    # include it from producing `cargo test test ...`, in which the second word is
    # not a subcommand but a TESTNAME filter, silently narrowing the sweep to the
    # tests whose path happens to contain "test".
    #
    # That defect went unnoticed because every test in the crates swept so far
    # lives under a `mod tests`, so the accidental filter matched all of them --
    # every baseline recorded "0 filtered out". A crate laid out differently would
    # have run a subset and still reported a clean sweep.
    $supplied = @($spec.testArgs)
    if ($supplied.Count -gt 0 -and $supplied[0] -eq 'test') {
        $supplied = @($supplied | Select-Object -Skip 1)
    }
    $testArgs = @('test') + $supplied
}

$selected = @($spec.sabotages | Where-Object { $_.name -like $Name })

if ($List) {
    Write-Report "Manifest : $manifestPath"
    Write-Report "Package  : $package"
    Write-Report "Command  : cargo $($testArgs -join ' ')"
    Write-Report ''
    $selected | ForEach-Object {
        Write-Report ("{0,-10} {1}" -f $_.expect, $_.name)
    }
    exit 0
}

if ($selected.Count -eq 0) {
    Exit-WithMessage "No sabotage in $manifestPath matches name filter '$Name'." 2
}

# Transcript and backup file names are derived from the sabotage's name, and
# the sanitiser collapses every run of non-alphanumerics to a single dash -- so
# two names differing only in punctuation ("a: b" and "a - b") produce the same
# stem. Sharing a stem means sharing a transcript, losing one entry's evidence,
# and sharing a backup path, which is the more serious half: it would put two
# different files' recovery copies at one location.
#
# Detected here, once, rather than defended against at each write, so the
# message names the real problem -- two manifest entries that cannot be told
# apart -- instead of a stale-file symptom. Computed into a lookup keyed by
# name so the loop below spells the sanitiser in one place only.
$stems = @{}
$stemOwners = @{}

# Stems this directory has already spoken for. `baseline.txt` and its phase
# variants are written before the sweep begins, so a sabotage reducing to
# `baseline` would overwrite the baseline's transcript with its own -- and the
# baseline is the evidence that the suite was green before any patching, which
# is the premise the whole sweep rests on. Compared case-insensitively because
# the sanitiser preserves case while the filesystem does not, so `Baseline` and
# `baseline` are one file here.
$reservedStems = @('baseline')

foreach ($sabotage in $selected) {
    $stem = $sabotage.name -replace '[^A-Za-z0-9]+', '-'
    if ($reservedStems -contains $stem.ToLowerInvariant()) {
        Exit-WithMessage (@(
                "This sabotage's name reduces to a file name stem this directory"
                "already uses for its own transcripts:"
                "  $($sabotage.name)"
                "It becomes '$stem', which would collide with '$stem.txt'. Rename it."
            ) -join "`n") 2
    }
    if ($stemOwners.ContainsKey($stem)) {
        Exit-WithMessage (@(
                "Two sabotages in this manifest reduce to the same file name stem,"
                "so they would share a transcript and a restore backup:"
                "  $($stemOwners[$stem])"
                "  $($sabotage.name)"
                "Both become '$stem'. Rename one so the two differ by more than"
                "punctuation."
            ) -join "`n") 2
    }
    $stemOwners[$stem] = $sabotage.name
    $stems[$sabotage.name] = $stem
}

# Resolve and validate every target before touching anything, so a manifest
# typo cannot leave the tree half-patched.
foreach ($sabotage in $selected) {
    $target = Join-Path $sourceRoot $sabotage.file
    if (-not (Test-Path -LiteralPath $target)) {
        Exit-WithMessage "Sabotage '$($sabotage.name)' names a file that does not exist: $target" 2
    }

    # Containment. The sweep patches inside the copy, so an escape here is no
    # longer a threat to the developer's files -- but a `root` pointing outside
    # the repository still cannot be mapped into the copy at all, and saying so
    # plainly beats letting it fail later as a missing file in the tree.
    $targetFull = [System.IO.Path]::GetFullPath($target)
    if (-not $targetFull.StartsWith($repoRootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        Exit-WithMessage (@(
                "Sabotage '$($sabotage.name)' names a file outside this repository:"
                "  $targetFull"
                "The repository is rooted at:"
                "  $repoRoot"
                "The sweep runs against a copy of this repository, so a file outside it"
                "has no place in the copy to be patched."
            ) -join "`n") 2
    }
}

# --- Everything from here on is sweep setup, and none of it runs under -List.
#
# The output directory is CREATED here rather than earlier because -List must be
# inert: it patches nothing, so it has no business creating directories or
# copying a tree. Raised in the PR #64 review.
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repoRoot '.scratch\sabotage' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

# The working copy must be somewhere git ignores, and this is not a tidiness
# rule. Sync-Tree enumerates the source through git, so what keeps the copy out
# of its own enumeration is only that `.scratch/` is ignored -- which covers the
# DEFAULT and nothing else. Pointed at a tracked-or-ignorable location inside
# the repository, the second run would enumerate the first run's copy and nest
# it one level deeper, marking every nested file as wanted so the deletion pass
# preserves the lot. Checked rather than documented, because the failure grows
# without bound and looks like nothing until it does.
$outputFull = [System.IO.Path]::GetFullPath($OutputDirectory)
if ($outputFull.StartsWith($repoRootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    Invoke-Native { git -C $repoRoot check-ignore -q -- $outputFull } | Out-Null
    if ($LASTEXITCODE -ne 0) {
        Exit-WithMessage (@(
                "-OutputDirectory is inside the repository but git does not ignore it:"
                "  $outputFull"
                "The working copy lives there, and it is kept out of its own enumeration"
                "only by being ignored -- so an un-ignored location makes each run copy"
                "the previous run's copy one level deeper. Point it outside the"
                "repository, or add it to .gitignore."
            ) -join "`n") 2
    }
}

# The copy the sweep actually patches, and the cargo target directory that
# serves it. Both persist between runs: the copy so that Sync-Tree has something
# to compare against and can leave unchanged files alone, and the target
# directory so the builds stay warm. Neither is ever the real tree's.
#
# The target directory is deliberately a SIBLING of the copy rather than inside
# it, so that Sync-Tree's deletion of files the source no longer has cannot walk
# into it, and so `cargo clean` semantics stay the developer's business.
$treeRoot = Join-Path $OutputDirectory 'tree'
$targetRoot = Join-Path $OutputDirectory 'target'
New-Item -ItemType Directory -Force -Path $treeRoot | Out-Null
New-Item -ItemType Directory -Force -Path $targetRoot | Out-Null

Write-Report 'Refreshing the working copy.' -Level note
Sync-Tree -From $repoRoot -To $treeRoot

# Every cargo invocation from here on builds the COPY, into the copy's own
# target directory, passed as --target-dir on each command line.
#
# NOT through $env:CARGO_TARGET_DIR, which an earlier revision used and which was
# wrong twice over. A .ps1 runs IN the caller's PowerShell process, so assigning
# $env: there mutates the developer's session and outlives the script -- every
# later cargo command in that session, in this repository or any other, would
# have built into this scratch directory. Measured: the variable is still set in
# the calling session after the script exits. And the safety property the old
# comment claimed was backwards: cargo's precedence is CLI flag OVER environment
# variable, so a `testArgs` carrying --target-dir would have overridden the
# environment, not been overridden by it. Measured that too.
#
# On the command line the precedence runs the right way: this wins over any
# CARGO_TARGET_DIR the developer already has set. The one thing it cannot
# survive is a manifest that also passes --target-dir, which cargo rejects as a
# duplicate -- so that is refused up front rather than left to fail mid-sweep.
$sweepTargetDir = [System.IO.Path]::GetFullPath($targetRoot)
$testArgs = Add-CargoFlag -CargoArgs $testArgs -Flag @('--target-dir', $sweepTargetDir)

# Targets are addressed in the copy from here on. The manifest still resolves
# them against the real tree -- that is where its `root` and `file` mean
# something -- and each is then re-based onto the copy by its path relative to
# the repository root, using $repoRootPrefix at the point of use.

# Clear the transcripts this run may write -- and only those.
#
# The goal is that a path named in an error message is always from this run or
# absent: a run that fails in the build phase never creates the test-phase
# transcript, so a survivor from an earlier green sweep would otherwise still be
# sitting exactly where the abort message points, contradicting it.
#
# This used to delete every file in $OutputDirectory, which is a destructive
# surprise given that the directory is caller-supplied -- pointed at somewhere
# holding anything else, the sweep would take it. It now removes precisely the
# paths this invocation can write, which achieves the same goal with no
# collateral at all. Raised in the PR #64 review.
#
# Placed after the -List exit and after validation, so neither listing a
# manifest nor being rejected by it deletes anything.
$writableStems = @('baseline') + @($selected | ForEach-Object { $stems[$_.name] })
foreach ($writableStem in $writableStems) {
    foreach ($suffix in @('.txt', '.txt.err', '.txt.build', '.txt.build.err')) {
        $stale = Join-Path $OutputDirectory ($writableStem + $suffix)
        if (Test-Path -LiteralPath $stale) {
            Remove-Item -LiteralPath $stale -Force -ErrorAction SilentlyContinue
        }
    }
}

Write-Report 'Baseline: running the unmodified suite.' -Level note
$baselinePath = Join-Path $OutputDirectory 'baseline.txt'

# The baseline is bounded by the BUILD budget rather than by the test budget it
# is about to calibrate. There is nothing yet to derive a test bound from, and
# the baseline is the one run that is known not to be sabotaged, so the risk of
# giving it room is the opposite of the risk everywhere else.
$baseline = Invoke-Sabotaged -CargoArgs $testArgs -WorkingDirectory $treeRoot `
    -TranscriptPath $baselinePath -BuildSeconds $BuildTimeoutSeconds -TestSeconds $BuildTimeoutSeconds

if ($baseline.Outcome -ne 'passed') {
    Exit-WithMessage (@(
            "The baseline suite did not pass ($($baseline.Outcome))."
            "Transcript: $(Get-EvidencePath -Outcome $baseline.Outcome -TranscriptPath $baselinePath)"
            "A sweep against a red suite reports every sabotage as caught and proves"
            "nothing while looking like a clean bill of health. Fix the suite first."
        ) -join "`n") 2
}

# What a sabotaged run is allowed to take before it is called hung.
#
# Derived from the baseline unless the caller named a number: the baseline just
# measured how long this suite legitimately takes ON THIS MACHINE, which is a
# better answer than any constant compiled into this file. A fixed default is
# wrong in both directions -- too tight on a loaded machine or a big suite,
# where a slow-but-finite run is scored as CAUGHT and quietly inflates the
# result; too loose on a fast one, where every hang costs the difference. Hangs
# dominate the wall clock, so that difference is most of the sweep: 720 of 853
# seconds on the 39-entry manifest at the old fixed 60.
#
# The figure used is the baseline's TEST phase alone. $baseline.Seconds carries
# that and not build-plus-test, and the distinction is load bearing: the bound
# governs test execution, so anything else imports a cost it does not govern --
# and the build is the volatile part, since the first run against a fresh copy
# pays a cold build and would inflate the bound several-fold on the one run
# least able to judge what is normal. Measured on the placement-probe manifest:
# 25s for build-plus-test against a cold copy, 3-4s for the tests alone.
$baselineSeconds = [Math]::Max(1, $baseline.Seconds)
if ($TimeoutSeconds -gt 0) {
    $defaultTimeout = $TimeoutSeconds
    # Named for where it actually came from. Reporting "-TimeoutSeconds" for a
    # bound the MANIFEST set would credit a flag the runner never passed, and
    # send anyone trying to change it to the wrong place.
    $timeoutSource = if ($timeoutFromCommandLine) { '-TimeoutSeconds' }
    else { "the manifest's timeoutSeconds" }
}
else {
    $defaultTimeout = [Math]::Max($TimeoutFloorSeconds, $TimeoutMultiplier * $baselineSeconds)
    $timeoutSource = "${TimeoutMultiplier}x the ${baselineSeconds}s baseline, floor ${TimeoutFloorSeconds}s"
}
Write-Report "Baseline is green in ${baselineSeconds}s. Hang bound: ${defaultTimeout}s ($timeoutSource)." -Level note
Write-Report 'Sweeping.' -Level note
Write-Report ''

$results = @()

foreach ($sabotage in $selected) {
    # The file to patch is the COPY's, reached by the real target's path
    # relative to the repository root. The manifest keeps meaning what it always
    # meant -- `root` and `file` still resolve against the real tree -- and only
    # the file that actually gets written changes.
    $realTarget = [System.IO.Path]::GetFullPath((Join-Path $sourceRoot $sabotage.file))
    $target = Join-Path $treeRoot $realTarget.Substring($repoRootPrefix.Length)
    if (-not (Test-Path -LiteralPath $target -PathType Leaf)) {
        # Reachable when the manifest names a file git does not list -- an
        # ignored one, say -- so it was never copied. Reported rather than
        # crashing on the read below.
        $results += [pscustomobject]@{
            Sabotage = $sabotage.name; Expected = $sabotage.expect
            Actual   = 'NOT IN THE WORKING COPY: git does not track or list this file'
            Ok       = $false; Patch = ''
        }
        continue
    }

    $find = ($sabotage.find -join "`n")
    $replace = ($sabotage.replace -join "`n")
    $original = [System.IO.File]::ReadAllText($target)

    # Exactly once, not at least once: a pattern matching two sites patches
    # whichever the string replace happens to reach, and the sabotage is then
    # not the one described.
    $occurrences = ([regex]::Matches($original, [regex]::Escape($find))).Count
    if ($occurrences -ne 1) {
        $results += [pscustomobject]@{
            Sabotage = $sabotage.name; Expected = $sabotage.expect
            Actual   = "MANIFEST STALE: pattern found $occurrences times, expected 1"
            Ok       = $false; Patch = (Format-Patch -Find $find -Replace $replace)
        }
        continue
    }

    $patched = $original.Replace($find, $replace)
    if ($patched -eq $original) {
        $results += [pscustomobject]@{
            Sabotage = $sabotage.name; Expected = $sabotage.expect
            Actual   = 'MANIFEST INERT: the patch does not change the file'
            Ok       = $false; Patch = (Format-Patch -Find $find -Replace $replace)
        }
        continue
    }

    # The stem is the one computed and collision-checked up front, not a second
    # spelling of the sanitiser: two copies could disagree, and the check would
    # then be guarding a name the writes do not use.
    $stem = $stems[$sabotage.name]
    $transcript = Join-Path $OutputDirectory ($stem + '.txt')

    # A single entry may buy itself more room. This is the case neither the
    # derived bound nor a global override can express: one sabotage whose run
    # legitimately takes far longer than the rest, where raising the bound for
    # the whole sweep would pay that cost on every other entry too. It only ever
    # raises -- a per-entry value below the sweep's bound is ignored, because the
    # reason to lower one is speed and the cost of being wrong about it is a
    # false `caught`.
    $entryTimeout = $defaultTimeout
    # `$null -ne` rather than truthiness, for the reason given where these are
    # validated: 0 is false in PowerShell, so a truthiness test reads an
    # explicit 0 as absent. Validation has already rejected 0 by this point, so
    # this is consistency rather than a second guard.
    if (($sabotage.PSObject.Properties.Name -contains 'timeoutSeconds') -and $null -ne $sabotage.timeoutSeconds) {
        $entryTimeout = [Math]::Max($defaultTimeout, [int]$sabotage.timeoutSeconds)
    }

    try {
        # Inside the guarded region, not before it, so a write that throws
        # part-way through still reaches the reset below and the next sabotage
        # starts from unmodified source.
        [System.IO.File]::WriteAllText($target, $patched, $utf8NoBom)
        $run = Invoke-Sabotaged -CargoArgs $testArgs -WorkingDirectory $treeRoot `
            -TranscriptPath $transcript -BuildSeconds $BuildTimeoutSeconds -TestSeconds $entryTimeout
    }
    finally {
        # Reset the copy from the REAL file, which is the authority, rather than
        # from a backup this script would otherwise have to write, verify and
        # account for. There is no data at risk here: the file being rewritten
        # belongs to a scratch copy, so the worst case of getting this wrong is
        # a stale copy that the next run's Sync-Tree corrects, or that the
        # developer fixes by deleting a directory.
        #
        # WriteAllBytes rather than Copy-Item, for the same reason Sync-Tree
        # uses it: it is byte-exact AND stamps the file now, so cargo rebuilds.
        # Copy-Item would carry the real file's older timestamp across, cargo
        # would judge the crate up to date against an artifact built from the
        # PATCHED source, and the sabotaged binary would survive into the next
        # sabotage's run -- measured, in the in-place design this replaced.
        [System.IO.File]::WriteAllBytes($target, [System.IO.File]::ReadAllBytes($realTarget))
    }

    $actual = switch ($run.Outcome) {
        'passed' { 'survived (NOT caught)' }
        'failed' { "caught (suite failed, exit $($run.Code))" }
        'hung' { "caught (tests HUNG past ${entryTimeout}s)" }
        # Not "caught": the tests never ran, so this says nothing about them.
        # It means the patch is not valid Rust -- a manifest problem to fix,
        # not a result to record.
        'build-failed' { 'MANIFEST DOES NOT COMPILE (tests never ran)' }
        'build-hung' { "BUILD HUNG past ${BuildTimeoutSeconds}s (tests never ran)" }
        # Same category as build-failed, reached one phase later because
        # doctests cannot be built by the build phase. The tests did run, but
        # the one that "failed" failed to compile, so it detected nothing.
        'doc-compile-failed' { 'MANIFEST DOES NOT COMPILE (a doctest would not build)' }
    }
    $ok = switch ($run.Outcome) {
        'passed' { $sabotage.expect -eq 'survives' }
        'failed' { $sabotage.expect -eq 'caught' }
        'hung' { $sabotage.expect -eq 'caught' }
        default { $false }
    }

    $results += [pscustomobject]@{
        Sabotage = $sabotage.name; Expected = $sabotage.expect
        Actual   = $actual; Ok = $ok; Patch = (Format-Patch -Find $find -Replace $replace)
    }

    $level = if ($ok) { 'good' } else { 'bad' }
    Write-Report ("{0,-58} {1}" -f $sabotage.name, $actual) -Level $level
}

Write-Report ''
$results | Select-Object Sabotage, Expected, Actual, Ok | Format-Table -AutoSize -Wrap |
    Out-String | ForEach-Object { $_.TrimEnd("`r", "`n") } | Write-Report

$unexpected = @($results | Where-Object { -not $_.Ok })
if ($unexpected.Count -eq 0) {
    Write-Report "All $($results.Count) sabotages behaved as declared." -Level good
    exit 0
}

Write-Report ''
Write-Report 'UNEXPECTED RESULTS -- read the patch before concluding the tests have a hole.' -Level bad
Write-Report 'A sabotage that does not actually break anything will be survived for an honest reason.' -Level bad
foreach ($result in $unexpected) {
    Write-Report ''
    Write-Report "  $($result.Sabotage)" -Level bad
    Write-Report "    expected $($result.Expected), got: $($result.Actual)"
    $result.Patch | Write-Report
}
Write-Report ''
Exit-WithMessage "$($unexpected.Count) of $($results.Count) sabotages did not behave as declared." 1
