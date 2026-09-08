# Copyright (c) 2026 Mike Grier. All rights reserved.
<#
.SYNOPSIS
    Tests for run-sabotage.ps1. Exits 0 only if every case passes.

.DESCRIPTION
    WHY THIS EXISTS. The harness accumulated fixes over eleven review rounds,
    and thirteen of the defects found in the later rounds were introduced by
    earlier fixes -- not because any fix went unverified, but because every
    verification was a one-off command that was then thrown away. Nothing
    re-checked round six's guarantee when round nine landed. This file is the
    ratchet: what it establishes stays established.

    That the harness had no tests was its own worst joke. It exists to measure
    whether a suite would notice a defect, and the two defects hardest to
    forgive -- a `timeoutSeconds` of zero slipping past the check written to
    reject it, and that check being skipped entirely under an override -- are
    exactly its own subject: a guard that is present, documented, and does not
    fire.

    WHY IT IS FAST. Two decisions. Every case runs against a THROWAWAY GIT
    REPOSITORY of two files, so the working-copy sync has nothing to copy. And
    cargo is replaced through -CargoCommand with a .cmd stub that passes, fails,
    or hangs on demand, so nothing is ever built. The whole file runs in
    seconds; one real sweep of the shipped waitable-queues manifest takes about
    six minutes.

    WHY NOT PESTER. Windows PowerShell 5.1 ships Pester 3, whose syntax differs
    incompatibly from Pester 5; the harness must work on both shells, so a
    Pester suite would either need an Install-Module step in CI or would only
    run on one of them. The four sibling tools in this directory are all
    standalone and dependency-free, CI invokes them directly, and a ratchet that
    needs installing first is a ratchet people skip. The assertion helpers below
    are the whole framework.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.

.PARAMETER Jobs
    How many shards to run at once, defaulting to the processor count capped at
    8. Pass 1 to run everything serially in this process.

    The cases are independent -- each builds its own throwaway repository under
    TEMP -- and almost none of the time is this script thinking. Measured per
    sweep: 338 ms of PowerShell startup, ~325 ms across four git subprocesses,
    ~310 ms spawning the stub, against ~770 ms of actual interpretation. Roughly
    half of every case is Windows creating processes, which is exactly the cost
    that parallelises.

    Sharding rather than in-process parallelism, because the two shells differ:
    ForEach-Object -Parallel is PowerShell 7 only, and runspace pools would need
    every helper re-declared inside them. Re-invoking this script with -Shard
    costs one process per shard and behaves identically on both, so what each
    shard runs is the serial path that is already verified.

.PARAMETER Shard
    Which shard this process is, from 0. Set by the dispatcher; not meant to be
    passed by hand.

.PARAMETER ShardCount
    How many shards exist in total. Set by the dispatcher.

.OUTPUTS
    Exits 0 if every case passed, 1 otherwise.
#>
[CmdletBinding()]
param(
    [string] $Name = '*',
    [int] $Jobs = 0,
    [int] $Shard = -1,
    [int] $ShardCount = 0
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# `Invoke-Native`, which every native call below goes through. Dot-sourced
# rather than imported: a module copy of that guard does not reach the
# scriptblock it is handed, and silently fails on 5.1 alone -- see
# [common.ps1](common.ps1), and [test-common.ps1](test-common.ps1) for the
# cross-host proof.
. (Join-Path $PSScriptRoot 'common.ps1')

$script:Harness = Join-Path $PSScriptRoot 'run-sabotage.ps1'
$script:Passed = 0
$script:Failed = 0
$script:Failures = @()

# Counts every case the -Name filter admits, whether or not this shard runs it,
# so the shard split is by POSITION and therefore stable: each case belongs to
# exactly one shard, and no case is run twice or skipped when the count changes.
$script:Ordinal = -1

function Write-Line {
    param([string] $Message, [ValidateSet('info', 'good', 'bad')] [string] $Level = 'info')
    $colour = @{ info = 'Gray'; good = 'Green'; bad = 'Red' }[$Level]
    Write-Host $Message -ForegroundColor $colour
}

# A case that throws is a failure, not a crash: one broken case must not hide
# the results of the rest, which is the whole point of running them together.
function Test-Case {
    param([string] $CaseName, [scriptblock] $Body)

    if ($CaseName -notlike $Name) { return }

    $script:Ordinal++
    if ($ShardCount -gt 0 -and ($script:Ordinal % $ShardCount) -ne $Shard) { return }

    try {
        & $Body
        $script:Passed++
        Write-Line "  PASS  $CaseName" -Level good
    }
    catch {
        $script:Failed++
        $script:Failures += "$CaseName -- $($_.Exception.Message)"
        Write-Line "  FAIL  $CaseName" -Level bad
        Write-Line "        $($_.Exception.Message)" -Level bad
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $Because = '')
    if ($Expected -ne $Actual) {
        throw "expected [$Expected], got [$Actual]$(if ($Because) { " -- $Because" })"
    }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $Because = '')
    if ($Text -notmatch $Pattern) {
        $shown = if ($Text.Length -gt 400) { $Text.Substring(0, 400) + '...' } else { $Text }
        throw "expected a match for [$Pattern]$(if ($Because) { " -- $Because" })`n--- output ---`n$shown"
    }
}

function Assert-True {
    param([bool] $Condition, [string] $Because = '')
    if (-not $Condition) { throw "expected true -- $Because" }
}

function Assert-False {
    param([bool] $Condition, [string] $Because = '')
    if ($Condition) { throw "expected false -- $Because" }
}

# --- fixtures ---------------------------------------------------------------

# A git repository with one source file, one manifest, and .scratch ignored.
# Ignoring it matters: the harness refuses an output directory inside the
# repository that git does not ignore, so a fixture without this would fail for
# a reason unrelated to whatever the case is asking about.
function New-Fixture {
    param($Manifest, [string] $Source = "// marker line`nfn main() {}`n")

    $root = Join-Path ([System.IO.Path]::GetTempPath()) ('sab-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'src') | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'stubs') | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $root '.gitignore'), ".scratch/`n")
    [System.IO.File]::WriteAllText((Join-Path $root 'src\lib.rs'), $Source)

    Invoke-Native { git -C $root init --quiet } | Out-Null
    if ($null -ne $Manifest) { Set-Manifest -Root $root -Spec $Manifest }
    # Added to the index but not committed: `git ls-files` reads the index,
    # which is all the harness needs, and committing would demand identity
    # configuration this fixture has no reason to care about.
    Invoke-Native { git -C $root add -A } | Out-Null
    return $root
}

function Set-Manifest {
    param([string] $Root, $Spec)
    $json = $Spec | ConvertTo-Json -Depth 8
    [System.IO.File]::WriteAllText((Join-Path $Root 'sabotage.json'), $json)
    Invoke-Native { git -C $Root add -A } | Out-Null
}

# The default manifest: patches the fixture's marker line, expecting it caught.
function New-Spec {
    param([hashtable] $Extra = @{}, [hashtable] $EntryExtra = @{}, [string[]] $EntryDrop = @())

    $entry = [ordered]@{
        name    = 'the marker is removed'
        file    = 'src/lib.rs'
        expect  = 'caught'
        why     = 'The marker is what the imaginary suite checks for.'
        find    = @('// marker line')
        replace = @('')
    }
    foreach ($k in $EntryExtra.Keys) { $entry[$k] = $EntryExtra[$k] }
    # Removed, not set to $null: the harness tests for the property's PRESENCE,
    # and a JSON null is present. Nulling a field would test nothing.
    foreach ($k in $EntryDrop) { $entry.Remove($k) }

    $spec = [ordered]@{ package = 'fixture'; sabotages = @($entry) }
    foreach ($k in $Extra.Keys) { $spec[$k] = $Extra[$k] }
    return $spec
}

# Stub cargo. `--no-run` marks the BUILD phase and always succeeds, so a stub
# can make the build pass and the test fail -- the shape every real `caught`
# sabotage has, and the one the phase split exists to tell apart.
function New-Stub {
    param(
        [ValidateSet('pass', 'fail', 'hang', 'doc-fail', 'build-fail')] [string] $Behaviour,
        [string] $Root
    )

    # Two guards, in order, and both are what make the stub behave like a real
    # suite rather than a constant.
    #
    # `--no-run` is the BUILD phase, which always succeeds: a stub that failed
    # it would exercise the build-failure path instead of the one under test.
    #
    # Then the stub reads the source it was pointed at and passes when the
    # marker is still there. That is the whole point: the BASELINE runs against
    # unmodified source and must be green, or the sweep refuses to start, and
    # only the sabotaged run -- marker removed -- may fail. A stub that failed
    # unconditionally would only ever prove that a red baseline aborts.
    $skipBuild = "echo %* | findstr /C:`"--no-run`" >nul && exit /b 0"
    $intact = "findstr /C:`"// marker line`" src\lib.rs >nul && exit /b 0"
    $body = switch ($Behaviour) {
        'pass' { "@echo off`r`necho ok`r`nexit /b 0`r`n" }
        'build-fail' { "@echo off`r`necho broken`r`nexit /b 101`r`n" }
        'fail' { "@echo off`r`n$skipBuild`r`n$intact`r`necho test failed`r`nexit /b 101`r`n" }
        'doc-fail' {
            "@echo off`r`n$skipBuild`r`n$intact`r`n" +
            "echo Couldn't compile the test.`r`nexit /b 101`r`n"
        }
        'hang' { "@echo off`r`n$skipBuild`r`n$intact`r`nping -n 900 127.0.0.1 >nul`r`n" }
    }
    $path = Join-Path $Root "stubs\$Behaviour.cmd"
    [System.IO.File]::WriteAllText($path, $body)
    return $path
}

# Runs the harness inside a fixture. A child process, not dot-sourcing, because
# the exit code IS the contract under test.
function Invoke-Harness {
    param([string] $Root, [string[]] $Arguments)

    Push-Location $Root
    # Through Invoke-Native, and that is load bearing on Windows PowerShell 5.1.
    # The harness runs here as a CHILD PROCESS, so its `Exit-WithMessage` writes
    # -- which go straight to the process stderr handle -- are native stderr to
    # this script. Redirected with 2>&1 under Stop they arrive as ErrorRecords,
    # which is a TERMINATING error: every case testing a rejection path threw on
    # the harness's own message instead of reading its exit code, and reported
    # the harness's text as the failure. The harness writes to stderr
    # deliberately, so its output is data here, not a fault. PowerShell 7 does
    # not do this, which is why the suite passed there and failed on 5.1 until
    # it was run on both.
    try {
        $shell = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell' }
        $text = Invoke-Native {
            & $shell -NoProfile -File $script:Harness @Arguments
        } | Out-String
        return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $text }
    }
    finally {
        Pop-Location
    }
}

function Remove-Fixture {
    param([string] $Root)
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}

# Processes still alive from a given fixture's hang stub.
#
# Scoped by the FIXTURE PATH, not by looking for sleepers globally. The stub is
# a .cmd, so Windows runs it as `cmd.exe /c <path>` and that path carries the
# fixture's GUID -- unique to one case. An earlier version matched any ping with
# the stub's distinctive count, which is correct when cases run one at a time
# and wrong the moment they do not: under -Jobs every other shard's hang case
# would look like this one's leak.
function Get-StrayProcesses {
    param([string] $FixtureRoot)
    Get-CimInstance Win32_Process -Filter "name='cmd.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine -like "*$FixtureRoot*" }
}

# --- parallel dispatch ------------------------------------------------------
#
# A parent process fans the cases out across shards and never runs one itself,
# so there is exactly one code path for actually executing a case: the serial
# one below, which every shard takes. Nothing about a case behaves differently
# under -Jobs; it only runs in a different process.
#
# Shards are shells of THIS script with -Shard/-ShardCount, chosen over
# ForEach-Object -Parallel (PowerShell 7 only) and runspace pools (every helper
# would need re-declaring inside them). One extra process per shard buys
# identical behaviour on both shells.
if ($Shard -lt 0) {
    if ($Jobs -le 0) {
        $Jobs = [Math]::Min(8, [Environment]::ProcessorCount)
    }

    if ($Jobs -gt 1) {
        $shell = if ($PSVersionTable.PSVersion.Major -ge 6) { 'pwsh' } else { 'powershell' }
        $running = @()
        for ($i = 0; $i -lt $Jobs; $i++) {
            $out = Join-Path ([System.IO.Path]::GetTempPath()) "sab-shard-$PID-$i.txt"
            $running += [pscustomobject]@{
                Index   = $i
                Output  = $out
                Process = Start-Process -FilePath $shell -PassThru -NoNewWindow `
                    -RedirectStandardOutput $out -RedirectStandardError "$out.err" `
                    -ArgumentList @(
                    '-NoProfile', '-File', $PSCommandPath,
                    '-Name', $Name, '-Shard', $i, '-ShardCount', $Jobs)
            }
        }

        # Named $worker, NOT $shard: PowerShell matches variables case
        # insensitively, so a loop variable called $shard IS the [int] $Shard
        # parameter, and assigning an object to it fails at runtime.
        #
        # Handles are touched before waiting for the same reason the harness
        # does it: on Windows PowerShell 5.1 a Start-Process object does not
        # cache one, and ExitCode then reads back $null however the shard ended.
        foreach ($worker in $running) { $null = $worker.Process.Handle }

        $failed = 0
        foreach ($worker in $running) {
            $worker.Process.WaitForExit()
            if (Test-Path -LiteralPath $worker.Output) {
                Get-Content -LiteralPath $worker.Output | ForEach-Object { Write-Host $_ }
            }
            if ($worker.Process.ExitCode -ne 0) { $failed++ }
            Remove-Item -LiteralPath $worker.Output, "$($worker.Output).err" `
                -Force -ErrorAction SilentlyContinue
        }

        Write-Line ''
        if ($failed -eq 0) {
            Write-Line "All shards passed ($Jobs in parallel)." -Level good
            exit 0
        }
        Write-Line "$failed of $Jobs shards reported failures." -Level bad
        exit 1
    }
}

# --- validation: every one of these must be exit 2 --------------------------
#
# Exit 2 is "nothing was swept". Driven from a table because the POPULATION is
# what matters here: a guard that exists but cannot fire is the defect class
# this file is here to prevent, and the only way to know each one fires is to
# name each and run it. Two of these -- the zero bounds -- were guards that
# shipped unable to fire, because PowerShell counts 0 as false and the check
# short-circuited before reaching its own comparison.

Write-Line 'validation (each must exit 2)'

$validationCases = @(
    @{ Case = 'a manifest file that does not exist'
        Args = @('-Manifest', 'no-such-file.json', '-List') }

    @{ Case = 'a manifest with no sabotages array'
        Spec = [ordered]@{ package = 'fixture' } }

    @{ Case = 'an empty sabotages array'
        Spec = [ordered]@{ package = 'fixture'; sabotages = @() } }

    @{ Case = 'a missing package with no testArgs'
        Spec = [ordered]@{ sabotages = @([ordered]@{ name = 'n'; file = 'src/lib.rs'
                expect = 'caught'; why = 'w'; find = @('// marker line'); replace = @('') }) } }

    @{ Case = 'a manifest timeoutSeconds of zero'; Extra = @{ timeoutSeconds = 0 } }
    @{ Case = 'a manifest timeoutSeconds below zero'; Extra = @{ timeoutSeconds = -5 } }
    @{ Case = 'a per-sabotage timeoutSeconds of zero'; EntryExtra = @{ timeoutSeconds = 0 } }
    @{ Case = 'an expect that is neither caught nor survives'; EntryExtra = @{ expect = 'maybe' } }
    @{ Case = 'a missing per-sabotage why'; EntryDrop = @('why') }
    @{ Case = 'a missing per-sabotage find'; EntryDrop = @('find') }
    @{ Case = 'a root that does not resolve'; Extra = @{ root = 'no/such/place' } }
    @{ Case = 'testArgs carrying --target-dir, which the sweep sets itself'
        Extra = @{ testArgs = @('-p', 'fixture', '--target-dir', 'C:\elsewhere') } }
    @{ Case = 'a TimeoutMultiplier of zero'
        Args = @('-Manifest', 'sabotage.json', '-TimeoutMultiplier', '0', '-List') }
    @{ Case = 'a TimeoutFloorSeconds of zero'
        Args = @('-Manifest', 'sabotage.json', '-TimeoutFloorSeconds', '0', '-List') }

    # These three are checked AFTER the -List exit, because listing writes no
    # transcript and patches no file, so nothing can collide or be missing yet.
    # They therefore need a real sweep to reach their guard -- which is itself
    # worth pinning, since testing them with -List would silently pass.
    @{ Case = 'a sabotage named baseline, which the transcript owns'
        EntryExtra = @{ name = 'baseline' }; Sweep = $true }
    @{ Case = 'a file the manifest names but the tree does not have'
        EntryExtra = @{ file = 'src/absent.rs' }; Sweep = $true }
)

foreach ($case in $validationCases) {
    $spec = if ($case.ContainsKey('Spec')) { $case.Spec }
    else {
        New-Spec -Extra $(if ($case.ContainsKey('Extra')) { $case.Extra } else { @{} }) `
            -EntryExtra $(if ($case.ContainsKey('EntryExtra')) { $case.EntryExtra } else { @{} }) `
            -EntryDrop $(if ($case.ContainsKey('EntryDrop')) { $case.EntryDrop } else { @() })
    }
    $arguments = if ($case.ContainsKey('Args')) { $case.Args }
    else { @('-Manifest', 'sabotage.json', '-List') }
    $sweep = $case.ContainsKey('Sweep')

    Test-Case "rejects $($case.Case)" ([scriptblock]::Create(@'
        $root = New-Fixture -Manifest $spec
        try {
            $effective = $arguments
            if ($sweep) {
                $stub = New-Stub -Behaviour 'fail' -Root $root
                $effective = @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
            }
            $result = Invoke-Harness -Root $root -Arguments $effective
            Assert-Equal 2 $result.ExitCode $result.Output
        }
        finally { Remove-Fixture $root }
'@))
}

Test-Case 'rejects two sabotages whose names differ only in punctuation' {
    $spec = [ordered]@{
        package   = 'fixture'
        sabotages = @(
            [ordered]@{ name = 'a: b'; file = 'src/lib.rs'; expect = 'caught'; why = 'w'
                find = @('// marker line'); replace = @('') },
            [ordered]@{ name = 'a - b'; file = 'src/lib.rs'; expect = 'caught'; why = 'w'
                find = @('fn main() {}'); replace = @('') })
    }
    $root = New-Fixture -Manifest $spec
    try {
        # A sweep, not -List: the stem check runs after the listing path exits,
        # because listing writes no transcript for two names to collide over.
        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 2 $result.ExitCode $result.Output
        Assert-Match 'same file name stem' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'rejects a manifest that is not valid JSON' {
    $root = New-Fixture -Manifest $null
    try {
        [System.IO.File]::WriteAllText((Join-Path $root 'sabotage.json'), '{ "package": nope')
        $result = Invoke-Harness -Root $root -Arguments @('-Manifest', 'sabotage.json', '-List')
        Assert-Equal 2 $result.ExitCode $result.Output
        Assert-Match 'not valid JSON' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'rejects an output directory inside the repository that git does not ignore' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-OutputDirectory', 'not-ignored')
        Assert-Equal 2 $result.ExitCode $result.Output
        Assert-Match 'does not ignore it' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'rejects being run outside a git repository, on both hosts' {
    # This one is host-sensitive, and it went unnoticed because the suite only
    # ever ran the harness INSIDE a fixture repository. `Get-RepoRoot` captured
    # git with `2>$null`, and on Windows PowerShell 5.1 any stderr redirect makes
    # a native command's stderr a TERMINATING error under `Stop` -- so outside a
    # working tree, where git's whole answer is `fatal: not a git repository` on
    # stderr, the deliberate exit-2 path was unreachable. The harness died with
    # NativeCommandError and exited 1, which in this script means "sabotages did
    # not behave as declared": a broken instrument reported as a finding.
    #
    # PowerShell 7 reached the intended path either way, which is exactly why
    # this needs a test rather than a reading.
    $outside = Join-Path ([System.IO.Path]::GetTempPath()) ('nogit-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $outside | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $outside 'sabotage.json'), (New-Spec | ConvertTo-Json -Depth 8))
    try {
        $result = Invoke-Harness -Root $outside -Arguments @('-Manifest', 'sabotage.json', '-List')
        Assert-Equal 2 $result.ExitCode $result.Output
        Assert-Match 'Not inside a git repository' $result.Output
    }
    finally { Remove-Item -Recurse -Force $outside -ErrorAction SilentlyContinue }
}

Test-Case 'validates a manifest timeoutSeconds even when -TimeoutSeconds overrides it' {
    # The check used to be gated on "we are about to use this", so a bogus
    # manifest value was diagnosed only on the runs that did NOT override it --
    # the run an author debugging an override is least likely to make.
    $root = New-Fixture -Manifest (New-Spec -Extra @{ timeoutSeconds = -5 })
    try {
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-TimeoutSeconds', '30', '-List')
        Assert-Equal 2 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'treats -TimeoutSeconds 0 as derive-it, not as an invalid bound' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-TimeoutSeconds', '0', '-List')
        Assert-Equal 0 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

# --- -List is inert ---------------------------------------------------------

Write-Line ''
Write-Line '-List is inert'

Test-Case 'lists without creating its output directory' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $result = Invoke-Harness -Root $root -Arguments @('-Manifest', 'sabotage.json', '-List')
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'the marker is removed' $result.Output
        Assert-False (Test-Path (Join-Path $root '.scratch\sabotage')) `
            'listing must not create the output directory'
    }
    finally { Remove-Fixture $root }
}

# --- classification ---------------------------------------------------------
#
# The outcomes this tool exists to keep apart. Getting any of these wrong is
# worse than a crash, because a wrong answer here reads as a clean bill of
# health.

Write-Line ''
Write-Line 'classification'

Test-Case 'reports a sabotage the suite fails on as caught' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'caught \(suite failed' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'reports a sabotage the suite sails through as survived, and fails the sweep' {
    # The manifest expects `caught`, so a survivor is a mismatch: exit 1, the
    # code meaning "the sweep ran and something did not behave as declared".
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'pass' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'survived \(NOT caught\)' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'does not count a patch that only breaks a doctest as caught' {
    # `--no-run` cannot build doctests, so such a patch passes the build and
    # fails the run with a compile error. Counting that as caught would credit
    # the tests with a detection they never made -- the weaker claim wearing the
    # stronger one's label.
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'doc-fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'MANIFEST DOES NOT COMPILE \(a doctest would not build\)' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'refuses to sweep when the baseline is already red' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'build-fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 2 $result.ExitCode $result.Output
        Assert-Match 'baseline suite did not pass' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'reports a pattern that matches nothing as stale, not as a result' {
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ find = @('text that is not there') })
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'MANIFEST STALE' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'reports a pattern that matches twice as stale' {
    # Exactly once, not at least once: a pattern matching two sites patches
    # whichever the replace happens to reach, and the sabotage is then not the
    # one the manifest describes.
    # The marker stays so the BASELINE is green; the twins are what the pattern
    # matches twice.
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ find = @('// twin') }) `
        -Source "// marker line`n// twin`n// twin`nfn main() {}`n"
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'found 2 times' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'reports a patch that changes nothing as inert' {
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ replace = @('// marker line') })
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 1 $result.ExitCode $result.Output
        Assert-Match 'MANIFEST INERT' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'accepts a control that is declared to survive' {
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ expect = 'survives' })
    try {
        $stub = New-Stub -Behaviour 'pass' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'survived \(NOT caught\)' $result.Output
    }
    finally { Remove-Fixture $root }
}

# --- hangs ------------------------------------------------------------------

Write-Line ''
Write-Line 'hangs'

Test-Case 'kills a hung run at the bound and counts it as caught' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'hang' -Root $root
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $result = Invoke-Harness -Root $root -Arguments @(
            '-Manifest', 'sabotage.json', '-CargoCommand', $stub, '-TimeoutSeconds', '5')
        $clock.Stop()

        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match 'caught \(tests HUNG past 5s\)' $result.Output

        # The bound must actually bound. A timed WaitForExit did not: the
        # process cargo spawns inherits the redirected stream handles, and the
        # wait outlives the deadline for as long as they are held -- measured
        # once at 31 minutes against a 60-second bound. A tool whose job is
        # detecting hangs must not be hangable by one.
        Assert-True ($clock.Elapsed.TotalSeconds -lt 60) `
            "the 5s bound took $([int]$clock.Elapsed.TotalSeconds)s to enforce"
    }
    finally { Remove-Fixture $root }
}

Test-Case 'leaves no stray process behind after killing a hung run' {
    $root = New-Fixture -Manifest (New-Spec)
    # Scoped to processes this case starts, and cleaned up afterwards whatever
    # the result. Both matter when the harness is swept by sabotage2.json: an
    # entry that deletes the kill leaves orphans behind, and a case comparing
    # against an absolute count would then fail for every LATER entry too --
    # including the control, whose whole job is to survive.
    try {
        $stub = New-Stub -Behaviour 'hang' -Root $root
        Invoke-Harness -Root $root -Arguments @(
            '-Manifest', 'sabotage.json', '-CargoCommand', $stub, '-TimeoutSeconds', '5') | Out-Null

        Assert-Equal 0 @(Get-StrayProcesses -FixtureRoot $root).Count `
            'the kill must take the whole process tree'
    }
    finally {
        # Cleaned up whatever the result, so an entry in sabotage2.json that
        # deletes the kill cannot leave orphans behind for later entries.
        Get-StrayProcesses -FixtureRoot $root |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Remove-Fixture $root
    }
}

# --- the working tree is never touched --------------------------------------
#
# The premise of the whole design. Each of these was a real defect: the tree was
# patched in place, the caller's environment was mutated and outlived the run,
# and a restore that preserved mtimes left a SABOTAGED binary in the build cache
# for the next run to test against.

Write-Line ''
Write-Line 'the working tree is never touched'

Test-Case 'leaves every source file byte-identical and creates no target directory' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $source = Join-Path $root 'src\lib.rs'
        $before = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash

        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 0 $result.ExitCode $result.Output

        Assert-Equal $before (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash `
            'the sweep patches a copy, never the real file'
        Assert-False (Test-Path (Join-Path $root 'target')) `
            'the build must land in the scratch target, not the tree'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'does not leave CARGO_TARGET_DIR set in the calling session' {
    # A .ps1 runs IN the caller's process, so an $env: assignment outlives it:
    # every later cargo command in that session, in any repository, would have
    # built into this tool's scratch directory.
    $root = New-Fixture -Manifest (New-Spec)
    $before = $env:CARGO_TARGET_DIR
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        Push-Location $root
        # Run IN this process, not through Invoke-Harness: an $env: assignment
        # in a child would not be observable here, and whether it escapes into
        # the caller is the entire question. 6>$null swallows the harness's
        # progress output, which Write-Host puts on the information stream.
        try { & $script:Harness -Manifest 'sabotage.json' -CargoCommand $stub 6>$null 2>&1 | Out-Null }
        finally { Pop-Location }
        Assert-Equal $before $env:CARGO_TARGET_DIR 'the sweep must not touch the caller environment'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'sweeps the working tree as it stands, uncommitted edits included' {
    $root = New-Fixture -Manifest (New-Spec)
    try {
        # Rewritten after `git add`, so this content exists only in the working
        # tree. A copy taken from a commit would not see it.
        [System.IO.File]::WriteAllText((Join-Path $root 'src\lib.rs'),
            "// marker line`nfn edited_but_uncommitted() {}`n")

        $stub = New-Stub -Behaviour 'fail' -Root $root
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 0 $result.ExitCode $result.Output

        $copied = Get-Content -LiteralPath (Join-Path $root '.scratch\sabotage\tree\src\lib.rs') -Raw
        Assert-Match 'edited_but_uncommitted' $copied 'the copy must reflect the working tree'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'stamps the restored file so a build system sees it as changed' {
    # Found by sabotage2.json, which is the point of pointing the tool at
    # itself: replacing the restore with File.Copy SURVIVED the suite. Nothing
    # here had noticed, because the defect is about cargo's rebuild decision and
    # every case stubs cargo out -- so cargo's fingerprinting is structurally
    # invisible to them.
    #
    # This tests the MECHANISM instead of the consequence: the restore must
    # stamp the file with the current time, which is what makes a build system
    # treat it as changed. File.Copy carries the source's timestamp across
    # instead, so the artifact built from the PATCHED source looks newer than
    # the restored input and survives into the next run.
    #
    # The real file is aged deliberately, so "carried the source's timestamp"
    # and "stamped now" are an hour apart rather than milliseconds.
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $real = Join-Path $root 'src\lib.rs'
        (Get-Item -LiteralPath $real).LastWriteTime = (Get-Date).AddHours(-1)

        $stub = New-Stub -Behaviour 'fail' -Root $root
        $started = Get-Date
        $result = Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
        Assert-Equal 0 $result.ExitCode $result.Output

        $copied = Join-Path $root '.scratch\sabotage\tree\src\lib.rs'
        $stamped = (Get-Item -LiteralPath $copied).LastWriteTime
        Assert-True ($stamped -ge $started) (
            "the restored copy is stamped $stamped, before the sweep began at " +
            "$started -- it carried the source's timestamp instead of being rewritten")
    }
    finally { Remove-Fixture $root }
}

Test-Case 'stays green across two consecutive sweeps' {
    # The regression that matters most, and the one a single sweep cannot see.
    # A restore preserving the original mtime let cargo judge the crate up to
    # date against an artifact built from PATCHED source, so the sabotaged build
    # survived into the next run and the second sweep's baseline failed on code
    # nobody had changed.
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $stub = New-Stub -Behaviour 'fail' -Root $root
        foreach ($pass in 1, 2) {
            $result = Invoke-Harness -Root $root `
                -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub)
            Assert-Equal 0 $result.ExitCode "sweep $pass`n$($result.Output)"
        }
    }
    finally { Remove-Fixture $root }
}

# --- the working copy tracks the real tree ----------------------------------

Write-Line ''
Write-Line 'the working copy tracks the real tree'

Test-Case 'copies a file whose name git quotes, and drops it when the source does' {
    # core.quotePath is on by default, so a non-ASCII path arrives wrapped in
    # quotes with octal escapes. Read literally that names nothing, so the file
    # was neither copied nor recorded as wanted: the copy differed from the tree
    # by exactly the files nothing could see.
    $root = New-Fixture -Manifest (New-Spec)
    try {
        $odd = Join-Path $root ('src\caf' + [char]0xE9 + '.rs')
        [System.IO.File]::WriteAllText($odd, "// unicode`n")
        Invoke-Native { git -C $root add -A } | Out-Null

        $stub = New-Stub -Behaviour 'fail' -Root $root
        Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub) | Out-Null

        $copied = Join-Path $root ('.scratch\sabotage\tree\src\caf' + [char]0xE9 + '.rs')
        Assert-True (Test-Path -LiteralPath $copied) 'the copy must not silently omit it'

        Remove-Item -LiteralPath $odd -Force
        Invoke-Native { git -C $root add -A } | Out-Null
        Invoke-Harness -Root $root `
            -Arguments @('-Manifest', 'sabotage.json', '-CargoCommand', $stub) | Out-Null

        Assert-False (Test-Path -LiteralPath $copied) `
            'a deletion must not leave a stale twin for the compiler to find'
    }
    finally { Remove-Fixture $root }
}

# --- the hang bound ---------------------------------------------------------

Write-Line ''
Write-Line 'the hang bound'

$boundCases = @(
    @{ Case = 'falls back to the floor when the multiplied baseline is smaller'
        Extra = @{}; Args = @(); Expect = '15s \(3x the \d+s baseline, floor 15s\)' }

    @{ Case = 'takes the command line over everything'
        Extra = @{ timeoutSeconds = 40 }; Args = @('-TimeoutSeconds', '25')
        Expect = '25s \(-TimeoutSeconds\)' }

    @{ Case = 'credits the manifest, not a flag nobody passed'
        Extra = @{ timeoutSeconds = 40 }; Args = @()
        Expect = "40s \(the manifest's timeoutSeconds\)" }

    @{ Case = 'honours a raised floor'
        Extra = @{}; Args = @('-TimeoutFloorSeconds', '33'); Expect = '33s' }
)

foreach ($case in $boundCases) {
    $spec = New-Spec -Extra $case.Extra
    $extraArgs = $case.Args
    $expect = $case.Expect

    Test-Case "the bound $($case.Case)" ([scriptblock]::Create(@'
        $root = New-Fixture -Manifest $spec
        try {
            $stub = New-Stub -Behaviour 'fail' -Root $root
            $result = Invoke-Harness -Root $root -Arguments (
                @('-Manifest', 'sabotage.json', '-CargoCommand', $stub) + $extraArgs)
            Assert-Equal 0 $result.ExitCode $result.Output
            Assert-Match "Hang bound: $expect" $result.Output
        }
        finally { Remove-Fixture $root }
'@))
}

Test-Case 'lets a single sabotage raise the bound above the sweep-wide one' {
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ timeoutSeconds = 12 })
    try {
        $stub = New-Stub -Behaviour 'hang' -Root $root
        $result = Invoke-Harness -Root $root -Arguments @(
            '-Manifest', 'sabotage.json', '-CargoCommand', $stub, '-TimeoutSeconds', '4')
        # 4 on the command line, 12 on the entry: the entry raises it.
        Assert-Match 'HUNG past 12s' $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case 'ignores a per-sabotage bound that would lower the sweep-wide one' {
    # Only ever raises: the reason to lower one is speed, and the cost of being
    # wrong about it is a run scored `caught` that was merely slow.
    $root = New-Fixture -Manifest (New-Spec -EntryExtra @{ timeoutSeconds = 3 })
    try {
        $stub = New-Stub -Behaviour 'hang' -Root $root
        $result = Invoke-Harness -Root $root -Arguments @(
            '-Manifest', 'sabotage.json', '-CargoCommand', $stub, '-TimeoutSeconds', '8')
        Assert-Match 'HUNG past 8s' $result.Output
    }
    finally { Remove-Fixture $root }
}

# --- report -----------------------------------------------------------------

Write-Line ''
if ($script:Failed -eq 0) {
    Write-Line "All $($script:Passed) cases passed." -Level good
    exit 0
}

Write-Line "$($script:Failed) of $($script:Passed + $script:Failed) cases failed:" -Level bad
foreach ($failure in $script:Failures) { Write-Line "  $failure" -Level bad }
exit 1
