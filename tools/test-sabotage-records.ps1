# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [publish-sabotage-catchers.ps1](publish-sabotage-catchers.ps1) and
    [fetch-sabotage-catchers.ps1](fetch-sabotage-catchers.ps1).

.DESCRIPTION
    These two move the record of which tests caught each sabotage entry through a
    git remote, so a defect in either is quiet in the worst way: the workflow
    still goes green, having published less than it claimed, or a clone adopts a
    record that quietly makes its next sweep smaller than it should be.

    Every case runs against a throwaway bare repository standing in for the
    remote, and clones of it standing in for CI and for a developer's machine.
    Nothing here touches the real repository or a network.

    Cases are BIDIRECTIONAL where a guard has two sides. A publisher that read an
    unreachable remote as an empty branch would pass "an absent branch is
    created"; a publisher that refused every absent branch would pass "an
    unreachable remote is refused". Both are pinned.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$publisher = Join-Path $PSScriptRoot 'publish-sabotage-catchers.ps1'
$fetcher = Join-Path $PSScriptRoot 'fetch-sabotage-catchers.ps1'
foreach ($script in @($publisher, $fetcher)) {
    if (-not (Test-Path -LiteralPath $script)) { throw "$script not found beside this script" }
}

. (Join-Path $PSScriptRoot 'common.ps1')

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

# Every directory a case makes, removed after it, including when it threw.
$script:roots = New-Object System.Collections.Generic.List[string]

function New-Root {
    $path = Join-Path ([System.IO.Path]::GetTempPath()) ('sabotage-records-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $path | Out-Null
    $script:roots.Add($path) | Out-Null
    return $path
}

function Remove-Roots {
    foreach ($path in $script:roots) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
    $script:roots.Clear()
}

$identity = @('-c', 'user.name=t', '-c', 'user.email=t@t', '-c', 'commit.gpgsign=false')

# A bare repository to stand in for the remote.
function New-Remote {
    $path = Join-Path (New-Root) 'remote.git'
    Invoke-Native { git init --bare --quiet $path } | Out-Null
    return $path
}

# A repository with one commit, whose `origin` is `Remote`.
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

function Get-Head {
    param([string] $Clone)
    return ([string](@(Invoke-NativeStdout { git -C $Clone rev-parse HEAD }) | Select-Object -First 1)).Trim()
}

# A record file holding `Entries`, each @{ M = manifest; E = entry; T = tests; At = ...; C = commit }.
function New-RecordFile {
    param([object[]] $Entries)
    $record = New-SabotageCatcherRecord
    foreach ($e in $Entries) {
        Set-SabotageCatcher -Record $record -Manifest $e.M -Entry $e.E -Tests ([string[]]$e.T) -At $e.At -Commit $e.C
    }
    $path = Join-Path (New-Root) 'catchers.json'
    Write-SabotageCatcherRecord -Path $path -Record $record
    return $path
}

# What the remote's `sabotage-records` branch holds, as a record; empty when it has none.
function Read-Published {
    param([string] $Remote)
    $lines = @(Invoke-NativeStdout { git --git-dir $Remote show 'refs/heads/sabotage-records:catchers.json' })
    $path = Join-Path (New-Root) 'published.json'
    if ($LASTEXITCODE -eq 0) { [System.IO.File]::WriteAllText($path, (($lines -join "`n") + "`n")) }
    return (Read-SabotageCatcherRecord -Path $path)
}

function Get-Tip {
    param([string] $Remote)
    $tip = @(Invoke-NativeStdout { git --git-dir $Remote rev-parse --verify --quiet 'refs/heads/sabotage-records' }) | Select-Object -First 1
    if ($LASTEXITCODE -ne 0) { return $null }
    return ([string]$tip).Trim()
}

function Get-CommitCount {
    param([string] $Remote)
    return [int]((@(Invoke-NativeStdout { git --git-dir $Remote rev-list --count 'refs/heads/sabotage-records' }) | Select-Object -First 1))
}

function Invoke-Publisher {
    param([string] $Clone, [string] $Record, [hashtable] $Extra = @{}, [string] $Remote = 'origin')
    $output = & $publisher -Record $Record -RepositoryRoot $Clone -Remote $Remote -RetryDelaySeconds 0 @Extra 6>&1 | Out-String
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $output }
}

function Invoke-Fetcher {
    param([string] $Clone, [string] $Output, [string] $Remote = 'origin')
    $text = & $fetcher -RepositoryRoot $Clone -Output $Output -Remote $Remote 6>&1 | Out-String
    return [pscustomobject]@{ Exit = $LASTEXITCODE; Output = $text }
}

function Assert-Entry {
    param([hashtable] $Record, [string] $Manifest, [string] $Entry, [string[]] $Tests, [string] $At)
    Assert-True ($Record.ContainsKey($Manifest) -and $Record[$Manifest].ContainsKey($Entry)) "the record has $Manifest / $Entry"
    $value = $Record[$Manifest][$Entry]
    Assert-Equal ($Tests -join ',') (@($value.Tests) -join ',') "$Entry's tests"
    Assert-Equal $At $value.At "$Entry's time"
}

$m1 = 'crates/a/sabotage.json'
$m2 = 'crates/b/sabotage.json'
$t0 = '2026-01-01T00:00:00.0000000Z'
$t1 = '2026-01-02T00:00:00.0000000Z'
$t2 = '2026-01-03T00:00:00.0000000Z'
$t3 = '2026-01-04T00:00:00.0000000Z'
$sha = 'a' * 40

$cases = @(
    @{
        Name = 'an-absent-branch-is-created-parentless-with-the-record-and-a-readme'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $record = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('x::t'); At = $t1; C = $sha })
            Assert-Equal $null (Get-Tip $remote) 'the branch does not exist yet'
            $r = Invoke-Publisher $clone $record
            Assert-Equal 0 $r.Exit $r.Output
            $parents = @((Invoke-NativeStdout { git --git-dir $remote rev-list --parents -n 1 'refs/heads/sabotage-records' }) -split ' ')
            Assert-Equal 1 $parents.Count 'the first commit has no parent, so the branch shares no history with main'
            $names = [string[]]@(Invoke-NativeStdout { git --git-dir $remote ls-tree --name-only 'refs/heads/sabotage-records' })
            [Array]::Sort($names, [System.StringComparer]::Ordinal)
            Assert-Equal 'README.md,catchers.json' ($names -join ',') 'the branch holds the record and its explanation and nothing else'
            Assert-Entry (Read-Published $remote) $m1 'e1' @('x::t') $t1
        }
    },
    @{
        Name = 'a-second-publish-merges-and-keeps-the-latest-of-each-entry'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $first = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('old'); At = $t1; C = $sha },
                @{ M = $m1; E = 'e2'; T = @('kept'); At = $t3; C = $sha })
            $second = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('new'); At = $t2; C = $sha },
                @{ M = $m2; E = 'e3'; T = @('other'); At = $t0; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $clone $first).Exit 'first'
            $tip = Get-Tip $remote
            Assert-Equal 0 (Invoke-Publisher $clone $second).Exit 'second'
            $held = Read-Published $remote
            Assert-Entry $held $m1 'e1' @('new') $t2
            Assert-Entry $held $m1 'e2' @('kept') $t3
            Assert-Entry $held $m2 'e3' @('other') $t0
            $parent = ([string](@(Invoke-NativeStdout { git --git-dir $remote rev-parse 'refs/heads/sabotage-records^' }) | Select-Object -First 1)).Trim()
            Assert-Equal $tip $parent 'the second commit is on the first, a fast-forward'
        }
    },
    @{
        Name = 'a-record-with-nothing-newer-commits-nothing'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $current = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('new'); At = $t2; C = $sha })
            $stale = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('old'); At = $t1; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $clone $current).Exit 'first'
            $tip = Get-Tip $remote
            $r = Invoke-Publisher $clone $stale
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'nothing to publish' $r.Output 'it says so'
            Assert-Equal $tip (Get-Tip $remote) 'an older result does not move the branch'
            Assert-Entry (Read-Published $remote) $m1 'e1' @('new') $t2
            $same = Invoke-Publisher $clone $current
            Assert-Match 'nothing to publish' $same.Output 'the same record again'
            Assert-Equal $tip (Get-Tip $remote) 'nor does the same record'
        }
    },
    @{
        Name = 'publishing-leaves-the-clone-it-ran-in-untouched'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $record = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('x'); At = $t1; C = $sha })
            $head = Get-Head $clone
            $refs = (@(Invoke-NativeStdout { git -C $clone for-each-ref --format='%(refname)' refs/heads }) -join ',')
            Assert-Equal 0 (Invoke-Publisher $clone $record).Exit 'publish'
            Assert-Equal $head (Get-Head $clone) 'HEAD did not move'
            Assert-Equal $refs (@(Invoke-NativeStdout { git -C $clone for-each-ref --format='%(refname)' refs/heads }) -join ',') 'no local branch was made'
            Assert-Equal '' ((@(Invoke-NativeStdout { git -C $clone status --porcelain }) -join '')) 'the working tree and index are clean'
        }
    },
    @{
        Name = 'a-push-that-loses-a-race-is-retried-and-merges-the-winner'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote; $other = New-Clone $remote
            $ours = New-RecordFile @(@{ M = $m1; E = 'mine'; T = @('a'); At = $t1; C = $sha })
            $theirs = New-RecordFile @(@{ M = $m2; E = 'theirs'; T = @('b'); At = $t2; C = $sha })
            # Between our fetch and our push, another publisher creates the branch. The callback is
            # built from literal values: it runs inside the publisher, so a variable or function it
            # names would be looked up from there, and a `GetNewClosure()` one runs in a module scope
            # that cannot see this script's functions when the script is itself run from a nested
            # scope, as CI's `. '<script>'` wrapper does. A literal needs neither.
            $quote = { param($text) "'" + $text.Replace("'", "''") + "'" }
            $race = [scriptblock]::Create(
                "& $(& $quote $publisher) -Record $(& $quote $theirs) -RepositoryRoot $(& $quote $other) -Remote origin -RetryDelaySeconds 0 | Out-Null")
            $r = Invoke-Publisher $clone $ours @{ BeforePush = $race }
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'Attempt 1 of 5' $r.Output 'the first attempt is reported as refused'
            $held = Read-Published $remote
            Assert-Entry $held $m1 'mine' @('a') $t1
            Assert-Entry $held $m2 'theirs' @('b') $t2
            Assert-Equal 2 (Get-CommitCount $remote) 'the winner, then ours on top of it; nothing was forced'
        }
    },
    @{
        Name = 'a-push-the-remote-keeps-refusing-fails-after-every-attempt'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $hook = Join-Path $remote 'hooks\pre-receive'
            [System.IO.File]::WriteAllText($hook, "#!/bin/sh`nexit 1`n", [System.Text.UTF8Encoding]::new($false))
            $record = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('x'); At = $t1; C = $sha })
            $r = Invoke-Publisher $clone $record @{ Attempts = 3 }
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match 'after 3 attempt\(s\)' $r.Output 'it says how many it made'
            Assert-Match 'refused' $r.Output 'and why'
            Assert-Equal $null (Get-Tip $remote) 'nothing was published'
        }
    },
    @{
        Name = 'an-unreachable-remote-is-not-read-as-an-empty-branch'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $record = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('x'); At = $t1; C = $sha })
            $r = Invoke-Publisher $clone $record @{ Attempts = 2 } 'no-such-remote'
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match 'could not ask' $r.Output 'it names the failure and does not publish a root commit'
            Assert-Equal $null (Get-Tip $remote) 'nothing reached the real remote either'
        }
    },
    @{
        Name = 'a-record-that-does-not-exist-is-refused'
        Run  = {
            $remote = New-Remote; $clone = New-Clone $remote
            $r = Invoke-Publisher $clone (Join-Path (New-Root) 'missing.json')
            Assert-Equal 2 $r.Exit $r.Output
            Assert-Equal $null (Get-Tip $remote) 'nothing was published'
        }
    },
    @{
        Name = 'fetching-seeds-a-clone-and-an-entry-at-its-head-is-usable-there'
        Run  = {
            $remote = New-Remote; $ci = New-Clone $remote; $dev = New-Clone $remote
            $head = Get-Head $dev
            $record = New-RecordFile @(
                @{ M = $m1; E = 'here'; T = @('x::t'); At = $t1; C = $head },
                @{ M = $m1; E = 'elsewhere'; T = @('y::t'); At = $t1; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $ci $record).Exit 'publish'
            $output = Join-Path (New-Root) 'sabotage\catchers.json'
            $r = Invoke-Fetcher $dev $output
            Assert-Equal 0 $r.Exit $r.Output
            $adopted = Read-SabotageCatcherRecord -Path $output
            $here = Get-SabotageCatcherTests -RepositoryRoot $dev -Record $adopted -Manifest $m1 -Entry 'here'
            Assert-True $here.Usable 'an entry learned at a commit the clone has is used'
            Assert-Equal 'x::t' ($here.Tests -join ',') 'with its tests'
            $there = Get-SabotageCatcherTests -RepositoryRoot $dev -Record $adopted -Manifest $m1 -Entry 'elsewhere'
            Assert-True (-not $there.Usable) 'one learned at a commit the clone does not have is held, and not used'
        }
    },
    @{
        Name = 'fetching-merges-into-a-local-record-and-drops-what-names-no-commit'
        Run  = {
            $remote = New-Remote; $ci = New-Clone $remote; $dev = New-Clone $remote
            $published = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('published'); At = $t1; C = $sha },
                @{ M = $m1; E = 'e2'; T = @('published'); At = $t1; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $ci $published).Exit 'publish'
            # e1: a newer local result that names a commit, which wins. e2: a newer local
            # result from a tree that was no commit, which must not displace the published
            # one. e9: only here.
            $output = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('local'); At = $t3; C = $sha },
                @{ M = $m1; E = 'e2'; T = @('dirty'); At = $t3; C = '' },
                @{ M = $m2; E = 'e9'; T = @('only-here'); At = $t0; C = $sha })
            $r = Invoke-Fetcher $dev $output
            Assert-Equal 0 $r.Exit $r.Output
            $merged = Read-SabotageCatcherRecord -Path $output
            Assert-Entry $merged $m1 'e1' @('local') $t3
            Assert-Entry $merged $m1 'e2' @('published') $t1
            Assert-Entry $merged $m2 'e9' @('only-here') $t0
            Assert-Match 'dropped 1 local' $r.Output 'it says what it dropped'
        }
    },
    @{
        Name = 'fetching-with-no-branch-changes-nothing-and-is-not-an-error'
        Run  = {
            $remote = New-Remote; $dev = New-Clone $remote
            $output = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('local'); At = $t1; C = '' })
            $before = [System.IO.File]::ReadAllText($output)
            $r = Invoke-Fetcher $dev $output
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'No sabotage-records branch' $r.Output 'it says there is none'
            Assert-Equal $before ([System.IO.File]::ReadAllText($output)) 'the local record is as it was, even its entry that names no commit'
        }
    },
    @{
        Name = 'fetching-from-an-unreachable-remote-is-an-error-and-changes-nothing'
        Run  = {
            $remote = New-Remote; $dev = New-Clone $remote
            $output = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('local'); At = $t1; C = $sha })
            $before = [System.IO.File]::ReadAllText($output)
            $r = Invoke-Fetcher $dev $output 'no-such-remote'
            Assert-Equal 1 $r.Exit $r.Output
            Assert-Match 'could not ask' $r.Output 'it names the failure'
            Assert-Equal $before ([System.IO.File]::ReadAllText($output)) 'the local record is as it was'
        }
    },
    @{
        Name = 'a-branch-whose-name-merely-ends-the-same-is-not-the-record-branch'
        Run  = {
            $remote = New-Remote; $dev = New-Clone $remote
            # `ls-remote` matches a pattern against the tail of a ref's name, so a bare
            # branch name would find this one and the fetch would then fail to read it.
            Invoke-Native { git -C $dev push --quiet origin 'HEAD:refs/heads/other/sabotage-records' } | Out-Null
            $output = Join-Path (New-Root) 'catchers.json'
            $r = Invoke-Fetcher $dev $output
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'No sabotage-records branch' $r.Output 'the longer name is not it'
            Assert-True (-not (Test-Path -LiteralPath $output)) 'and nothing was written'
        }
    },
    @{
        Name = 'names-that-are-not-ascii-survive-a-publish-a-merge-and-a-fetch-under-a-console-that-is-not-utf-8'
        Run  = {
            $remote = New-Remote; $ci = New-Clone $remote; $dev = New-Clone $remote
            # Built from code points, so this file stays ASCII: Windows PowerShell 5.1 reads a
            # script with no byte order mark as the ANSI code page. An accent, two CJK
            # characters, and one outside the basic plane, which is two UTF-16 units.
            $face = [char]::ConvertFromUtf32(0x1F600)
            $entry = 'caf' + [char]0xE9 + ' ' + [char]0x65E5 + [char]0x672C + ' ' + $face
            $test = 'mod::t_' + [char]0xFC + $face
            $first = New-RecordFile @(@{ M = $m1; E = $entry; T = @($test); At = $t1; C = $sha })
            $second = New-RecordFile @(@{ M = $m2; E = 'plain'; T = @('x'); At = $t2; C = $sha })
            # PowerShell decodes a native command's output with the console's code page. This is
            # the case where that is not UTF-8, which is a Windows console's default, and the
            # one that corrupted a name while leaving the JSON around it valid. If the host will
            # not let the page be set, say so: the case then runs under the host's own.
            $page = $null
            try { $page = [Console]::OutputEncoding; [Console]::OutputEncoding = [System.Text.Encoding]::GetEncoding(437) }
            catch { $page = $null; Write-Host '        (the console code page could not be set here; this ran under the host''s own)' }
            try {
                Assert-Equal 0 (Invoke-Publisher $ci $first).Exit 'publish the name'
                # The second publish reads the branch and writes what it read back, which is where
                # a corrupted name would be put on the shared branch.
                Assert-Equal 0 (Invoke-Publisher $ci $second).Exit 'publish another, merging with the first'
                $output = Join-Path (New-Root) 'sabotage\catchers.json'
                Assert-Equal 0 (Invoke-Fetcher $dev $output).Exit 'fetch'
            }
            finally { if ($null -ne $page) { [Console]::OutputEncoding = $page } }
            $adopted = Read-SabotageCatcherRecord -Path $output
            Assert-Entry $adopted $m1 $entry @($test) $t1
            Assert-Entry $adopted $m2 'plain' @('x') $t2
            Assert-Equal 2 (@($adopted[$m1].Keys).Count + @($adopted[$m2].Keys).Count) 'and no corrupted copy beside them'
        }
    },
    @{
        Name = 'an-entry-that-names-no-commit-is-never-published-and-never-displaces-one-that-does'
        Run  = {
            $remote = New-Remote; $ci = New-Clone $remote; $dev = New-Clone $remote
            $committed = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('real'); At = $t1; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $ci $committed).Exit 'publish the committed entry'
            $tip = Get-Tip $remote
            # A sweep of a tree with uncommitted edits: newer than the published entry, naming no
            # commit, and with one entry of its own that the branch does not hold.
            $dirty = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('dirty'); At = $t3; C = '' },
                @{ M = $m1; E = 'e2'; T = @('dirty'); At = $t3; C = '' })
            $r = Invoke-Publisher $ci $dirty
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'Leaving out 2 entries' $r.Output 'it says what it left out'
            Assert-Equal $tip (Get-Tip $remote) 'nothing newer to publish, so the branch did not move'
            $held = Read-Published $remote
            Assert-Entry $held $m1 'e1' @('real') $t1
            Assert-True (-not $held[$m1].ContainsKey('e2')) 'and the entry that names no commit was not added'
            # Mixed with a committed one, the committed one goes and the other does not.
            $mixed = New-RecordFile @(
                @{ M = $m1; E = 'e1'; T = @('dirty'); At = $t3; C = '' },
                @{ M = $m2; E = 'e3'; T = @('real'); At = $t2; C = $sha })
            Assert-Equal 0 (Invoke-Publisher $ci $mixed).Exit 'publish a mixed record'
            $held = Read-Published $remote
            Assert-Entry $held $m1 'e1' @('real') $t1
            Assert-Entry $held $m2 'e3' @('real') $t2
        }
    },
    @{
        Name = 'a-record-of-only-entries-that-name-no-commit-does-not-create-the-branch'
        Run  = {
            $remote = New-Remote; $ci = New-Clone $remote
            $dirty = New-RecordFile @(@{ M = $m1; E = 'e1'; T = @('dirty'); At = $t1; C = '' })
            $r = Invoke-Publisher $ci $dirty
            Assert-Equal 0 $r.Exit $r.Output
            Assert-Match 'no entry that names a commit' $r.Output 'it says why there is nothing to publish'
            Assert-Equal $null (Get-Tip $remote) 'and the branch was not made, empty'
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
    Write-Host "test-sabotage-records: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-sabotage-records: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
