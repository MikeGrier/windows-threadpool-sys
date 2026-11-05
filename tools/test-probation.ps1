# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [probation-common.ps1](probation-common.ps1), [run-tier.ps1](run-tier.ps1),
    [publish-probation-results.ps1](publish-probation-results.ps1) and
    [report-probation.ps1](report-probation.ps1).

.DESCRIPTION
    The record these keep is what promoting a test from probation is decided on, so a defect
    in any of them is quiet in the worst way: a test counted as passing that did not, a run
    counted twice, or a test that killed its process counted as never having run.

    Cases are BIDIRECTIONAL where a rule has two sides: a test that is a candidate at the
    threshold is paired with one a pass short of it, a failure that ends a run of passes with
    a pass that continues one, a run applied once with the same run applied again.

    Parsing and the record are driven in memory from synthetic captures. The publisher runs
    against a bare repository standing in for the remote. `run-tier.ps1` runs against real
    one-crate workspaces, because libtest's JSON and cargo's headers are what it reads, and
    one of them has an executable that aborts, the case a synthetic capture can only imitate.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$publisher = Join-Path $PSScriptRoot 'publish-probation-results.ps1'
$reporter = Join-Path $PSScriptRoot 'report-probation.ps1'
$runner = Join-Path $PSScriptRoot 'run-tier.ps1'
foreach ($script in @($publisher, $reporter, $runner)) {
    if (-not (Test-Path -LiteralPath $script)) { throw "$script not found beside this script" }
}

. (Join-Path $PSScriptRoot 'probation-common.ps1')

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $Message)
    if ($Expected -ne $Actual) { throw "$Message (expected '$Expected', got '$Actual')" }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $Message)
    if ($Text -notmatch $Pattern) { throw "$Message (no match for /$Pattern/ in: $Text)" }
}

$script:roots = New-Object System.Collections.Generic.List[string]

function New-Root {
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ('probation-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    $script:roots.Add($path) | Out-Null
    return $path
}

function Remove-Roots {
    foreach ($path in $script:roots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
    $script:roots.Clear()
}

# Captured messages must not wrap: on Windows PowerShell 5.1 `Out-String` wraps captured host
# output at the host's width, which on a CI runner is narrow.
$captureWidth = 4096

# ---- synthetic captures -------------------------------------------------------------------

function Write-Text {
    param([string] $Path, [string[]] $Lines)
    [System.IO.File]::WriteAllText($Path, (($Lines -join "`n") + "`n"), [System.Text.UTF8Encoding]::new($false))
}

# A `compiler-artifact` message for a test executable.
function New-Artifact {
    param([string] $Root, [string] $Exe, [string] $Source, [bool] $Test = $true, [bool] $HasExe = $true)
    $message = [ordered]@{
        reason     = 'compiler-artifact'
        target     = [ordered]@{ name = 'x'; src_path = (Join-Path $Root $Source) }
        profile    = [ordered]@{ test = $Test }
        executable = $(if ($HasExe) { "$Root\target\debug\deps\$Exe" } else { $null })
    }
    return ($message | ConvertTo-Json -Compress -Depth 5)
}

function New-Header { param([string] $Exe) "     Running unittests src\lib.rs (target\debug\deps\$Exe)" }
function New-Started { param([string] $Test) "{ `"type`": `"test`", `"event`": `"started`", `"name`": `"$Test`" }" }
function New-Result {
    param([string] $Test, [string] $Event, [string] $Out = '')
    $extra = ''
    if ($Out) { $extra = ', "stdout": "' + $Out + '"' }
    return '{ "type": "test", "name": "' + $Test + '", "event": "' + $Event + '"' + $extra + ' }'
}
$suiteOk = '{ "type": "suite", "event": "ok", "passed": 0, "failed": 0, "ignored": 0, "measured": 0, "filtered_out": 0 }'

# The map and the parse of a capture, from lines.
function Read-Capture {
    param([string[]] $Lines, [hashtable] $Map)
    $path = Join-Path (New-Root) 'capture.txt'
    Write-Text $path $Lines
    return ConvertFrom-ProbationCapture -CapturePath $path -TargetMap $Map
}

$mapOne = @{ 'a-111.exe' = 'crates/a/src/lib.rs'; 'b-222.exe' = 'crates/b/src/lib.rs' }

function Get-Outcome {
    param($Read, [string] $Key)
    return @($Read.Outcomes | Where-Object { $_.Key -eq $Key })
}

# ---- records ------------------------------------------------------------------------------

function New-Run {
    param([object[]] $Pairs)
    return @($Pairs | ForEach-Object { [pscustomobject]@{ Key = $_[0]; Outcome = $_[1]; Note = '' } })
}

$t0 = '2026-01-01T00:00:00Z'
$t1 = '2026-01-02T00:00:00Z'
$t2 = '2026-01-03T00:00:00Z'
$sha1 = '1' * 40
$sha2 = '2' * 40

# ---- the publisher's remote ---------------------------------------------------------------

$identity = @('-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false')

function New-Remote {
    $path = Join-Path (New-Root) 'remote.git'
    Invoke-Native { git init --bare --quiet $path } | Out-Null
    return $path
}

function New-Clone {
    param([string] $Remote)
    $path = Join-Path (New-Root) 'clone'
    Invoke-Native { git init --quiet $path } | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $path 'README.md'), "fixture`n")
    Invoke-Native { git -C $path add -A } | Out-Null
    Invoke-Native { git -C $path @identity commit --quiet -m base } | Out-Null
    Invoke-Native { git -C $path remote add origin $Remote } | Out-Null
    return $path
}

function New-OutcomesFile {
    param([object[]] $Pairs)
    $path = Join-Path (New-Root) 'outcomes.json'
    $items = @($Pairs | ForEach-Object { [ordered]@{ key = $_[0]; outcome = $_[1] } })
    $json = [ordered]@{ version = 1; module = 'probation'; outcomes = $items } | ConvertTo-Json -Depth 5
    [System.IO.File]::WriteAllText($path, $json + "`n", [System.Text.UTF8Encoding]::new($false))
    return $path
}

function Invoke-Publisher {
    param([string] $Clone, [string] $Outcomes, [string] $RunId, [string] $At = $t1, [string] $Commit = $sha1, [hashtable] $Extra = @{}, [string] $Remote = 'origin')
    $output = & $publisher -Outcomes $Outcomes -RunId $RunId -Commit $Commit -At $At -RepositoryRoot $Clone -Remote $Remote -RetryDelaySeconds 0 @Extra 6>&1 | Out-String -Width $captureWidth
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $output }
}

function Get-Tip {
    param([string] $Remote)
    $found = @(Invoke-NativeStdout { git --git-dir $Remote rev-parse --verify 'refs/heads/probation-results' })
    if ($LASTEXITCODE -ne 0) { return $null }
    return ([string]$found[0]).Trim()
}

function Read-Published {
    param([string] $Remote)
    if (-not (Get-Tip $Remote)) { return $null }
    # Read through a clone: a bare repository is only read with --git-dir, which the UTF-8 reader does not take.
    $reader = Join-Path (New-Root) 'reader'
    Invoke-Native { git init --quiet $reader } | Out-Null
    Invoke-Native { git -C $reader fetch --quiet $Remote '+refs/heads/probation-results:refs/heads/held' } | Out-Null
    return ConvertFrom-ProbationRecordText (Get-GitBlobUtf8 -RepositoryRoot $reader -Spec 'held:results.json')
}

function Get-CommitCount {
    param([string] $Remote)
    $n = @(Invoke-NativeStdout { git --git-dir $Remote rev-list --count 'refs/heads/probation-results' })
    return [int]([string]$n[0]).Trim()
}

# ---- real one-crate workspaces ------------------------------------------------------------

function New-Fixture {
    param([hashtable] $Files)
    $root = Join-Path (New-Root) 'fixture'
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'src'), (Join-Path $root 'tests') | Out-Null
    [System.IO.File]::WriteAllText((Join-Path $root 'Cargo.toml'), "[package]`nname = `"tiers-fixture`"`nversion = `"0.0.0`"`nedition = `"2021`"`n")
    foreach ($relative in $Files.Keys) { [System.IO.File]::WriteAllText((Join-Path $root $relative), $Files[$relative]) }
    Push-Location -LiteralPath $root
    try { & cargo generate-lockfile 2>&1 | Out-Null } finally { Pop-Location }
    return $root
}

function Invoke-Tier {
    param([string] $Root, [string] $Module, [hashtable] $Extra = @{}, [string[]] $CargoArgs = @('--lib', '--tests', '--no-fail-fast'))
    $out = Join-Path (New-Root) 'out'
    $text = & $runner -Module $Module -OutputDirectory $out -RepositoryRoot $Root -CargoArgs $CargoArgs @Extra 6>&1 | Out-String -Width $captureWidth
    $outcomes = $null
    $file = Join-Path $out 'outcomes.json'
    if (Test-Path -LiteralPath $file) { $outcomes = [System.IO.File]::ReadAllText($file) | ConvertFrom-Json }
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $text; Outcomes = $outcomes }
}

function Get-Verdict {
    param($Result, [string] $Key)
    $found = @($Result.Outcomes.outcomes | Where-Object { $_.key -eq $Key })
    if ($found.Count -ne 1) { return $null }
    return [string]$found[0].outcome
}

$cases = @(
    # ---- the map of executables to targets ----
    @{
        Name = 'the-map-names-each-test-executable-by-its-source-path-relative-to-the-workspace'
        Run  = {
            $root = New-Root
            $build = Join-Path $root 'build.json'
            Write-Text $build @(
                (New-Artifact $root 'a-111.exe' 'crates\a\src\lib.rs'),
                (New-Artifact $root 'b-222.exe' 'crates\b\tests\t.rs'),
                'Compiling noise',
                (New-Artifact $root 'lib-333.exe' 'crates\c\src\lib.rs' -Test $false),
                (New-Artifact $root 'none.exe' 'crates\d\src\lib.rs' -HasExe $false))
            $map = Get-ProbationTargetMap -BuildJsonPath $build -RepositoryRoot $root
            Assert-Equal 'crates/a/src/lib.rs' $map['a-111.exe'] 'a library test executable'
            Assert-Equal 'crates/b/tests/t.rs' $map['b-222.exe'] 'an integration test, with forward slashes'
            Assert-True (-not $map.ContainsKey('lib-333.exe')) 'a build that is not a test build is not a test executable'
            Assert-True (-not $map.ContainsKey('none.exe')) 'an artifact with no executable is not one'
            Assert-Equal 2 $map.Count 'and nothing else'
        }
    },
    @{
        Name = 'a-source-outside-the-workspace-keeps-its-whole-path'
        Run  = {
            $root = New-Root; $other = New-Root
            $build = Join-Path $root 'build.json'
            Write-Text $build @((New-Artifact $other 'x-1.exe' 'src\lib.rs'))
            $map = Get-ProbationTargetMap -BuildJsonPath $build -RepositoryRoot $root
            Assert-True ($map['x-1.exe'] -match '^[A-Za-z]:/') "a path that is not under the root is not made to look as if it were: $($map['x-1.exe'])"
        }
    },
    # ---- parsing a capture ----
    @{
        Name = 'a-capture-gives-each-test-its-outcome-under-the-target-it-ran-in'
        Run  = {
            $read = Read-Capture @(
                'running 3 tests',
                (New-Header 'a-111.exe'), '{ "type": "suite", "event": "started", "test_count": 3 }',
                (New-Started 'm::passes'), (New-Result 'm::passes' 'ok'),
                (New-Started 'm::fails'), (New-Result 'm::fails' 'failed' 'it broke'),
                (New-Started 'm::skipped'), (New-Result 'm::skipped' 'ignored'),
                $suiteOk) $mapOne
            Assert-Equal 2 $read.Outcomes.Count 'a pass and a failure; an ignored test is neither'
            Assert-Equal 'pass' (Get-Outcome $read 'crates/a/src/lib.rs::m::passes')[0].Outcome 'the pass'
            $failed = (Get-Outcome $read 'crates/a/src/lib.rs::m::fails')[0]
            Assert-Equal 'fail' $failed.Outcome 'the failure'
            Assert-Equal 'it broke' $failed.Note 'and what it printed'
            Assert-Equal 0 $read.Problems.Count 'nothing was unreadable'
        }
    },
    @{
        Name = 'the-same-test-name-in-two-executables-is-two-tests'
        Run  = {
            $read = Read-Capture @(
                (New-Header 'a-111.exe'), (New-Started 'm::same'), (New-Result 'm::same' 'ok'), $suiteOk,
                (New-Header 'b-222.exe'), (New-Started 'm::same'), (New-Result 'm::same' 'failed'), $suiteOk) $mapOne
            Assert-Equal 'pass' (Get-Outcome $read 'crates/a/src/lib.rs::m::same')[0].Outcome 'in a'
            Assert-Equal 'fail' (Get-Outcome $read 'crates/b/src/lib.rs::m::same')[0].Outcome 'in b, which is not a'
        }
    },
    @{
        Name = 'a-test-that-started-and-never-reported-is-a-failure-at-the-next-executable-and-at-the-end'
        Run  = {
            $read = Read-Capture @(
                (New-Header 'a-111.exe'), (New-Started 'm::dies'), (New-Started 'm::fine'), (New-Result 'm::fine' 'ok'),
                (New-Header 'b-222.exe'), (New-Started 'm::also_dies')) $mapOne
            $dies = @(Get-Outcome $read 'crates/a/src/lib.rs::m::dies')
            Assert-Equal 1 $dies.Count 'it is counted'
            Assert-Equal 'fail' $dies[0].Outcome 'as a failure, when the next executable begins'
            Assert-Match 'executable stopped' $dies[0].Note 'and says why'
            Assert-Equal 'pass' (Get-Outcome $read 'crates/a/src/lib.rs::m::fine')[0].Outcome 'one that finished is not caught up in it'
            Assert-Equal 'fail' (Get-Outcome $read 'crates/b/src/lib.rs::m::also_dies')[0].Outcome 'and at the end of the capture'
        }
    },
    @{
        Name = 'lines-that-are-not-events-are-passed-over-and-a-broken-event-is-reported'
        Run  = {
            $read = Read-Capture @(
                '   Compiling thing v0.0.0', '', 'warning: unused', (New-Header 'a-111.exe'), 'running 1 test',
                '{ this is not json', (New-Started 'm::t'), (New-Result 'm::t' 'ok'),
                '{ "type": "test", "event": "timeout", "name": "m::t" }', $suiteOk) $mapOne
            Assert-Equal 1 $read.Outcomes.Count 'the test is counted once'
            Assert-Equal 1 $read.Problems.Count 'the line that began like an event and was not one is reported'
            Assert-Match 'not JSON' $read.Problems[0] 'as such'
        }
    },
    @{
        Name = 'an-executable-the-build-did-not-produce-is-an-error-not-a-skipped-target'
        Run  = {
            $threw = $false
            try { Read-Capture @((New-Header 'unknown-999.exe'), (New-Started 'm::t'), (New-Result 'm::t' 'ok')) $mapOne | Out-Null }
            catch { $threw = $true; Assert-Match 'unknown-999.exe' $_.Exception.Message 'it names the executable' }
            Assert-True $threw 'tests under a target nobody can name would be recorded under none'
        }
    },
    @{
        Name = 'an-event-before-any-executable-is-an-error'
        Run  = {
            $threw = $false
            try { Read-Capture @((New-Started 'm::t'), (New-Result 'm::t' 'ok')) $mapOne | Out-Null } catch { $threw = $true }
            Assert-True $threw 'it cannot be keyed'
        }
    },
    # ---- the record ----
    @{
        Name = 'a-first-run-counts-one-run-and-a-pass-starts-a-run-of-passes'
        Run  = {
            $r = Add-ProbationRun -Record (New-ProbationRecord) -Outcomes (New-Run @(, @('k', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t0
            $t = $r.tests['k']
            Assert-Equal 1 $t.runs 'runs'; Assert-Equal 1 $t.passes 'passes'; Assert-Equal 1 $t.streak 'streak'
            Assert-Equal 'pass' $t.last 'last'; Assert-Equal $t0 $t.firstAt 'first seen'; Assert-Equal 'r1' $r.latestRun 'the latest run'
        }
    },
    @{
        Name = 'a-failure-ends-the-run-of-passes-and-a-pass-after-it-starts-another'
        Run  = {
            $r = New-ProbationRecord
            foreach ($step in @(@('r1', 'pass'), @('r2', 'pass'), @('r3', 'pass'), @('r4', 'fail'), @('r5', 'pass'))) {
                $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('k', $step[1]))) -RunId $step[0] -Commit $sha1 -At $t1
            }
            $t = $r.tests['k']
            Assert-Equal 5 $t.runs 'every run counts'
            Assert-Equal 4 $t.passes 'a failure is not a pass'
            Assert-Equal 1 $t.streak 'the run of passes since the failure, and not the three before it'
            Assert-Equal 'pass' $t.last 'last'
            $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('k', 'fail'))) -RunId 'r6' -Commit $sha1 -At $t1
            Assert-Equal 0 $r.tests['k'].streak 'a failure leaves no run of passes'
            Assert-Equal 'fail' $r.tests['k'].last 'and is the last'
        }
    },
    @{
        Name = 'the-same-run-applied-twice-is-counted-once-and-a-new-run-is-counted'
        Run  = {
            $r = Add-ProbationRun -Record (New-ProbationRecord) -Outcomes (New-Run @(, @('k', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t0
            $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('k', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t1
            Assert-Equal 1 $r.tests['k'].runs 'a retry cannot count a run twice'
            Assert-Equal $t0 $r.latestAt 'and changes nothing'
            $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('k', 'pass'))) -RunId 'r1.2' -Commit $sha1 -At $t1
            Assert-Equal 2 $r.tests['k'].runs 'a re-run is another run of the tests'
        }
    },
    @{
        Name = 'a-test-a-run-did-not-report-keeps-its-counts-and-the-latest-run-moves-on'
        Run  = {
            $r = Add-ProbationRun -Record (New-ProbationRecord) -Outcomes (New-Run @(@('a', 'pass'), @('b', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t0
            $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('a', 'pass'))) -RunId 'r2' -Commit $sha2 -At $t1
            Assert-Equal 1 $r.tests['b'].runs 'b was not in r2 and keeps what it had'
            Assert-Equal 'r1' $r.tests['b'].lastRun 'and is last seen at r1'
            Assert-Equal 'r2' $r.latestRun 'the latest run is r2'
            Assert-Equal $sha2 $r.tests['a'].lastCommit 'a was seen at the later commit'
            $r = Add-ProbationRun -Record $r -Outcomes @() -RunId 'r3' -Commit $sha2 -At $t2
            Assert-Equal 'r3' $r.latestRun 'a run that selected nothing is still the latest'
        }
    },
    @{
        Name = 'the-record-text-is-ordinal-lf-and-reads-back-as-written'
        Run  = {
            $r = Add-ProbationRun -Record (New-ProbationRecord) -Outcomes (New-Run @(@('b', 'pass'), @('B', 'fail'), @('a', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t0
            $text = ConvertTo-ProbationRecordText $r
            Assert-True (-not $text.Contains("`r")) 'LF only, whatever the host'
            Assert-True ($text.IndexOf('"B"') -lt $text.IndexOf('"a"') -and $text.IndexOf('"a"') -lt $text.IndexOf('"b"')) 'ordinal, so a capital sorts before a lower-case letter'
            $back = ConvertFrom-ProbationRecordText $text
            Assert-Equal $text (ConvertTo-ProbationRecordText $back) 'it reads back to the same bytes'
            Assert-Equal 'fail' $back.tests['B'].last 'with its values'
            Assert-Equal 'pass' $back.tests['b'].last 'and b is not B: a test name is case-sensitive, and two can differ only by case'
            Assert-Equal 3 $back.tests.Count 'three tests, not two'
        }
    },
    @{
        Name = 'a-record-of-another-version-is-refused'
        Run  = {
            $threw = $false
            try { ConvertFrom-ProbationRecordText '{ "version": 2, "tests": [] }' | Out-Null } catch { $threw = $true; Assert-Match 'version 2' $_.Exception.Message 'it says which' }
            Assert-True $threw 'a layout this does not know is not read as if it were this one'
        }
    },
    # ---- the report ----
    @{
        Name = 'a-test-is-a-candidate-at-the-threshold-and-not-a-pass-short-of-it'
        Run  = {
            $r = New-ProbationRecord
            foreach ($i in 1..5) { $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(@('five', 'pass'), @('four', $(if ($i -le 4) { 'pass' } else { 'fail' })))) -RunId "r$i" -Commit $sha1 -At $t1 }
            $rows = @{}
            foreach ($row in (Get-ProbationReport -Record $r -Threshold 5)) { $rows[$row.Test] = $row }
            Assert-Equal 'candidate' $rows['five'].Status 'five passes in a row at a threshold of five'
            Assert-Equal 'failing' $rows['four'].Status 'a test whose last run failed is not a candidate'
            $rows = @{}
            foreach ($row in (Get-ProbationReport -Record $r -Threshold 6)) { $rows[$row.Test] = $row }
            Assert-Equal 'accruing' $rows['five'].Status 'five passes in a row at a threshold of six'
        }
    },
    @{
        Name = 'a-test-the-latest-run-did-not-report-is-absent-however-long-its-streak'
        Run  = {
            $r = New-ProbationRecord
            foreach ($i in 1..4) { $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('gone', 'pass'))) -RunId "r$i" -Commit $sha1 -At $t1 }
            $r = Add-ProbationRun -Record $r -Outcomes (New-Run @(, @('other', 'pass'))) -RunId 'r5' -Commit $sha1 -At $t1
            $row = @(Get-ProbationReport -Record $r -Threshold 2) | Where-Object { $_.Test -eq 'gone' }
            Assert-Equal 'not in the latest run' $row.Status 'it was promoted, renamed or deleted, and is not a candidate for what it is no longer'
            Assert-Equal 4 $row.Streak 'its counts are kept'
        }
    },
    @{
        Name = 'a-threshold-below-one-is-refused-and-an-empty-record-says-so'
        Run  = {
            $threw = $false
            try { Get-ProbationReport -Record (New-ProbationRecord) -Threshold 0 | Out-Null } catch { $threw = $true }
            Assert-True $threw 'a candidate with no passes would be every test'
            Assert-Match 'No test has been on probation' ((Format-ProbationReport -Record (New-ProbationRecord) -Threshold 5) -join "`n") 'an empty record is said to be empty'
        }
    },
    @{
        Name = 'the-report-is-a-table-and-says-the-threshold-is-not-a-decision'
        Run  = {
            $r = Add-ProbationRun -Record (New-ProbationRecord) -Outcomes (New-Run @(, @('crates/a/src/lib.rs::probation::t', 'pass'))) -RunId 'r1' -Commit $sha1 -At $t0
            $text = (Format-ProbationReport -Record $r -Threshold 5) -join "`n"
            Assert-Match '\| `crates/a/src/lib.rs::probation::t` \| 1 \| 1 \| 1 \| pass \| accruing \|' $text 'a row'
            Assert-Match 'working position, not a decision' $text 'and what the threshold is'
            Assert-Match 'run r1' $text 'and what the latest run was'
        }
    },
    # ---- the publisher ----
    @{
        Name = 'an-absent-branch-is-created-parentless-with-the-record-and-a-readme'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            Assert-Equal $null (Get-Tip $remote) 'the branch does not exist yet'
            $r = Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r1'
            Assert-Equal 0 $r.Exit $r.Output
            $parents = @((Invoke-NativeStdout { git --git-dir $remote rev-list --parents -n 1 'refs/heads/probation-results' }) -split ' ')
            Assert-Equal 1 $parents.Count 'the first commit has no parent, so the branch shares no history with main'
            $names = [string[]]@(Invoke-NativeStdout { git --git-dir $remote ls-tree --name-only 'refs/heads/probation-results' })
            [Array]::Sort($names, [System.StringComparer]::Ordinal)
            Assert-Equal 'README.md,results.json' ($names -join ',') 'the record and its explanation and nothing else'
            $held = Read-Published $remote
            Assert-Equal 1 $held.tests['k'].streak 'the record has the run'
            Assert-Equal $sha1 $held.latestCommit 'and the commit it tested'
        }
    },
    @{
        Name = 'a-second-run-is-applied-on-top-as-a-fast-forward-and-a-run-of-passes-carries-over'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            Assert-Equal 0 (Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r1').Exit 'first'
            $tip = Get-Tip $remote
            $r = Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r2' -At $t2 -Commit $sha2
            Assert-Equal 0 $r.Exit $r.Output
            $held = Read-Published $remote
            Assert-Equal 2 $held.tests['k'].streak 'two passes in a row across two runs'
            Assert-Equal 2 $held.tests['k'].runs 'two runs'
            $parent = ([string](@(Invoke-NativeStdout { git --git-dir $remote rev-parse 'refs/heads/probation-results^' })[0])).Trim()
            Assert-Equal $tip $parent 'the second commit is on the first'
        }
    },
    @{
        Name = 'the-same-run-published-twice-commits-once'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $file = New-OutcomesFile @(, @('k', 'pass'))
            Assert-Equal 0 (Invoke-Publisher $clone $file 'r1').Exit 'first'
            $tip = Get-Tip $remote
            $r = Invoke-Publisher $clone $file 'r1'
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'already holds run r1' $r.Output 'it says so'
            Assert-Equal $tip (Get-Tip $remote) 'the branch did not move'
            Assert-Equal 1 (Read-Published $remote).tests['k'].runs 'and the run was counted once'
        }
    },
    @{
        Name = 'a-run-that-selected-nothing-creates-no-branch-but-moves-the-latest-run-of-one-that-exists'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $none = New-OutcomesFile @()
            $r = Invoke-Publisher $clone $none 'r1'
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'selected no test' $r.Output 'it says why there is nothing'
            Assert-Equal $null (Get-Tip $remote) 'and the branch was not made, empty'
            Assert-Equal 0 (Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r2').Exit 'a test is recorded'
            Assert-Equal 0 (Invoke-Publisher $clone $none 'r3').Exit 'then a run selects none'
            $held = Read-Published $remote
            Assert-Equal 'r3' $held.latestRun 'the latest run moved, so the test reads as not in it'
            Assert-Equal 1 $held.tests['k'].runs 'and its counts are kept'
        }
    },
    @{
        Name = 'an-unreachable-remote-is-refused-and-is-not-read-as-an-absent-branch'
        Run  = {
            $clone = New-Clone (Join-Path (New-Root) 'not-a-repository.git')
            $r = Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r1' -Extra @{ Attempts = 2 }
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match 'could not ask' $r.Output 'it says it could not ask'
        }
    },
    @{
        Name = 'a-publisher-that-loses-a-race-applies-its-run-on-top-of-the-winners'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote; $other = New-Clone $remote
            $raceFile = New-OutcomesFile @(, @('k', 'pass'))
            # Built from the values, not from variables: it runs inside the publisher's scope.
            $race = [scriptblock]::Create("& '$publisher' -Outcomes '$raceFile' -RunId 'r2' -Commit '$sha2' -At '2026-01-02T12:00:00Z' -RepositoryRoot '$other' -RetryDelaySeconds 0 6>&1 | Out-Null")
            Assert-Equal 0 (Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r1').Exit 'first'
            $r = Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r3' -At $t2 -Extra @{ BeforePush = $race }
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'Attempt 1 of 5' $r.Output 'the first attempt was refused'
            $held = Read-Published $remote
            Assert-Equal 3 $held.tests['k'].runs 'the winner is counted and not overwritten, and so is ours'
            Assert-Equal 'r3' $held.latestRun 'ours is the latest'
        }
    },
    @{
        Name = 'a-branch-whose-name-only-ends-the-same-is-not-the-branch'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            # ls-remote matches a pattern against the tail of each ref, so a bare name also finds this.
            Invoke-Native { git -C $clone push --quiet $remote 'HEAD:refs/heads/other/probation-results' } | Out-Null
            $r = Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) 'r1'
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Equal 1 (Read-Published $remote).tests['k'].runs 'the branch was made and not mistaken for the other one'
            Assert-Equal 1 (Get-CommitCount $remote) 'as a branch of its own'
        }
    },
    @{
        Name = 'a-test-name-that-is-not-ascii-survives-the-branch'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $name = "crates/a/src/lib.rs::probation::caf$([char]0x00e9)_$([char]0x4e2d)"
            Assert-Equal 0 (Invoke-Publisher $clone (New-OutcomesFile @(, @($name, 'pass'))) 'r1').Exit 'published'
            Assert-True ((Read-Published $remote).tests.Contains($name)) 'it is read back as it was written'
        }
    },
    @{
        Name = 'missing-or-unreadable-outcomes-are-refused'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $r = Invoke-Publisher $clone (Join-Path (New-Root) 'absent.json') 'r1'
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Match 'No outcomes' $r.Output 'it says what is missing'
            $bad = Join-Path (New-Root) 'bad.json'
            [System.IO.File]::WriteAllText($bad, '{ "version": 9, "outcomes": [] }')
            Assert-Equal 2 (Invoke-Publisher $clone $bad 'r1').Exit 'a version it does not read'
            Assert-Equal $null (Get-Tip $remote) 'and nothing was published'
        }
    },
    # ---- the report script ----
    @{
        Name = 'the-report-script-reads-the-branch-and-a-record-file-alike'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            foreach ($i in 1..3) { Assert-Equal 0 (Invoke-Publisher $clone (New-OutcomesFile @(, @('k', 'pass'))) "r$i").Exit "run $i" }
            $text = & $reporter -RepositoryRoot $clone -Threshold 3 6>&1 | Out-String -Width $captureWidth
            Assert-Equal 0 $LASTEXITCODE $text
            Assert-Match '\| `k` \| 3 \| 3 \| 3 \| pass \| candidate \|' $text 'from the branch: a candidate at three in a row'
            $file = Join-Path (New-Root) 'results.json'
            [System.IO.File]::WriteAllText($file, (ConvertTo-ProbationRecordText (Read-Published $remote)))
            $text = & $reporter -Record $file -Threshold 4 6>&1 | Out-String -Width $captureWidth
            Assert-Match '\| `k` \| 3 \| 3 \| 3 \| pass \| accruing \|' $text 'from a file, at a higher threshold'
            $summary = Join-Path (New-Root) 'summary.md'
            & $reporter -Record $file -SummaryPath $summary 6>&1 | Out-Null
            Assert-Match 'Probation tier' ([System.IO.File]::ReadAllText($summary)) 'and appended to the step summary'
        }
    },
    @{
        Name = 'the-report-script-says-an-absent-branch-is-empty-and-an-unreachable-remote-is-an-error'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $text = & $reporter -RepositoryRoot $clone 6>&1 | Out-String -Width $captureWidth
            Assert-Equal 0 $LASTEXITCODE $text
            Assert-Match 'No test has been on probation' $text 'no branch is no tests'
            $broken = New-Clone (Join-Path (New-Root) 'not-a-repository.git')
            $text = & $reporter -RepositoryRoot $broken 6>&1 | Out-String -Width $captureWidth
            Assert-Equal 1 $LASTEXITCODE $text
            Assert-Match 'could not ask' $text 'a remote that cannot be asked is not an empty record'
        }
    },
    # ---- run-tier.ps1, against real workspaces ----
    @{
        Name = 'a-tier-run-records-each-test-and-selects-only-its-own-tier'
        Run  = {
            $root = New-Fixture @{
                'src\lib.rs' = @'
#[test] fn plain() {}
#[test] #[ignore = "needs a share"] fn other_ignored() {}
mod probation {
    #[test] #[ignore = "probation: new"] fn passes() {}
    #[test] #[ignore = "probation: new"] fn fails() { panic!("it broke"); }
}
mod long_haul { #[test] #[ignore = "long-haul: 1s"] fn heavy() {} }
'@
            }
            $probation = Invoke-Tier $root 'probation'
            Assert-True ($probation.Exit -ne 0) "a failing test fails the step -- $($probation.Output)"
            Assert-Equal 'pass' (Get-Verdict $probation 'src/lib.rs::probation::passes') 'the pass is keyed by its source path'
            Assert-Equal 'fail' (Get-Verdict $probation 'src/lib.rs::probation::fails') 'and the failure'
            Assert-Equal 2 @($probation.Outcomes.outcomes).Count 'nothing else was selected: not the long-haul test, not a test ignored for another reason, not one that is not ignored'
            Assert-Match 'FAILED src/lib.rs::probation::fails' $probation.Output 'the failure is named'
            Assert-Match 'it broke' $probation.Output 'with what it printed'
            $long = Invoke-Tier $root 'long_haul'
            Assert-Equal 0 $long.Exit "the long-haul tier has nothing failing -- $($long.Output)"
            Assert-Equal 1 @($long.Outcomes.outcomes).Count 'one test'
            Assert-Equal 'pass' (Get-Verdict $long 'src/lib.rs::long_haul::heavy') 'the long-haul one'
        }
    },
    @{
        Name = 'a-tier-runs-one-test-at-a-time-unless-asked-for-more'
        Run  = {
            # Each test holds a shared counter for a while and fails if it finds another test holding it.
            $source = @(
                'use std::sync::atomic::{AtomicUsize, Ordering};',
                'use std::time::Duration;',
                'static ACTIVE: AtomicUsize = AtomicUsize::new(0);',
                'fn hold() {',
                '    let others = ACTIVE.fetch_add(1, Ordering::SeqCst);',
                '    std::thread::sleep(Duration::from_millis(600));',
                '    ACTIVE.fetch_sub(1, Ordering::SeqCst);',
                '    assert_eq!(others, 0, "another test was running at the same time");',
                '}',
                'mod probation {',
                '    #[test] #[ignore = "probation: new"] fn first() { super::hold(); }',
                '    #[test] #[ignore = "probation: new"] fn second() { super::hold(); }',
                '}') -join "`n"
            $root = New-Fixture @{ 'src\lib.rs' = $source }
            $alone = Invoke-Tier $root 'probation'
            Assert-Equal 0 $alone.Exit "by default they run one at a time -- $($alone.Output)"
            Assert-Equal 2 @($alone.Outcomes.outcomes | Where-Object { $_.outcome -eq 'pass' }).Count 'both passed'
            $together = Invoke-Tier $root 'probation' @{ TestThreads = 2 }
            Assert-True ($together.Exit -ne 0) "asked for two at once, they overlap and one fails, which is the choice being made -- $($together.Output)"
        }
    },
    @{
        Name = 'a-test-that-aborts-its-executable-is-recorded-as-failing-and-other-executables-still-report'
        Run  = {
            $root = New-Fixture @{
                'src\lib.rs'       = @'
mod probation { #[test] #[ignore = "probation: new"] fn aborts() { std::process::abort(); } }
'@
                'tests\second.rs'  = @'
mod probation { #[test] #[ignore = "probation: new"] fn passes() {} }
'@
            }
            $r = Invoke-Tier $root 'probation'
            Assert-True ($r.Exit -ne 0) "the step fails -- $($r.Output)"
            Assert-Equal 'fail' (Get-Verdict $r 'src/lib.rs::probation::aborts') 'a test that killed its process is a failure, not a test never run'
            Assert-Equal 'pass' (Get-Verdict $r 'tests/second.rs::probation::passes') 'the next executable ran and reported, as --no-fail-fast says'
        }
    },
    @{
        Name = 'a-tier-is-read-whatever-colour-cargo-is-told-to-use'
        Run  = {
            $root = New-Fixture @{ 'src\lib.rs' = 'mod probation { #[test] #[ignore = "probation: new"] fn passes() {} }' }
            $previous = $env:CARGO_TERM_COLOR
            $env:CARGO_TERM_COLOR = 'always'
            try { $r = Invoke-Tier $root 'probation' }
            finally { if ($null -eq $previous) { Remove-Item Env:\CARGO_TERM_COLOR -ErrorAction SilentlyContinue } else { $env:CARGO_TERM_COLOR = $previous } }
            Assert-Equal 0 $r.Exit "a host that asks for colour does not change what a run says -- $($r.Output)"
            Assert-Equal 'pass' (Get-Verdict $r 'src/lib.rs::probation::passes') 'the test was recorded'
        }
    },
    @{
        Name = 'a-cargo-argument-with-a-space-reaches-the-run-as-one-argument'
        Run  = {
            $root = New-Fixture @{ 'src\lib.rs' = 'mod probation { #[test] #[ignore = "probation: new"] fn passes() {} }' }
            $target = Join-Path (New-Root) 'a %USERNAME% target dir'
            $r = Invoke-Tier $root 'probation' @{} @('--lib', '--no-fail-fast', '--target-dir', $target)
            Assert-Equal 0 $r.Exit "the build and the run both took the directory whole, a percent sign included -- $($r.Output)"
            Assert-Equal 'pass' (Get-Verdict $r 'src/lib.rs::probation::passes') 'and the test was recorded'
            # A batch file replaces %USERNAME% with the value; the run that did would build into that directory instead.
            $expanded = Join-Path (Split-Path -Parent $target) ('a ' + $env:USERNAME + ' target dir')
            Assert-True (-not (Test-Path -LiteralPath $expanded)) 'the percent signs were passed as they are, and not read as a variable'
            $quote = Invoke-Tier $root 'probation' @{} @('--lib', '--cfg=a"b')
            Assert-Equal 2 $quote.Exit "an argument cmd cannot carry is refused, not half passed -- $($quote.Output)"
        }
    },
    @{
        Name = 'a-line-that-is-not-an-event-fails-the-step-and-writes-no-outcomes'
        Run  = {
            # A child that inherits the test run's stdout writes into the capture past libtest, so a line
            # that is not a test event lands among the events.
            $root = New-Fixture @{
                'src\lib.rs' = @'
mod probation {
    #[test] #[ignore = "probation: new"] fn first() {
        std::process::Command::new("cmd").args(["/c", "echo { this is not an event"]).status().unwrap();
    }
    #[test] #[ignore = "probation: new"] fn second() {}
}
'@
            }
            $r = Invoke-Tier $root 'probation'
            Assert-Equal 2 $r.Exit "tests that passed do not make a run whose capture could not be read whole -- $($r.Output)"
            Assert-Equal $null $r.Outcomes 'no partial record is written for the publisher to take as the whole run'
            Assert-Match 'unread: not JSON' $r.Output 'the line is named'
        }
    },
    @{
        Name = 'a-tier-with-no-test-selected-writes-an-empty-outcomes-file-and-succeeds'
        Run  = {
            $root = New-Fixture @{ 'src\lib.rs' = '#[test] fn plain() {}' }
            $r = Invoke-Tier $root 'probation'
            Assert-Equal 0 $r.Exit "nothing failed -- $($r.Output)"
            Assert-Equal 0 @($r.Outcomes.outcomes).Count 'an empty list, which is not the same as no file'
            Assert-Match 'probation tier: 0 passed, 0 failed' $r.Output 'and it says so'
        }
    },
    @{
        Name = 'tests-that-do-not-build-fail-the-step-and-write-no-outcomes'
        Run  = {
            $root = New-Fixture @{ 'src\lib.rs' = 'this is not rust' }
            $r = Invoke-Tier $root 'probation'
            Assert-True ($r.Exit -ne 0) "the step fails -- $($r.Output)"
            Assert-Equal $null $r.Outcomes 'a build that failed is not a run with no tests'
            Assert-Match 'could not build' $r.Output 'it says the build failed'
        }
    }
)

$selected = $cases | Where-Object { $_.Name -like $Name }
if (-not $selected) {
    Write-Host "No cases matched '$Name'." -ForegroundColor Red
    exit 1
}

$failures = 0
foreach ($case in $selected) {
    try {
        & $case.Run
        Write-Host "  PASS  $($case.Name)" -ForegroundColor Green
    }
    catch {
        $failures++
        Write-Host "  FAIL  $($case.Name)" -ForegroundColor Red
        Write-Host "        $($_.Exception.Message)" -ForegroundColor Red
    }
    finally {
        Remove-Roots
    }
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "test-probation: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-probation: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
