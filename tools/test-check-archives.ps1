# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-archives.ps1](check-archives.ps1).

.DESCRIPTION
    Every rule is tested in both directions: the checker must REJECT what it
    exists to catch and ACCEPT what is legitimate, because a rule that rejects
    everything passes a rejection test and a rule that was switched off passes
    an acceptance test.

    Each case builds a throwaway git repository under TEMP, commits a base, then
    changes the working tree and runs the checker against `-Base HEAD`, so the
    immutability rule compares the change with the commit before it. Nothing
    here reads this repository's own archives.

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

$checker = Join-Path $PSScriptRoot 'check-archives.ps1'
if (-not (Test-Path -LiteralPath $checker)) {
    throw "check-archives.ps1 not found beside this script at $checker"
}

function Assert-True {
    param([bool] $Condition, [string] $Because = '')
    if (-not $Condition) { throw "expected true -- $Because" }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $Because = '')
    if ($Text -notmatch $Pattern) {
        throw "expected a match for [$Pattern] -- $Because`n--- output ---`n$Text"
    }
}

function Write-Fixture {
    param([string] $Path, [string] $Content)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path -LiteralPath $parent)) { New-Item -ItemType Directory -Path $parent | Out-Null }
    [System.IO.File]::WriteAllText($Path, $Content, (New-Object System.Text.UTF8Encoding $false))
}

<#
.SYNOPSIS
    Commit `Base` (a map of relative path to content) in a new repository, apply
    `Change` (a map of path to content, or to $null to delete), and run the
    checker. Returns the exit code and output.
#>
function Invoke-Case {
    param([hashtable] $Base, [hashtable] $Change = @{}, [string[]] $Arguments = @('-Base', 'HEAD'))

    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("check-archives-test-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $root | Out-Null
    try {
        & git -C $root init --quiet 2>&1 | Out-Null
        foreach ($path in $Base.Keys) { Write-Fixture (Join-Path $root $path) $Base[$path] }
        & git -C $root add -A 2>&1 | Out-Null
        & git -C $root -c user.name=test -c user.email=test@example.invalid commit --quiet -m base 2>&1 | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "fixture commit failed in $root" }
        foreach ($path in $Change.Keys) {
            $full = Join-Path $root $path
            if ($null -eq $Change[$path]) { Remove-Item -LiteralPath $full } else { Write-Fixture $full $Change[$path] }
        }
        $output = & pwsh -NoProfile -File $checker -Root $root @Arguments *>&1 | Out-String
        [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
    }
    finally {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
}

$archive = 'crate/COMPLETED-CHECKLIST.md'
$good = @"
# Completed checklist

## Moved 2026-10-19 12:00:00 -04:00 -- M1: a group

### <a id="m11"></a>M1.1 -- An item. *(completed 2026-10-19)*

Body text, with an issue #12 and C# mentioned in passing.

- [x] **M1.1** -- the item as it stood.
"@ + "`n"

$cases = @(
    @{
        Name = 'a-clean-archive-is-accepted'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good }
            Assert-True ($r.ExitCode -eq 0) "a clean archive must pass: $($r.Output)"
            Assert-Match 'Archives clean: 1 file' $r.Output 'and say what it checked'
        }
    },
    @{
        Name = 'a-heading-without-a-blank-line-above-is-rejected-at-every-level'
        Run  = {
            foreach ($heading in '## Moved later -- M2', '### <a id="m12"></a>M1.2 -- x', '#### A detail') {
                $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $good + "text`n$heading`n" }
                Assert-True ($r.ExitCode -eq 1) "[$heading] directly under text must fail"
                Assert-Match 'HEADING WITHOUT A BLANK LINE ABOVE' $r.Output 'and say which rule fired'
            }
        }
    },
    @{
        Name = 'a-heading-on-the-first-line-needs-no-blank-above'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = "# Title`n`ntext`n" }
            Assert-True ($r.ExitCode -eq 0) "line 1 has nothing above it: $($r.Output)"
        }
    },
    @{
        Name = 'a-heading-glued-onto-a-line-is-rejected-in-both-shapes'
        Run  = {
            foreach ($glued in 'text. ### <a id="m12"></a>M1.2 -- x', 'text. ## Moved 2026-10-20 -- M2') {
                $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $good + "`n$glued`n" }
                Assert-True ($r.ExitCode -eq 1) "[$glued] must fail"
                Assert-Match 'HEADING GLUED ONTO A LINE' $r.Output 'and say which rule fired'
            }
        }
    },
    @{
        Name = 'an-open-box-is-rejected-and-a-ticked-one-accepted'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $good + "`n- [ ] **M1.2** -- still open`n" }
            Assert-True ($r.ExitCode -eq 1) 'an open box must fail'
            Assert-Match 'OPEN BOX IN AN ARCHIVE' $r.Output 'and say which rule fired'
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $good + "`n  - [x] nested and ticked`n" }
            Assert-True ($r.ExitCode -eq 0) "a ticked box must pass: $($r.Output)"
        }
    },
    @{
        Name = 'lines-inside-a-code-fence-are-not-headings-or-boxes'
        Run  = {
            $fenced = $good + "`n``````markdown`ntext`n## not a heading`n- [ ] not a box`n``````n"
            $r = Invoke-Case -Base @{ $archive = $fenced }
            Assert-True ($r.ExitCode -eq 0) "fenced lines must be skipped: $($r.Output)"
        }
    },
    @{
        Name = 'only-completed-checklists-are-linted'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good; 'crate/CHECKLIST.md' = "text`n## M2`n- [ ] open`n" }
            Assert-True ($r.ExitCode -eq 0) "a live checklist is not an archive: $($r.Output)"
        }
    },
    @{
        Name = 'an-untracked-new-archive-is-linted'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ 'other/COMPLETED-CHECKLIST.md' = "text`n- [ ] open`n" }
            Assert-True ($r.ExitCode -eq 1) 'a new archive not yet added must still be checked'
            Assert-Match 'other/COMPLETED-CHECKLIST.md' $r.Output 'and be named'
        }
    },
    @{
        Name = 'a-changed-heading-is-rejected'
        Run  = {
            $changed = $good -replace 'M1: a group', 'M1: a group, renamed'
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $changed }
            Assert-True ($r.ExitCode -eq 1) 'rewriting an archived heading must fail'
            Assert-Match 'HEADING CHANGED OR DELETED' $r.Output 'and say which rule fired'
        }
    },
    @{
        Name = 'a-removed-anchor-is-rejected'
        Run  = {
            $changed = $good -replace '<a id="m11"></a>', ''
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $changed }
            Assert-True ($r.ExitCode -eq 1) 'removing an anchor must fail'
            Assert-Match 'ANCHOR REMOVED.*m11' $r.Output 'and name the anchor'
        }
    },
    @{
        Name = 'a-deleted-archive-is-rejected'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $null }
            Assert-True ($r.ExitCode -eq 1) 'deleting an archive must fail'
            Assert-Match 'ARCHIVE DELETED' $r.Output 'and say which rule fired'
        }
    },
    @{
        Name = 'body-corrections-and-appended-entries-are-accepted'
        Run  = {
            # The acceptance half of rule 4: this repository's rules require
            # repointing links and allow factual corrections, so only headings and
            # anchors are fixed.
            $changed = ($good -replace 'issue #12', 'issue #13') + "`n## Moved 2026-10-20 -- M2: another`n`ntext`n"
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $changed }
            Assert-True ($r.ExitCode -eq 0) "a body edit and an appended group must pass: $($r.Output)"
        }
    },
    @{
        Name = 'an-unresolvable-base-is-a-usage-error-and-no-history-skips-the-rule'
        Run  = {
            $r = Invoke-Case -Base @{ $archive = $good } -Arguments @('-Base', 'no-such-ref')
            Assert-True ($r.ExitCode -eq 2) "an unknown base must exit 2: $($r.Output)"
            $changed = $good -replace 'M1: a group', 'M1: renamed'
            $r = Invoke-Case -Base @{ $archive = $good } -Change @{ $archive = $changed } -Arguments @('-NoHistory')
            Assert-True ($r.ExitCode -eq 0) "-NoHistory must skip rule 4: $($r.Output)"
            Assert-Match 'history not checked' $r.Output 'and say it did'
        }
    }
)

$failures = 0
$ran = 0
foreach ($case in $cases | Where-Object { $_.Name -like $Name }) {
    $ran++
    try {
        & $case.Run
        Write-Host "  PASS  $($case.Name)"
    }
    catch {
        $failures++
        Write-Host "  FAIL  $($case.Name)" -ForegroundColor Red
        Write-Host "        $($_.Exception.Message)" -ForegroundColor Red
    }
}

if ($ran -eq 0) {
    Write-Host "No case matches '$Name'." -ForegroundColor Red
    exit 1
}
if ($failures -gt 0) {
    Write-Host "$failures of $ran cases failed." -ForegroundColor Red
    exit 1
}
Write-Host "All $ran cases passed."
exit 0
