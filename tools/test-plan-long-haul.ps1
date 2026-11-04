# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [plan-long-haul.ps1](plan-long-haul.ps1).

.DESCRIPTION
    The planner is what makes the long-haul workflow run "daily, and only when main has moved". A defect in it
    is quiet in both directions: one that always skips leaves the tier never running while the schedule stays
    green, and one that always runs spends a workspace build and every long test each day to repeat a result.

    Cases are BIDIRECTIONAL: each rule that skips is paired with the neighbouring case that must run, so a
    planner that did either unconditionally fails one of them. The earlier runs come from a recorded file, and
    the path that asks GitHub is driven through a fake `gh` on PATH.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$planner = Join-Path $PSScriptRoot 'plan-long-haul.ps1'
if (-not (Test-Path -LiteralPath $planner)) { throw "$planner not found beside this script" }

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
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ('long-haul-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    $script:roots.Add($path) | Out-Null
    return $path
}

function Remove-Roots {
    foreach ($path in $script:roots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
    $script:roots.Clear()
}

# Captured messages must not wrap: on Windows PowerShell 5.1 `Out-String` wraps captured host output at the
# host's width, which on a CI runner is narrow.
$captureWidth = 4096

$a = 'a' * 40
$b = 'b' * 40
$c = 'c' * 40

# A recorded run: @(id, sha, conclusion, hour). The hour orders them, since the planner must not rely on the
# order they are listed in.
function New-RunsFile {
    param([object[]] $Runs)
    $path = Join-Path (New-Root) 'runs.json'
    $items = foreach ($r in $Runs) {
        [ordered]@{ databaseId = $r[0]; headSha = $r[1]; conclusion = $r[2]; createdAt = ('2026-01-01T{0:00}:00:00Z' -f $r[3]) }
    }
    $json = if (@($items).Count -eq 0) { '[]' } else { ConvertTo-Json -InputObject @($items) }
    [System.IO.File]::WriteAllText($path, $json)
    return $path
}

function Invoke-Planner {
    param([string] $EventName, [string] $Sha, [string] $RunId = '100', [string] $RunsPath, [string] $Output)
    $arguments = @{ EventName = $EventName; Sha = $Sha; CurrentRunId = $RunId }
    if ($RunsPath) { $arguments.RunsPath = $RunsPath }
    if ($Output) { $arguments.GitHubOutput = $Output }
    $text = & $planner @arguments 6>&1 | Out-String -Width $captureWidth
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $text }
}

function Assert-Plan {
    param([string] $Expected, $Result, [string] $Message)
    Assert-Equal 0 $Result.Exit "$Message -- $($Result.Output)"
    Assert-Match "long-haul: $Expected --" $Result.Output $Message
}

# A `gh` that records its arguments and prints a file, or fails, standing in for the real one while `Run`
# executes. Run is given the directory the arguments are recorded in, as args.txt.
function Use-FakeGh {
    param([string] $Prints, [int] $ExitCode, [scriptblock] $Run)
    $dir = New-Root
    $record = "@echo %* > `"%~dp0args.txt`"`r`n"
    $body = if ($Prints) { "$record@type `"$Prints`"`r`n@exit /b $ExitCode`r`n" } else { "$record@echo gh: no luck 1>&2`r`n@exit /b $ExitCode`r`n" }
    [System.IO.File]::WriteAllText((Join-Path $dir 'gh.cmd'), "@echo off`r`n$body")
    $saved = $env:PATH
    $env:PATH = "$dir;$saved"
    try { & $Run $dir } finally { $env:PATH = $saved }
}

$cases = @(
    @{
        Name = 'a-dispatch-always-runs-even-on-a-commit-that-already-ran'
        Run  = {
            $runs = New-RunsFile @(, @(1, $a, 'success', 1))
            Assert-Plan 'run' (Invoke-Planner -EventName workflow_dispatch -Sha $a -RunsPath $runs) 'the engineer asked'
            Assert-Plan 'run' (Invoke-Planner -EventName push -Sha $a -RunsPath $runs) 'and so is any trigger but the schedule'
        }
    },
    @{
        Name = 'a-dispatch-does-not-ask-github-so-a-failing-gh-cannot-warn-about-it'
        Run  = {
            Use-FakeGh -Prints '' -ExitCode 1 -Run {
                param($dir)
                $r = Invoke-Planner -EventName workflow_dispatch -Sha $a
                Assert-Plan 'run' $r 'a dispatch runs'
                Assert-True (-not ($r.Output -match '::warning::')) "a dispatch has no use for the listing, so its failure is not reported -- $($r.Output)"
                Assert-True (-not (Test-Path -LiteralPath (Join-Path $dir 'args.txt'))) 'gh was not run at all'
            }
        }
    },
    @{
        Name = 'a-schedule-with-no-earlier-run-runs'
        Run  = {
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $a -RunsPath (New-RunsFile @())) 'the first run ever'
        }
    },
    @{
        Name = 'a-schedule-on-the-commit-the-last-run-completed-on-skips'
        Run  = {
            $runs = New-RunsFile @(, @(1, $a, 'success', 1))
            $r = Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs
            Assert-Plan 'skip' $r 'main has not moved'
            Assert-Match 'run 1' $r.Output 'it names the run it matched'
        }
    },
    @{
        Name = 'a-schedule-after-main-moved-runs'
        Run  = {
            $runs = New-RunsFile @(, @(1, $a, 'success', 1))
            $r = Invoke-Planner -EventName schedule -Sha $b -RunsPath $runs
            Assert-Plan 'run' $r 'main moved'
            Assert-Match 'main moved from' $r.Output 'it says so'
        }
    },
    @{
        Name = 'a-failed-or-timed-out-run-counts-as-completed'
        Run  = {
            foreach ($conclusion in @('failure', 'timed_out', 'success')) {
                $runs = New-RunsFile @(, @(1, $a, $conclusion, 1))
                Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs) "a $conclusion run on an unchanged main is not repeated"
            }
        }
    },
    @{
        Name = 'a-cancelled-or-skipped-run-does-not-count-as-completed'
        Run  = {
            foreach ($conclusion in @('cancelled', 'skipped', 'startup_failure', 'action_required', '')) {
                $runs = New-RunsFile @(, @(1, $a, $conclusion, 1))
                Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs) "a '$conclusion' run did not complete, so the commit has not been run"
            }
        }
    },
    @{
        Name = 'a-cancelled-run-is-passed-over-for-the-completed-one-before-it'
        Run  = {
            $runs = New-RunsFile @(@(1, $a, 'success', 1), @(2, $b, 'cancelled', 2))
            Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs) 'the last completed run is the one on a'
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $b -RunsPath $runs) 'the cancelled run on b ran nothing'
        }
    },
    @{
        Name = 'only-the-most-recent-completed-run-decides'
        Run  = {
            # main went a, then b. A run on a is old news now, so a is not skipped just because it once ran.
            $runs = New-RunsFile @(@(1, $a, 'success', 1), @(2, $b, 'success', 2))
            Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $b -RunsPath $runs) 'b is the last one run'
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs) 'a ran before b, and the last run was on b'
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $c -RunsPath $runs) 'c is new'
        }
    },
    @{
        Name = 'the-order-the-runs-are-listed-in-does-not-matter'
        Run  = {
            $oldestFirst = New-RunsFile @(@(1, $a, 'success', 1), @(2, $b, 'success', 2))
            $newestFirst = New-RunsFile @(@(2, $b, 'success', 2), @(1, $a, 'success', 1))
            foreach ($file in @($oldestFirst, $newestFirst)) {
                Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $b -RunsPath $file) 'b was last'
                Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $file) 'a was not'
            }
        }
    },
    @{
        Name = 'this-run-is-never-counted-as-an-earlier-one'
        Run  = {
            $runs = New-RunsFile @(, @(100, $a, 'success', 1))
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $a -RunId 100 -RunsPath $runs) 'the only run listed is this one'
            Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $a -RunId 101 -RunsPath $runs) 'the same listing, for a different run'
        }
    },
    @{
        Name = 'the-answer-is-written-to-the-step-output-file'
        Run  = {
            $out = Join-Path (New-Root) 'output.txt'
            $runs = New-RunsFile @(, @(1, $a, 'success', 1))
            Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $a -RunsPath $runs -Output $out) 'skipping'
            $text = [System.IO.File]::ReadAllText($out)
            Assert-Match '(?m)^run=false$' $text 'a skip is run=false'
            Assert-Match '(?m)^reason=main has not moved' $text 'with its reason'
            $out2 = Join-Path (New-Root) 'output.txt'
            Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $b -RunsPath $runs -Output $out2) 'running'
            Assert-Match '(?m)^run=true$' ([System.IO.File]::ReadAllText($out2)) 'a run is run=true'
        }
    },
    @{
        Name = 'a-missing-runs-file-is-a-planner-that-could-not-be-asked'
        Run  = {
            $r = Invoke-Planner -EventName schedule -Sha $a -RunsPath (Join-Path (New-Root) 'absent.json')
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Match 'No runs file' $r.Output 'it says what is missing'
        }
    },
    @{
        Name = 'the-runs-come-from-gh-when-no-file-is-given'
        Run  = {
            $runs = New-RunsFile @(, @(1, $a, 'success', 1))
            Use-FakeGh -Prints $runs -ExitCode 0 -Run {
                param($dir)
                Assert-Plan 'skip' (Invoke-Planner -EventName schedule -Sha $a) 'gh listed a completed run on this commit'
                Assert-Plan 'run' (Invoke-Planner -EventName schedule -Sha $b) 'and one on another'
                $asked = [System.IO.File]::ReadAllText((Join-Path $dir 'args.txt'))
                Assert-Match '\brun list\b' $asked 'it lists runs'
                Assert-Match '--workflow long-haul\.yml\b' $asked 'of this workflow and no other, whose commits would match for the wrong reason'
                Assert-Match '--branch main\b' $asked 'on main, which is the branch whose movement is asked about'
                Assert-Match '--status completed\b' $asked 'that have completed'
            }
        }
    },
    @{
        Name = 'a-gh-that-fails-runs-with-a-warning-rather-than-skipping'
        Run  = {
            Use-FakeGh -Prints '' -ExitCode 1 -Run {
                $r = Invoke-Planner -EventName schedule -Sha $a
                Assert-Plan 'run' $r 'not being able to ask is not evidence that nothing changed'
                Assert-Match '::warning::could not list earlier runs' $r.Output 'and it says it could not'
            }
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
    Write-Host "test-plan-long-haul: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-plan-long-haul: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
