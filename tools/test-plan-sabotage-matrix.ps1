# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [plan-sabotage-matrix.ps1](plan-sabotage-matrix.ps1).

.DESCRIPTION
    The planner decides which jobs `.github/workflows/sabotage-sweep.yml` runs,
    so a defect in it is invisible in the worst way: the workflow still goes
    green, having swept less than it claimed or nothing at all. That is exactly
    the failure the planner was written to remove, and it is not the kind of
    thing a reader notices in a diff.

    Every case here is therefore BIDIRECTIONAL. A planner that returned an
    empty matrix for every input would satisfy "a bad name produces no jobs";
    a planner that ignored its filter entirely would satisfy "a good name
    produces jobs". Neither is acceptable, so both directions are pinned.

    Most cases build a throwaway `crates/` tree under TEMP and point the
    planner at it with `-RepositoryRoot`, so the arithmetic is tested against
    manifests whose entry counts this file chooses. The two cases that do run
    against the real repository assert only STRUCTURE -- that every manifest is
    represented, and that shard indices are contiguous -- never a count, which
    would go stale the next time a sabotage is added.

    WHY NOT PESTER: see the same note in
    [test-run-sabotage.ps1](test-run-sabotage.ps1). The sibling tools are
    standalone and dependency-free, and CI invokes them directly.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$planner = Join-Path $PSScriptRoot 'plan-sabotage-matrix.ps1'
if (-not (Test-Path -LiteralPath $planner)) {
    throw "plan-sabotage-matrix.ps1 not found beside this script at $planner"
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot

function Assert-True {
    param([bool] $Condition, [string] $Message)
    if (-not $Condition) { throw $Message }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $Message)
    if ($Expected -ne $Actual) { throw "$Message (expected '$Expected', got '$Actual')" }
}

# Every fixture directory this suite creates, so the runner can remove them all
# after each case -- INCLUDING when the case threw, which is when a leftover is
# least likely to be noticed and most likely to confuse the next run. The
# sibling `test-run-sabotage.ps1` does this with a per-case `finally`; recording
# them in one place here means a case added later cannot forget to.
$script:fixtureRoots = New-Object System.Collections.Generic.List[string]

# A fresh temp directory path, registered for cleanup. Nothing in this suite may
# mint a fixture path any other way, or it will be the one that leaks.
function New-FixtureRootPath {
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("plan-matrix-" + [guid]::NewGuid().ToString('N'))
    $script:fixtureRoots.Add($root) | Out-Null
    return $root
}

function Remove-Fixtures {
    foreach ($root in $script:fixtureRoots) {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
    $script:fixtureRoots.Clear()
}

<#
Build a throwaway repository root holding one `crates/<name>/sabotage.json` per
entry of $Layout, whose value is how many sabotages that manifest declares.
Only the count matters to the planner, so the entries carry nothing else.
#>
function New-FixtureRoot {
    param([hashtable] $Layout)

    $root = New-FixtureRootPath
    foreach ($crate in $Layout.Keys) {
        $dir = Join-Path (Join-Path $root 'crates') $crate
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $sabotages = @(1..[math]::Max(1, $Layout[$crate]) | ForEach-Object { @{ name = "s$_"; file = 'src/lib.rs' } })
        if ($Layout[$crate] -eq 0) { $sabotages = @() }
        $spec = @{ package = $crate; sabotages = $sabotages }
        Set-Content -LiteralPath (Join-Path $dir 'sabotage.json') `
            -Value ($spec | ConvertTo-Json -Depth 5) -Encoding utf8
    }
    return $root
}

# Run the planner and return the parsed matrix. Errors propagate, which is what
# the refusal cases catch.
function Invoke-Planner {
    param([string] $Root, [string] $Manifest = '', [int] $EntriesPerShard = 10, [string] $GitHubOutput = '')
    $json = & $planner -RepositoryRoot $Root -Manifest $Manifest -EntriesPerShard $EntriesPerShard -GitHubOutput $GitHubOutput
    return ($json | ConvertFrom-Json)
}

$cases = @(
    @{
        Name = 'every-manifest-crate-is-represented'
        Run  = {
            # Structural, against the real tree: no count is asserted, only
            # that nothing with a manifest is silently left unswept.
            $m = Invoke-Planner -Root $repositoryRoot
            $planned = @($m.include | ForEach-Object { $_.crate } | Sort-Object -Unique)
            $onDisk = @(Get-ChildItem -Path (Join-Path $repositoryRoot 'crates') -Directory |
                    Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'sabotage.json') } |
                    ForEach-Object { $_.Name } | Sort-Object)
            Assert-Equal ($onDisk -join ',') ($planned -join ',') 'every crate with a manifest must appear in the matrix'
        }
    },
    @{
        Name = 'shard-indices-are-contiguous-and-unique'
        Run  = {
            $m = Invoke-Planner -Root $repositoryRoot
            foreach ($group in ($m.include | Group-Object crate)) {
                $shards = @($group.Group | ForEach-Object { $_.shard } | Sort-Object)
                $declared = @($group.Group | ForEach-Object { $_.shards } | Sort-Object -Unique)
                Assert-Equal 1 $declared.Count "$($group.Name) must declare one shard count"
                Assert-Equal $declared[0] $shards.Count "$($group.Name) must emit one job per shard"
                Assert-Equal ((0..($declared[0] - 1)) -join ',') ($shards -join ',') "$($group.Name) shard indices must be 0..n-1 with no gaps or repeats"
            }
        }
    },
    @{
        Name = 'shard-count-rounds-up'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 25 }
            $m = Invoke-Planner -Root $root
            Assert-Equal 3 @($m.include).Count '25 entries at 10 per shard is three shards'
        }
    },
    @{
        Name = 'an-exact-multiple-does-not-gain-a-shard'
        Run  = {
            # The boundary an off-by-one in the ceiling would move.
            $root = New-FixtureRoot @{ 'crate-a' = 20 }
            $m = Invoke-Planner -Root $root
            Assert-Equal 2 @($m.include).Count '20 entries at 10 per shard is exactly two shards'
        }
    },
    @{
        Name = 'a-small-manifest-gets-one-shard'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 3 }
            $m = Invoke-Planner -Root $root
            Assert-Equal 1 @($m.include).Count 'fewer entries than the target is still one shard, never zero'
        }
    },
    @{
        Name = 'entries-per-shard-is-honoured'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 20 }
            $m = Invoke-Planner -Root $root -EntriesPerShard 5
            Assert-Equal 4 @($m.include).Count 'the tuning knob must actually change the split'
        }
    },
    @{
        Name = 'a-named-manifest-selects-only-that-crate'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 25; 'crate-b' = 25 }
            $m = Invoke-Planner -Root $root -Manifest 'crates/crate-b/sabotage.json'
            $crates = @($m.include | ForEach-Object { $_.crate } | Sort-Object -Unique)
            Assert-Equal 'crate-b' ($crates -join ',') 'a named manifest must exclude the others'
            Assert-Equal 3 @($m.include).Count 'and must still be sharded'
        }
    },
    @{
        Name = 'a-named-manifest-accepts-backslashes'
        Run  = {
            # The workflow passes a forward-slash path; a human dispatching by
            # hand on Windows will paste a backslashed one.
            $root = New-FixtureRoot @{ 'crate-a' = 3; 'crate-b' = 3 }
            $m = Invoke-Planner -Root $root -Manifest 'crates\crate-a\sabotage.json'
            Assert-Equal 'crate-a' (@($m.include | ForEach-Object { $_.crate } | Sort-Object -Unique) -join ',') 'a backslashed path must resolve'
        }
    },
    @{
        Name = 'an-unknown-manifest-fails-rather-than-emitting-nothing'
        Run  = {
            # THE case this file exists for. An empty matrix would make every
            # sweep job vanish and the run report success having done nothing.
            $root = New-FixtureRoot @{ 'crate-a' = 3 }
            $failed = $false
            try { Invoke-Planner -Root $root -Manifest 'crates/typo/sabotage.json' }
            catch { $failed = $true; $message = $_.Exception.Message }
            Assert-True $failed 'an unknown manifest must be an error, not an empty matrix'
            Assert-True ($message -match 'matches no sabotage manifest') 'and must say so plainly'
            Assert-True ($message -match 'crate-a') 'and must list what it does know, so the typo is obvious'
        }
    },
    @{
        Name = 'a-manifest-outside-crates-is-unknown'
        Run  = {
            # Existing on disk is not the test; being a manifest the matrix
            # covers is. A path that exists but is not swept must still refuse.
            $root = New-FixtureRoot @{ 'crate-a' = 3 }
            $stray = Join-Path $root 'sabotage.json'
            Set-Content -LiteralPath $stray -Value '{"sabotages":[{"name":"s1"}]}' -Encoding utf8
            $failed = $false
            try { Invoke-Planner -Root $root -Manifest 'sabotage.json' } catch { $failed = $true }
            Assert-True $failed 'a manifest outside crates/ is not part of the matrix and must be refused'
        }
    },
    @{
        Name = 'an-empty-manifest-is-refused'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 0 }
            $failed = $false
            try { Invoke-Planner -Root $root } catch { $failed = $true }
            Assert-True $failed 'a manifest declaring no sabotages would emit a job that sweeps nothing'
        }
    },
    @{
        Name = 'a-root-with-no-manifests-is-refused'
        Run  = {
            $root = New-FixtureRootPath
            New-Item -ItemType Directory -Path (Join-Path $root 'crates') -Force | Out-Null
            $failed = $false
            try { Invoke-Planner -Root $root } catch { $failed = $true }
            Assert-True $failed 'an empty matrix must never be the answer'
        }
    },
    @{
        Name = 'a-nonsense-entries-per-shard-is-refused'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 3 }
            $failed = $false
            try { Invoke-Planner -Root $root -EntriesPerShard 0 } catch { $failed = $true }
            Assert-True $failed 'zero entries per shard would divide by zero or loop forever'
        }
    },
    @{
        Name = 'the-github-output-line-is-written'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 3 }
            $out = Join-Path $root 'gh-output.txt'
            Set-Content -LiteralPath $out -Value '' -Encoding utf8
            Invoke-Planner -Root $root -GitHubOutput $out | Out-Null
            $written = @(Get-Content -LiteralPath $out | Where-Object { $_ -like 'matrix=*' })
            Assert-Equal 1 $written.Count 'exactly one matrix= line must be appended'
            # A step output is line-oriented, so the JSON must not wrap.
            Assert-True ($written[0] -match '^matrix=\{"include":\[') 'and must carry compact JSON on that one line'
        }
    },
    @{
        Name = 'the-matrix-is-a-single-line'
        Run  = {
            $root = New-FixtureRoot @{ 'crate-a' = 25; 'crate-b' = 25 }
            $json = & $planner -RepositoryRoot $root
            Assert-Equal 1 @($json).Count 'the planner must emit one line, or the step output truncates'
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
        Remove-Fixtures
    }
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "test-plan-sabotage-matrix: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-plan-sabotage-matrix: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
