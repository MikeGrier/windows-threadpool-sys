# Copyright (c) 2026 Mike Grier. All rights reserved.
<#
.SYNOPSIS
    Runs a sabotage sweep: injects deliberate defects one at a time and reports
    whether the test suite noticed.

.DESCRIPTION
    A green suite is evidence that the code passes its tests. It is not evidence
    that the tests would fail if the code were wrong, and those are different
    claims. This script measures the second one: for each defect in a manifest
    it patches the source, runs the suite, restores the source, and records
    whether the suite went red.

    Three rules are encoded here because each was learned by getting it wrong.

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

    Expect one full rebuild per sabotage. This is a deliberate, occasional
    instrument -- run it when a guard is written or changed, not on every
    commit.

.PARAMETER Manifest
    Path to a sabotage manifest (JSON). See tools/README-sabotage.md for the
    format, and crates/windows-waitable-queues/sabotage.json for a worked
    example.

.PARAMETER Name
    Optional wildcard filter over sabotage names, to re-run just one.

.PARAMETER TimeoutSeconds
    Bound on TEST EXECUTION only, defaulting to 60. A run that exceeds it is
    killed and counted as caught, because a lost wakeup hangs rather than
    fails.

    This is deliberately separate from -BuildTimeoutSeconds, and the split is
    what makes a tight bound safe here. Because a timeout counts as caught, a
    bound shorter than legitimate work manufactures a FALSE "caught" -- it
    credits the tests with detecting a defect they never ran against, which is
    the dangerous direction to be wrong in. Under a single combined bound the
    number had to be generous enough for the slowest imaginable cold build,
    which made every genuinely-hanging sabotage cost that same generous number.

    Measured on this workspace: building the crate after a one-file edit takes
    under a second, and test execution takes about twelve, nearly all of it
    compiling doctests -- `cargo test --no-run` does not build those, and Cargo
    offers no `--doc --no-run` to pre-pay it. The default therefore leaves
    roughly five times headroom over the measured cost. Raise it for a
    substantially slower machine or a much larger suite; a sweep whose result
    you intend to believe should never be run with this tightened for speed.

.PARAMETER BuildTimeoutSeconds
    Bound on the build phase, defaulting to 300. Generous on purpose: a slow
    cold build must never be mistaken for a hang, and it costs nothing when
    builds are fast. A sabotage that fails to build is reported as such rather
    than as caught -- the compiler rejecting a patch says nothing about whether
    the tests would have noticed it. The same holds for a patch that only breaks
    a doctest: those cannot be built in this phase, so they surface during the
    run and are reclassified there rather than being counted as caught.

.PARAMETER OutputDirectory
    Where to write per-sabotage transcripts. Defaults to .scratch/sabotage.
    Stale transcripts are cleared at startup, and pre-patch copies of every
    target are kept under its `restore/` subdirectory until their restore is
    verified.

.PARAMETER List
    Print the manifest's sabotages and exit without running anything.

.PARAMETER AllowDirty
    Permit running when a target file has uncommitted changes. Off by default:
    a clean starting tree makes the damage from an interrupted run obvious, and
    makes `git checkout` a safe second recourse. It is not safe once a target
    carries uncommitted work, so under this switch the pre-patch copy written to
    the output directory is the only correct recovery -- which is what the
    script's restore-failure message names.

.OUTPUTS
    Exits 0 only if every sabotage matched its declared expectation.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Manifest,

    [string] $Name = '*',

    [int] $TimeoutSeconds = 60,

    [int] $BuildTimeoutSeconds = 300,

    [string] $OutputDirectory,

    [switch] $List,

    [switch] $AllowDirty
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

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
    $root = git rev-parse --show-toplevel 2>$null
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

    $process = Start-Process -FilePath 'cargo' -ArgumentList $CargoArgs `
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

    if ($process.WaitForExit($Seconds * 1000)) {
        # Spelled as an if/else rather than a ternary on purpose: `? :` is
        # PowerShell 7 syntax, and this is a PARSE error under Windows
        # PowerShell 5.1 -- so a single ternary anywhere makes the whole script
        # unrunnable on the shell that `powershell.exe` still starts by default,
        # failing before the first line executes rather than at this line. The
        # three sibling scripts in this directory are 5.1-clean; this one stays
        # that way too. Raised in the PR #64 review.
        $outcome = if ($process.ExitCode -eq 0) { 'passed' } else { 'failed' }
        return [pscustomobject]@{ Outcome = $outcome; Code = $process.ExitCode }
    }

    Stop-Tree -ProcessId $process.Id
    return [pscustomobject]@{ Outcome = 'hung'; Code = $null }
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
    param([string[]] $CargoArgs, [string] $Flag)

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
        return [pscustomobject]@{ Outcome = 'build-failed'; Code = $build.Code }
    }
    if ($build.Outcome -eq 'hung') {
        return [pscustomobject]@{ Outcome = 'build-hung'; Code = $null }
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
            return [pscustomobject]@{ Outcome = 'doc-compile-failed'; Code = $run.Code }
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

# The two bounds reach [Process]::WaitForExit($Seconds * 1000), which does not
# mean what a caller might hope at or below zero -- and gets it wrong in the
# dangerous direction. Zero returns immediately without waiting, so every phase
# would be classified as a hang and every sabotage would read as `caught`: a
# fully green sweep that proved nothing, which is the one outcome this tool
# exists to make impossible. A negative value throws a MethodInvocationException
# instead, breaking the report-don't-throw contract. Both are measured, and both
# are rejected here rather than allowed to reach the wait. Raised in the PR #64
# review.
foreach ($bound in @(
        @{ Name = 'TimeoutSeconds'; Value = $TimeoutSeconds },
        @{ Name = 'BuildTimeoutSeconds'; Value = $BuildTimeoutSeconds })) {
    if ($bound.Value -lt 1) {
        Exit-WithMessage (@(
                "-$($bound.Name) must be at least 1 second, and was $($bound.Value)."
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

    # Containment, checked for every target and NOT waived by -AllowDirty.
    #
    # A manifest's `root` may point anywhere, so without this the tool will
    # cheerfully patch a file outside the repository. -AllowDirty is documented
    # as waiving the CLEANLINESS requirement; letting it also widen what may be
    # modified conflates two separate things, and the second is the one with no
    # `git checkout` behind it. Checked before the dirtiness query rather than
    # after, so an out-of-repo path is reported as what it is instead of as a
    # git pathspec failure. Raised in the PR #64 review.
    $targetFull = [System.IO.Path]::GetFullPath($target)
    if (-not $targetFull.StartsWith($repoRootPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
        Exit-WithMessage (@(
                "Sabotage '$($sabotage.name)' names a file outside this repository:"
                "  $targetFull"
                "The repository is rooted at:"
                "  $repoRoot"
                "A sweep only ever patches files it can also restore and reason about,"
                "so this is refused whether or not -AllowDirty was passed -- that switch"
                "waives the cleanliness check, not the boundary."
            ) -join "`n") 2
    }

    if (-not $AllowDirty) {
        # An absolute pathspec is fine -- git resolves it against the repository
        # root, so this matches regardless of the caller's working directory.
        # What is NOT fine is assuming the query succeeded: git reports an
        # unusable pathspec (a manifest `root` pointing outside the repository,
        # say) on stderr and exits non-zero, leaving $status empty -- which is
        # indistinguishable from "the file is clean". A guard that cannot tell
        # "clean" from "I could not check" is not a guard, so the exit code is
        # inspected rather than the output alone.
        $status = git -C $repoRoot status --porcelain -- $target 2>&1
        if ($LASTEXITCODE -ne 0) {
            Exit-WithMessage (@(
                    "Could not determine whether this sabotage target is clean in git:"
                    "  $target"
                    "git exited $LASTEXITCODE and said:"
                    "  $status"
                    "Refusing to proceed: a failed check is not a clean result, and"
                    "treating it as one is how a sweep overwrites uncommitted work."
                ) -join "`n") 2
        }
        if ($status) {
            Exit-WithMessage (@(
                    "Sabotage targets must be clean in git, and this one is not:"
                    "  $target"
                    "This script restores files by rewriting their previous contents, and keeps"
                    "a pre-patch copy under the output directory in case that fails. Starting"
                    "from a clean tree additionally makes 'git checkout' a safe second recourse,"
                    "which it is not once a file carries uncommitted work. Commit or stash"
                    "first, or pass -AllowDirty to proceed with the backup as the only recourse."
                ) -join "`n") 2
        }
    }
}

# --- Everything from here on is sweep setup, and none of it runs under -List.
#
# The output directory is CREATED here rather than earlier, and the leftover
# check made below rather than above, because -List must be inert: it patches
# nothing, so it has no business creating directories, and it must stay usable
# for inspecting a manifest even while an interrupted run's backups are waiting
# to be dealt with. Being blocked from reading a manifest by a recovery file is
# precisely when you would want to read it. The README says listing writes and
# deletes nothing; this ordering is what makes that true. Raised in the PR #64
# review.
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repoRoot '.scratch\sabotage' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

# Pre-patch copies live in their own subdirectory, so the clearing of stale
# transcripts below cannot reach them, and so a leftover here is unambiguous.
$backupDirectory = Join-Path $OutputDirectory 'restore'
New-Item -ItemType Directory -Force -Path $backupDirectory | Out-Null

# A leftover backup means the previous run did not get to restore its target,
# so that copy may be the only surviving version of the file -- and under
# -AllowDirty it may hold uncommitted work that exists nowhere else. Refusing
# to start is what makes the "a file here means an interrupted run" claim load
# bearing: without it, the next sweep would quietly overwrite the evidence it
# tells the reader to look for.
$leftover = @(Get-ChildItem -LiteralPath $backupDirectory -File -ErrorAction SilentlyContinue)
if ($leftover.Count -gt 0) {
    Exit-WithMessage (@(
            "Pre-patch backups from an earlier run are still present:"
            ($leftover | ForEach-Object { "  $($_.FullName)" })
            "That run was interrupted before it could restore its target, so each of"
            "these may be the only copy of the file it names -- under -AllowDirty,"
            "including uncommitted work that is in no commit. Compare each against its"
            "target and copy it back if the target is still sabotaged, then delete it."
            "This sweep will not start while they are here, because it would overwrite"
            "them. Listing the manifest with -List still works meanwhile."
        ) -join "`n") 2
}

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
$baseline = Invoke-Sabotaged -CargoArgs $testArgs -WorkingDirectory $repoRoot `
    -TranscriptPath $baselinePath -BuildSeconds $BuildTimeoutSeconds -TestSeconds $TimeoutSeconds

if ($baseline.Outcome -ne 'passed') {
    Exit-WithMessage (@(
            "The baseline suite did not pass ($($baseline.Outcome))."
            "Transcript: $(Get-EvidencePath -Outcome $baseline.Outcome -TranscriptPath $baselinePath)"
            "A sweep against a red suite reports every sabotage as caught and proves"
            "nothing while looking like a clean bill of health. Fix the suite first."
        ) -join "`n") 2
}
Write-Report 'Baseline is green. Sweeping.' -Level note
Write-Report ''

$results = @()

foreach ($sabotage in $selected) {
    $target = Join-Path $sourceRoot $sabotage.file
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

    # The pre-patch contents, on disk and not only in $original.
    #
    # $original is a variable, so it dies with the process: an interruption --
    # Ctrl+C, a crash, Stop-Process -- leaves the file patched with no in-memory
    # copy to put back. `git checkout` recovers that only when the file was
    # clean to begin with, which is precisely what -AllowDirty waives. Writing
    # the backup first is what lets the restore advice below be non-destructive
    # in both modes rather than only in the default one.
    $backup = Join-Path $backupDirectory ($stem + '.' + (Split-Path -Leaf $target) + '.bak')
    # Copied byte-for-byte rather than written from the decoded string. The
    # decoded round trip is not lossless: ReadAllText strips a UTF-8 BOM and
    # WriteAllText with $utf8NoBom does not put it back, so restoring a BOM'd
    # file that way silently drops three bytes -- and the old verification,
    # which compared decoded TEXT, passed anyway and then deleted the backup.
    # Measured: a 23-byte BOM'd file came back 20 bytes with the comparison
    # still reporting success. No tracked file here carries a BOM today (570
    # checked, 0 found), but a manifest may point at any file, and "restores
    # what it patched" has to mean the bytes. Raised in the PR #64 review.
    [System.IO.File]::WriteAllBytes($backup, [System.IO.File]::ReadAllBytes($target))

    try {
        # Inside the guarded region, not before it. A write that throws part-way
        # through -- having already truncated the file -- would otherwise never
        # reach the `finally` that restores it, and the tool's whole promise is
        # that it leaves the tree as it found it.
        [System.IO.File]::WriteAllText($target, $patched, $utf8NoBom)
        $run = Invoke-Sabotaged -CargoArgs $testArgs -WorkingDirectory $repoRoot `
            -TranscriptPath $transcript -BuildSeconds $BuildTimeoutSeconds -TestSeconds $TimeoutSeconds
    }
    finally {
        # Restored from the byte copy, and verified by hashing both files
        # rather than by comparing decoded text -- see the note above the
        # backup. The patched state is still written as decoded text, which is
        # fine: it is transient and only has to compile, and this restore puts
        # the original bytes back regardless of what the patch write did.
        #
        # WriteAllBytes, NOT File.Copy, and the difference is not stylistic.
        # File.Copy preserves the SOURCE's LastWriteTime, so a restored file
        # would carry its original timestamp -- older than the artifact cargo
        # had just built from the patched source. Cargo compares those mtimes,
        # concludes the crate is up to date, and leaves the SABOTAGED binary in
        # the build cache: the next `cargo test`, by this tool or by a person,
        # then runs against sabotaged code. Measured while writing this commit,
        # by doing exactly that and watching an unrelated test fail on a clean
        # tree until the source was touched. WriteAllBytes stamps the file now,
        # so cargo rebuilds.
        [System.IO.File]::WriteAllBytes($target, [System.IO.File]::ReadAllBytes($backup))
        $targetHash = (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash
        $backupHash = (Get-FileHash -LiteralPath $backup -Algorithm SHA256).Hash
        if ($targetHash -ne $backupHash) {
            Exit-WithMessage (@(
                    "FAILED TO RESTORE $target"
                    "Its pre-sabotage contents are saved at:"
                    "  $backup"
                    "Copy that file back over the target before doing anything else."
                    "Do NOT reach for 'git checkout' unless the target was clean when this"
                    "sweep started: under -AllowDirty it was not, and reverting to HEAD would"
                    "discard the uncommitted work that the backup above still holds."
                ) -join "`n") 3
        }
        # The restore is verified, so the backup has served its purpose. Removing
        # it is what makes a file left behind in that directory meaningful: it
        # can then only be from a run that was interrupted before it restored.
        Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
    }

    $actual = switch ($run.Outcome) {
        'passed' { 'survived (NOT caught)' }
        'failed' { "caught (suite failed, exit $($run.Code))" }
        'hung' { "caught (tests HUNG past ${TimeoutSeconds}s)" }
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
