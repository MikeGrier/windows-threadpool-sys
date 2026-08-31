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
    Per-sabotage bound covering build and test. Defaults to 300. A run that
    exceeds it is killed and counted as caught.

    Err generous rather than tight. Because a timeout counts as caught, a bound
    shorter than a legitimate build-and-test manufactures a FALSE "caught" --
    it credits the tests with detecting a defect they never even ran against,
    which is the dangerous direction to be wrong in. A too-long bound only
    wastes time on the sabotages that genuinely hang. Lower it deliberately
    when iterating on one sabotage; leave it alone for a sweep whose result
    you intend to believe.

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

    [int] $TimeoutSeconds = 300,

    [string] $OutputDirectory,

    [switch] $List,

    [switch] $AllowDirty
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

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

Write-Host 'Baseline: running the unmodified suite.' -ForegroundColor Cyan
$baselinePath = Join-Path $OutputDirectory 'baseline.txt'
$baseline = Invoke-Bounded -CargoArgs $testArgs -WorkingDirectory $repoRoot `
    -TranscriptPath $baselinePath -Seconds $TimeoutSeconds

if ($baseline.Outcome -ne 'passed') {
    Exit-WithMessage (@(
            "The baseline suite did not pass ($($baseline.Outcome))."
            "Transcript: $baselinePath"
            "A sweep against a red suite reports every sabotage as caught and proves"
            "nothing while looking like a clean bill of health. Fix the suite first."
        ) -join "`n") 2
}
Write-Host 'Baseline is green. Sweeping.' -ForegroundColor Cyan
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
    [System.IO.File]::WriteAllText($target, $patched, $utf8NoBom)
    try {
        $run = Invoke-Bounded -CargoArgs $testArgs -WorkingDirectory $repoRoot `
            -TranscriptPath $transcript -Seconds $TimeoutSeconds
    }
    finally {
        [System.IO.File]::WriteAllText($target, $original, $utf8NoBom)
        if ([System.IO.File]::ReadAllText($target) -ne $original) {
            Exit-WithMessage "FAILED TO RESTORE $target -- recover it with 'git checkout -- $target' before doing anything else." 3
        }
    }

    $caught = $run.Outcome -ne 'passed'
    $actual = switch ($run.Outcome) {
        'passed' { 'survived (NOT caught)' }
        'failed' { "caught (suite failed, exit $($run.Code))" }
        'hung' { "caught (suite HUNG past ${TimeoutSeconds}s)" }
    }
    $ok = if ($sabotage.expect -eq 'caught') { $caught } else { -not $caught }

    $results += [pscustomobject]@{
        Sabotage = $sabotage.name; Expected = $sabotage.expect
        Actual   = $actual; Ok = $ok; Patch = (Format-Patch -Find $find -Replace $replace)
    }

    $colour = if ($ok) { 'Green' } else { 'Red' }
    Write-Host ("{0,-58} {1}" -f $sabotage.name, $actual) -ForegroundColor $colour
}

''
$results | Select-Object Sabotage, Expected, Actual, Ok | Format-Table -AutoSize -Wrap

$unexpected = @($results | Where-Object { -not $_.Ok })
if ($unexpected.Count -eq 0) {
    Write-Host "All $($results.Count) sabotages behaved as declared." -ForegroundColor Green
    exit 0
}

''
Write-Host 'UNEXPECTED RESULTS -- read the patch before concluding the tests have a hole.' -ForegroundColor Red
Write-Host 'A sabotage that does not actually break anything will be survived for an honest reason.' -ForegroundColor Red
foreach ($result in $unexpected) {
    ''
    Write-Host "  $($result.Sabotage)" -ForegroundColor Red
    Write-Host "    expected $($result.Expected), got: $($result.Actual)"
    $result.Patch
}
''
Exit-WithMessage "$($unexpected.Count) of $($results.Count) sabotages did not behave as declared." 1
