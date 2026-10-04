# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-encoding.ps1](check-encoding.ps1).

.DESCRIPTION
    Every rule this covers is bidirectional: the check must REJECT what it
    exists to catch and ACCEPT what is legitimate. A guard tested in one
    direction only is the failure this repository keeps paying for -- a rule
    that rejects everything passes a rejection test, and a rule that was
    accidentally disabled passes an acceptance test.

    The opt-out markers are why this file exists. `allow-mojibake` and
    `allow-glued-doc-comment` each switch a rule off for a file, so each is one
    typo away from switching off more than it claims. The cases below pin what
    each marker suppresses AND what it must still leave armed -- which is the
    part a hand-run check cannot keep true, because the person who runs it
    closes the terminal afterwards.

    Fixtures are written under TEMP and the checker is pointed at them with
    `-Path`, so nothing here depends on the state of the working tree.

    WHY NOT PESTER: see the same note in
    [test-run-sabotage.ps1](test-run-sabotage.ps1). The sibling tools are
    standalone and dependency-free, and CI invokes them directly.

    WHY ONE HOST, where [test-common.ps1](test-common.ps1) insists on two: the
    tool under test does not parse under Windows PowerShell 5.1. It carries
    curated mojibake as its own pattern examples and has no BOM, so 5.1 reads
    it as ANSI and the pattern table is a syntax error. That is a property of
    `check-encoding.ps1`, not of this suite, and CI already pins `shell: pwsh`
    for it. Running here under 5.1 is therefore a hard failure with that
    explanation rather than a silent single-host pass.

.PARAMETER Name
    Optional wildcard filter over case names, to re-run just one.
#>
[CmdletBinding()]
param(
    [string] $Name = '*'
)

$ErrorActionPreference = 'Stop'

$checker = Join-Path $PSScriptRoot 'check-encoding.ps1'
if (-not (Test-Path -LiteralPath $checker)) {
    throw "check-encoding.ps1 not found beside this script at $checker"
}

if ($PSVersionTable.PSEdition -ne 'Core') {
    Write-Host "test-check-encoding requires PowerShell 7 (pwsh)." -ForegroundColor Red
    Write-Host "check-encoding.ps1 holds curated mojibake and carries no BOM, so Windows" -ForegroundColor Red
    Write-Host "PowerShell 5.1 decodes it as ANSI and cannot parse it at all. CI runs it" -ForegroundColor Red
    Write-Host "with 'shell: pwsh' for the same reason. Re-run this under pwsh." -ForegroundColor Red
    exit 1
}

# The three markers under test, spelled once. A test that retyped them would
# pass while the checker looked for something else.
$mojibakeMarker = 'encoding-check: allow-mojibake'
$gluedMarker = 'encoding-check: allow-glued-doc-comment'

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

function Assert-NoMatch {
    param([string] $Pattern, [string] $Text, [string] $Because = '')
    if ($Text -match $Pattern) {
        throw "expected NO match for [$Pattern] -- $Because`n--- output ---`n$Text"
    }
}

<#
.SYNOPSIS
    Run the checker over one fixture file and report its verdict.

.DESCRIPTION
    Returns the exit code and the combined output. The fixture is written as
    UTF-8 WITHOUT a BOM via .NET rather than Set-Content, because the thing
    under test is an encoding checker and PowerShell's own default encoding
    differs between the two hosts this must pass on.
#>
function Invoke-Checker {
    param([string] $FileName, [string] $Content)

    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("check-encoding-test-" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir | Out-Null
    try {
        $file = Join-Path $dir $FileName
        [System.IO.File]::WriteAllText($file, $Content, (New-Object System.Text.UTF8Encoding $false))
        $output = & $checker -Path $dir *>&1 | Out-String
        return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
    }
    finally {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# A doc marker welded to a non-space character: what rule 3 exists to catch.
$glued = "fn main() {`n    let x = 1;/// welded`n}`n"
# The same shape a banner comment has, which must NOT be flagged.
$banner = "//////////////////////`n// an ordinary banner`nfn main() {}`n"
# Latin-1 mojibake, spelled as the bytes a UTF-8 'e-acute' misread as 1252.
$mojibake = "// caf" + [char]0xC3 + [char]0xA9 + " was decoded wrongly`n"

$cases = @(
    @{
        Name = 'glued-doc-comment-is-rejected'
        Run  = {
            $r = Invoke-Checker -FileName 'a.rs' -Content $glued
            Assert-True ($r.ExitCode -ne 0) 'a welded doc marker must fail the check'
            Assert-Match 'GLUED DOC COMMENT' $r.Output 'and must say which rule fired'
        }
    },
    @{
        Name = 'ordinary-source-is-accepted'
        Run  = {
            $r = Invoke-Checker -FileName 'a.rs' -Content "/// A doc comment.`nfn main() {}`n"
            Assert-True ($r.ExitCode -eq 0) 'clean source must pass'
        }
    },
    @{
        Name = 'banner-comment-is-accepted'
        Run  = {
            $r = Invoke-Checker -FileName 'a.rs' -Content $banner
            Assert-True ($r.ExitCode -eq 0) 'a //// banner is not a welded doc marker'
        }
    },
    @{
        Name = 'glued-marker-suppresses-the-glued-rule'
        Run  = {
            $r = Invoke-Checker -FileName 'a.rs' -Content "// $gluedMarker`n$glued"
            Assert-True ($r.ExitCode -eq 0) 'the marker must switch rule 3 off for this file'
        }
    },
    @{
        Name = 'glued-marker-leaves-mojibake-armed'
        Run  = {
            # The narrowness of the opt-out is the whole claim. A marker that
            # skipped the file entirely would pass the case above and silently
            # take the mojibake rule with it.
            $r = Invoke-Checker -FileName 'a.rs' -Content "// $gluedMarker`n$glued$mojibake"
            Assert-True ($r.ExitCode -ne 0) 'the glued marker must not disable the mojibake rule'
            Assert-Match 'MOJIBAKE' $r.Output 'and mojibake is what must still be reported'
            Assert-NoMatch 'GLUED DOC COMMENT' $r.Output 'while the opted-out rule stays quiet'
        }
    },
    @{
        Name = 'glued-marker-does-not-leak-to-other-files'
        Run  = {
            # Two files in one run: the marker is a per-file opt-out, so a
            # neighbour must still be judged on its own content.
            $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("check-encoding-test-" + [guid]::NewGuid().ToString('N'))
            New-Item -ItemType Directory -Path $dir | Out-Null
            try {
                $utf8 = New-Object System.Text.UTF8Encoding $false
                [System.IO.File]::WriteAllText((Join-Path $dir 'opted-out.rs'), "// $gluedMarker`n$glued", $utf8)
                [System.IO.File]::WriteAllText((Join-Path $dir 'neighbour.rs'), $glued, $utf8)
                $output = & $checker -Path $dir *>&1 | Out-String
                Assert-True ($LASTEXITCODE -ne 0) 'the unmarked neighbour must still be flagged'
                Assert-Match 'neighbour\.rs' $output 'and must be named'
                Assert-NoMatch 'opted-out\.rs' $output 'while the marked file is not'
            }
            finally {
                Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
            }
        }
    },
    @{
        Name = 'mojibake-is-rejected'
        Run  = {
            $r = Invoke-Checker -FileName 'a.md' -Content $mojibake
            Assert-True ($r.ExitCode -ne 0) 'mojibake must fail the check'
            Assert-Match 'MOJIBAKE' $r.Output 'and must say which rule fired'
        }
    },
    @{
        Name = 'mojibake-marker-suppresses-the-mojibake-rule'
        Run  = {
            $r = Invoke-Checker -FileName 'a.md' -Content "$mojibakeMarker`n$mojibake"
            Assert-True ($r.ExitCode -eq 0) 'the documented opt-out must work'
        }
    },
    @{
        Name = 'mojibake-marker-leaves-the-glued-rule-armed'
        Run  = {
            # The reciprocal of the narrowness case above, for the older marker.
            $r = Invoke-Checker -FileName 'a.rs' -Content "// $mojibakeMarker`n$glued"
            Assert-True ($r.ExitCode -ne 0) 'the mojibake marker must not disable rule 3'
            Assert-Match 'GLUED DOC COMMENT' $r.Output 'rule 3 is what must still be reported'
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
}

Write-Host ''
if ($failures -gt 0) {
    Write-Host "test-check-encoding: $failures of $($selected.Count) case(s) failed." -ForegroundColor Red
    exit 1
}
Write-Host "test-check-encoding: all $($selected.Count) case(s) passed." -ForegroundColor Green
exit 0
