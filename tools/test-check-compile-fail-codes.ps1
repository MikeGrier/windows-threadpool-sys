# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-compile-fail-codes.ps1](check-compile-fail-codes.ps1).
    Exits 0 only if every case passes.

.DESCRIPTION
    The guard decides whether a `compile_fail` doctest names its error, and a
    guard that stopped recognising one spelling of a fence would go green over
    exactly the examples it exists to catch. So every spelling the workspace
    uses is checked in both directions -- refused when unpinned, accepted when
    pinned -- and so are the things it must not mistake for a fence.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell
    5.1 ships Pester 3, whose syntax differs incompatibly from Pester 5.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Script = Join-Path $PSScriptRoot 'check-compile-fail-codes.ps1'
$script:Failures = 0

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'heading')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'heading' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Test-Case {
    param([string] $Name, [scriptblock] $Body)
    try {
        & $Body
        Write-Report "  PASS  $Name" -Level good
    }
    catch {
        $script:Failures++
        Write-Report "  FAIL  $Name" -Level bad
        Write-Report "        $($_.Exception.Message)"
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $What)
    if ("$Expected" -ne "$Actual") {
        throw "$What -- expected '$Expected', got '$Actual'"
    }
}

function Assert-Match {
    param([string] $Pattern, [string] $Text, [string] $What)
    if ($Text -notmatch $Pattern) {
        throw "$What -- expected /$Pattern/ in: $Text"
    }
}

# A throwaway tree holding the given files, keyed by relative path.
function New-Fixture {
    param([hashtable] $Files)
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("cfc-" + [guid]::NewGuid().ToString('N'))
    foreach ($relative in $Files.Keys) {
        $path = Join-Path $root $relative
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [System.IO.File]::WriteAllText($path, ($Files[$relative] -replace "`r`n", "`n"))
    }
    return $root
}

function Invoke-Guard {
    param([string] $Root)
    # `*>&1`: the script reports through `Write-Host`, which a plain `2>&1`
    # merge does not capture.
    $output = & $script:Script -Root $Root *>&1 | Out-String
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
}

function Remove-Fixture {
    param([string] $Root)
    Remove-Item $Root -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Report "check-compile-fail-codes.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# --- Refused when unpinned ---------------------------------------------------

$unpinnedForms = [ordered]@{
    'an outer doc comment'        = "/// ``````compile_fail`n/// let x: u32 = ""no"";`n/// ``````"
    'an inner doc comment'        = "//! ``````compile_fail`n//! let x: u32 = ""no"";`n//! ``````"
    'a doc attribute (cfg_attr)'  = "#[cfg_attr(not(feature = ""x""),`n    doc = ""``````compile_fail"",`n    doc = ""let x: u32 = 1;"",`n    doc = ""``````"")]"
    'a #[doc] attribute'          = "#[doc = ""``````compile_fail""]"
    'a one-line cfg_attr'         = "#[cfg_attr(x, doc = ""``````compile_fail"", doc = ""``````"")]"
    'a rust-tagged info string'   = "/// ``````rust,compile_fail`n/// let x: u32 = ""no"";`n/// ``````"
    # CommonMark containers: rustdoc still runs a fence nested in a blockquote
    # or a list item, so the guard must still see it. Raised in PR #113's
    # review; the tilde fence below was found while fixing it.
    'a blockquote in a doc comment' = "/// > ``````compile_fail`n/// > let x: u32 = ""no"";`n/// > ``````"
    'a list item in a doc comment'  = "/// - ``````compile_fail`n///   let x: u32 = ""no"";`n///   ``````"
    'a numbered list item'          = "/// 1. ``````compile_fail`n///    ``````"
    'a blockquoted list item'       = "//! > * ``````compile_fail`n//! >   ``````"
    'a blockquote in a doc attribute' = "#[doc = ""> ``````compile_fail""]"
    'a tilde fence'                 = "/// ~~~compile_fail`n/// let x: u32 = ""no"";`n/// ~~~"
    'a blockquoted tilde fence'     = "/// > ~~~~rust,compile_fail`n/// > ~~~~"
    # Block doc comments: rustdoc reads `/** */` and `/*! */` too, strips a
    # leading ` * ` from their lines, and runs the fences inside. PR #113.
    'an outer block doc comment'    = "/** ``````compile_fail`nlet x: u32 = ""no"";`n``````*/"
    'an inner block doc comment'    = "/*! ``````compile_fail`n``````*/"
    'a starred block doc line'      = "/**`n * ``````compile_fail`n * let x: u32 = ""no"";`n * ```````n */"
    'a bare block doc line'         = "/*!`n``````compile_fail`n```````n*/"
    # Raw-string doc attributes, with and without hashes. PR #113 review.
    'a raw-string doc attribute'    = "#[doc = r""``````compile_fail""]"
    'a hashed raw-string doc'       = "#[doc = r#""``````compile_fail""#]"
    'a double-hashed raw cfg_attr'  = "#[cfg_attr(x, doc = r##""``````compile_fail""##, doc = ""``````"")]"
    'a blockquote in a raw doc'     = "#[doc = r#""> ``````compile_fail""#]"
    'a fence inside a multi-line raw doc' = "#[doc = r#""`nIntro.`n``````compile_fail`nlet x: u32 = ""no"";`n```````n""#]"
}
foreach ($form in $unpinnedForms.Keys) {
    $text = $unpinnedForms[$form]
    Test-Case "an unpinned fence in $form is refused" {
        $root = New-Fixture @{ 'a\src\lib.rs' = $text }
        try {
            $result = Invoke-Guard $root
            Assert-Equal 1 $result.ExitCode "exit code ($form)"
            Assert-Match 'a[\\/]src[\\/]lib\.rs:\d+' $result.Output 'the offending file and line are named'
        }
        finally { Remove-Fixture $root }
    }
}

Test-Case 'an unpinned fence in markdown is refused' {
    # Markdown is scanned because a crate may include_str! it as doctests.
    $root = New-Fixture @{ 'a\README.md' = "# A`n`n``````compile_fail`nlet x: u32 = ""no"";`n```````n" }
    try {
        $result = Invoke-Guard $root
        Assert-Equal 1 $result.ExitCode 'exit code'
        Assert-Match 'README\.md:3' $result.Output 'the fence line is named'
    }
    finally { Remove-Fixture $root }
}

$markdownForms = [ordered]@{
    'a markdown blockquote'   = "# A`n`n> ``````compile_fail`n> let x: u32 = ""no"";`n> ```````n"
    'a markdown list item'    = "# A`n`n+ ``````compile_fail`n  let x: u32 = ""no"";`n  ```````n"
    'a markdown tilde fence'  = "# A`n`n~~~compile_fail`nlet x: u32 = ""no"";`n~~~`n"
}
foreach ($form in $markdownForms.Keys) {
    $text = $markdownForms[$form]
    Test-Case "an unpinned fence in $form is refused" {
        $root = New-Fixture @{ 'a\README.md' = $text }
        try {
            $result = Invoke-Guard $root
            Assert-Equal 1 $result.ExitCode "exit code ($form)"
            Assert-Match 'README\.md:3' $result.Output 'the fence line is named'
        }
        finally { Remove-Fixture $root }
    }
}

# --- Accepted when pinned ----------------------------------------------------

Test-Case 'every pinned spelling is accepted' {
    $root = New-Fixture @{
        'a\src\lib.rs' = "/// ``````compile_fail,E0308`n/// ```````n//! ``````compile_fail,E0277`n//! ```````n" +
        "#[cfg_attr(x, doc = ""``````compile_fail,E0599"", doc = ""``````"")]`n" +
        "#[doc = ""``````compile_fail,E0382""]`n/// ``````rust,compile_fail,E0502`n/// ```````n"
        'a\README.md'  = "``````compile_fail,E0133`nlet x = 1;`n```````n"
        'b\src\lib.rs' = "/// > ``````compile_fail,E0308`n/// - ``````compile_fail,E0308`n/// 1. ``````compile_fail,E0308`n" +
        "//! > * ``````compile_fail,E0308`n#[doc = ""> ``````compile_fail,E0308""]`n/// ~~~compile_fail,E0308`n" +
        "/// > ~~~~rust,compile_fail,E0308`n"
        'b\README.md'  = "> ``````compile_fail,E0308`n+ ``````compile_fail,E0308`n~~~compile_fail,E0308`n"
        'c\src\lib.rs' = "/** ``````compile_fail,E0308`n``````*/`n/*! ``````compile_fail,E0308`n``````*/`n" +
        "/**`n * ``````compile_fail,E0308`n * ```````n */`n/*!`n``````compile_fail,E0308`n```````n*/`n"
        'd\src\lib.rs' = "#[doc = r""``````compile_fail,E0308""]`n#[doc = r#""``````compile_fail,E0308""#]`n" +
        "#[cfg_attr(x, doc = r##""``````compile_fail,E0308""##, doc = ""``````"")]`n" +
        "#[doc = r#""> ``````compile_fail,E0308""#]`n#[doc = r#""`n``````compile_fail,E0308`n```````n""#]`n"
    }
    try {
        $result = Invoke-Guard $root
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match '\(25 found\)' $result.Output 'every pinned fence is counted'
    }
    finally { Remove-Fixture $root }
}

# --- Not mistaken for a fence ------------------------------------------------

Test-Case 'prose, other fences and string literals are not fences' {
    $root = New-Fixture @{
        'a\src\lib.rs' = "/// A ``compile_fail`` example passes on any error.`n" +
        "/// ``````ignore`n/// ```````n/// ``````rust`n/// ```````n" +
        "const S: &str = ""compile_fail"";`n// compile_fail in a plain comment`n" +
        "/// - a ``compile_fail`` bullet, in single backticks`n/// > quoting ``compile_fail`` prose`n" +
        "/// text ~~~compile_fail mid-line is not a fence`n// > ``````compile_fail behind a plain comment`n"
    }
    try {
        $result = Invoke-Guard $root
        Assert-Equal 0 $result.ExitCode $result.Output
        Assert-Match '\(0 found\)' $result.Output 'nothing was counted'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'build and scratch directories are not scanned' {
    $bad = "/// ``````compile_fail`n/// ```````n"
    $root = New-Fixture @{ 'a\target\doc\x.rs' = $bad; 'a\.scratch\y.rs' = $bad; 'a\src\lib.rs' = "fn f() {}`n" }
    try {
        $result = Invoke-Guard $root
        Assert-Equal 0 $result.ExitCode $result.Output
    }
    finally { Remove-Fixture $root }
}

Test-Case '-ListFences emits one path, line and pinned record per recognised fence' {
    # check-compile-fail-doctests.ps1 parses this, so its shape is a contract.
    # Only fences are listed, not prose; a one-line cfg_attr lists twice.
    $root = New-Fixture @{
        'a\src\lib.rs' = "/// A ``compile_fail`` example.`n/// ``````compile_fail,E0308`n/// ```````n" +
        "#[cfg_attr(x, doc = ""``````compile_fail"", doc = ""``````compile_fail,E0599"")]`n"
    }
    try {
        $records = @(& $script:Script -Root $root -ListFences)
        Assert-Equal 0 $LASTEXITCODE 'exit code'
        Assert-Equal 3 $records.Count ($records -join ' | ')
        $file = (Resolve-Path -LiteralPath (Join-Path $root 'a\src\lib.rs')).Path
        Assert-Equal "$file`t2`tTrue" $records[0] 'the pinned fence'
        Assert-Equal "$file`t4`tFalse" $records[1] 'the first fence on the one-line cfg_attr'
        Assert-Equal "$file`t4`tTrue" $records[2] 'the second fence on the same line'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a root that does not exist is a configuration error, not a pass' {
    $result = Invoke-Guard (Join-Path ([System.IO.Path]::GetTempPath()) ('cfc-missing-' + [guid]::NewGuid().ToString('N')))
    Assert-Equal 2 $result.ExitCode 'exit code'
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)." -Level bad
    exit 1
}
Write-Report 'All cases passed.' -Level good
exit 0
