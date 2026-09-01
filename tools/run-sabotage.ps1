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
    the tests would have noticed it.

.PARAMETER OutputDirectory
    Where to write per-sabotage transcripts. Defaults to .scratch/sabotage.

.PARAMETER List
    Print the manifest's sabotages and exit without running anything.

.PARAMETER AllowDirty
    Permit running when a target file has uncommitted changes. Off by default:
    the script restores files by rewriting their pre-sabotage contents, and if
    it is interrupted, a clean starting tree is what makes the damage obvious
    and recoverable with `git checkout`.

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
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'note', 'good', 'bad')][string] $Level = 'info'
    )
    $colour = switch ($Level) {
        'note' { 'Cyan' }
        'good' { 'Green' }
        'bad' { 'Red' }
        default { 'Gray' }
    }
    Write-Host $Message -ForegroundColor $colour
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
    if ($LASTEXITCODE -ne 0) { throw 'Not inside a git repository.' }
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

    if ($process.WaitForExit($Seconds * 1000)) {
        return [pscustomobject]@{ Outcome = ($process.ExitCode -eq 0 ? 'passed' : 'failed'); Code = $process.ExitCode }
    }

    Stop-Tree -ProcessId $process.Id
    return [pscustomobject]@{ Outcome = 'hung'; Code = $null }
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

    $build = Invoke-Bounded -CargoArgs ($CargoArgs + '--no-run') -WorkingDirectory $WorkingDirectory `
        -TranscriptPath "$TranscriptPath.build" -Seconds $BuildSeconds
    if ($build.Outcome -eq 'failed') {
        return [pscustomobject]@{ Outcome = 'build-failed'; Code = $build.Code }
    }
    if ($build.Outcome -eq 'hung') {
        return [pscustomobject]@{ Outcome = 'build-hung'; Code = $null }
    }

    return Invoke-Bounded -CargoArgs $CargoArgs -WorkingDirectory $WorkingDirectory `
        -TranscriptPath $TranscriptPath -Seconds $TestSeconds
}

function Format-Patch {
    param([string] $Find, [string] $Replace)
    $lines = @('    --- injected patch ---')
    foreach ($line in $Find -split "`n") { $lines += "    - $line" }
    foreach ($line in $Replace -split "`n") { $lines += "    + $line" }
    if ([string]::IsNullOrEmpty($Replace)) { $lines += '    + (removed)' }
    return $lines -join "`n"
}

$repoRoot = Get-RepoRoot
$manifestPath = (Resolve-Path -LiteralPath $Manifest).Path
$manifestDir = Split-Path -Parent $manifestPath
$spec = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

# Each sabotage's `file` is resolved against the manifest's own directory, which
# is what a manifest sitting in the crate it sabotages wants. An optional `root`
# redirects that, for a manifest kept somewhere other than the code it patches.
$sourceRoot = $manifestDir
if ($spec.PSObject.Properties.Name -contains 'root' -and $spec.root) {
    $sourceRoot = (Resolve-Path -LiteralPath (Join-Path $manifestDir $spec.root)).Path
}

if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repoRoot '.scratch\sabotage' }
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null

$package = $spec.package
$testArgs = @('test', '-p', $package, '--locked')
if ($spec.PSObject.Properties.Name -contains 'testArgs' -and $spec.testArgs) {
    $testArgs = @('test') + $spec.testArgs
}

$selected = @($spec.sabotages | Where-Object { $_.name -like $Name })

if ($List) {
    "Manifest : $manifestPath"
    "Package  : $package"
    "Command  : cargo $($testArgs -join ' ')"
    ''
    $selected | ForEach-Object {
        "{0,-10} {1}" -f $_.expect, $_.name
    }
    exit 0
}

if ($selected.Count -eq 0) {
    Exit-WithMessage "No sabotage in $manifestPath matches name filter '$Name'." 2
}

# Resolve and validate every target before touching anything, so a manifest
# typo cannot leave the tree half-patched.
foreach ($sabotage in $selected) {
    $target = Join-Path $sourceRoot $sabotage.file
    if (-not (Test-Path -LiteralPath $target)) {
        Exit-WithMessage "Sabotage '$($sabotage.name)' names a file that does not exist: $target" 2
    }
    if (-not $AllowDirty) {
        $status = git -C $repoRoot status --porcelain -- $target
        if ($status) {
            Exit-WithMessage (@(
                    "Sabotage targets must be clean in git, and this one is not:"
                    "  $target"
                    "This script restores files by rewriting their previous contents; starting"
                    "from a clean tree is what makes an interrupted run recoverable with a"
                    "'git checkout'. Commit or stash first, or pass -AllowDirty to accept that risk."
                ) -join "`n") 2
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
            "Transcript: $baselinePath"
            "A sweep against a red suite reports every sabotage as caught and proves"
            "nothing while looking like a clean bill of health. Fix the suite first."
        ) -join "`n") 2
}
Write-Report 'Baseline is green. Sweeping.' -Level note
''

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

    $transcript = Join-Path $OutputDirectory ((($sabotage.name -replace '[^A-Za-z0-9]+', '-')) + '.txt')
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
        [System.IO.File]::WriteAllText($target, $original, $utf8NoBom)
        if ([System.IO.File]::ReadAllText($target) -ne $original) {
            Exit-WithMessage "FAILED TO RESTORE $target -- recover it with 'git checkout -- $target' before doing anything else." 3
        }
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

''
$results | Select-Object Sabotage, Expected, Actual, Ok | Format-Table -AutoSize -Wrap

$unexpected = @($results | Where-Object { -not $_.Ok })
if ($unexpected.Count -eq 0) {
    Write-Report "All $($results.Count) sabotages behaved as declared." -Level good
    exit 0
}

''
Write-Report 'UNEXPECTED RESULTS -- read the patch before concluding the tests have a hole.' -Level bad
Write-Report 'A sabotage that does not actually break anything will be survived for an honest reason.' -Level bad
foreach ($result in $unexpected) {
    ''
    Write-Report "  $($result.Sabotage)" -Level bad
    Write-Report "    expected $($result.Expected), got: $($result.Actual)"
    $result.Patch
}
''
Exit-WithMessage "$($unexpected.Count) of $($results.Count) sabotages did not behave as declared." 1
