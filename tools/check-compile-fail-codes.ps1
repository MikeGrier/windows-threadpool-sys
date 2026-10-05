# Copyright (c) Mike Grier
#
# tools/check-compile-fail-codes.ps1 -- every `compile_fail` doctest names the
# error it exists to produce.
#
# WHY. A `compile_fail` example passes on ANY compile error. Found during the
# PR #113 review: windows-ioring-sys guarded "a borrow held across a push is
# refused" with an example that called a method M28 had since removed, so it
# kept passing -- for "no such method" -- while testing nothing.
#
# Pinning the code (```compile_fail,E0502) is the fix, with a limit: rustdoc
# enforces a pinned code only on a nightly toolchain or under RUSTC_BOOTSTRAP=1.
# CI's `doctest-error-codes` job runs the doctests that way. This script is the
# other half: it refuses an unpinned fence, so there is nothing for that job to
# fail to enforce.
#
# What counts as a fence: a line whose code-fence info string lists
# `compile_fail`, in a `///` or `//!` doc comment or their `/** */` and
# `/*! */` block forms, in a `doc = "..."` attribute
# (the `cfg_attr` form windows-threadpool-sys uses), or in a markdown file --
# markdown is scanned because a crate may `include_str!` it as doctests. The
# fence may be backticks or tildes, and may sit inside any nesting of
# blockquotes and list items, because rustdoc follows CommonMark and runs all of
# those. It is pinned when the same info string also carries an `E` followed by
# four digits.
#
# Not parsed: a fence opened mid-string after an escaped `\n` inside one
# `doc = "..."` literal. Nothing in the workspace writes one, and the fixture
# suite says what is and is not recognised.
#
#   ./tools/check-compile-fail-codes.ps1            # scan crates/ (CI)
#   ./tools/check-compile-fail-codes.ps1 -Root DIR  # scan a fixture tree

[CmdletBinding()]
param(
    [string]$Root
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Root) { $Root = Join-Path (Split-Path -Parent $PSScriptRoot) 'crates' }
if (-not (Test-Path -LiteralPath $Root)) {
    Write-Host "CONFIG ERROR: root not found: $Root" -ForegroundColor Red
    exit 2
}
# Absolute, because file paths are reported relative to it by trimming its length.
$Root = (Resolve-Path -LiteralPath $Root).Path.TrimEnd('\', '/')

# A fence opens a line (optionally behind `///`, `//!`, or the `/**` / `/*!`
# that opens a block doc comment), or follows
# `doc = "` anywhere on a line -- a `cfg_attr` may put several `doc` strings on
# one line, and a pattern anchored to the line start missed exactly that until
# this script's own test found it.
#
# Then any run of CommonMark container markers -- `>` for a blockquote, `-`,
# `+`, `*` or `1.` / `1)` for a list item -- because a fence nested in those is
# still a doctest. Missing them let `/// > ```compile_fail` through (PR #113
# review), and the fix found tilde fences missing too. The ` * ` that starts a
# line inside a block doc comment is recognised by the same rule, as a list
# marker, so the inner lines of `/** */` needed nothing new -- only the opening
# line did, which the next review found. The cost is that a fence line inside
# a plain `/* */` comment is reported too: a false alarm, never a miss.
#
# Then the fence and its info string, up to the end of the line or the closing
# quote of an attribute. A backtick fence's info string cannot contain a
# backtick; a tilde fence's can.
$container = '(?:\s*(?:>|(?:[-+*]|\d{1,9}[.)])(?=\s)))*\s*'
$fence = [regex]('(?:^\s*(?://[/!]|/\*[*!])?|\bdoc\s*=\s*")' + $container +
    '(?:`{3,}(?<info>[^`"\r\n]*)|~{3,}(?<info>[^"\r\n]*))')
$skip = @('target', '.scratch', '.git', 'node_modules')

$unpinned = New-Object System.Collections.Generic.List[string]
$pinned = 0

$files = Get-ChildItem -LiteralPath $Root -Recurse -File -Include '*.rs', '*.md' |
    Where-Object {
        $parts = $_.FullName.Substring($Root.Length).Split([IO.Path]::DirectorySeparatorChar)
        -not ($parts | Where-Object { $skip -contains $_ })
    } | Sort-Object FullName

foreach ($file in $files) {
    $lineNumber = 0
    foreach ($line in [System.IO.File]::ReadAllLines($file.FullName)) {
        $lineNumber++
        foreach ($match in $fence.Matches($line)) {
            $tokens = @($match.Groups['info'].Value.Split(',') | ForEach-Object { $_.Trim() })
            if ($tokens -notcontains 'compile_fail') { continue }
            if (@($tokens | Where-Object { $_ -match '^E\d{4}$' }).Count -gt 0) {
                $pinned++
                continue
            }
            $relative = $file.FullName.Substring($Root.Length).TrimStart('\', '/')
            $unpinned.Add("${relative}:$lineNumber") | Out-Null
        }
    }
}

if ($unpinned.Count -eq 0) {
    Write-Host "Every compile_fail doctest pins its error code ($pinned found)." -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host 'These compile_fail doctests do not name the error they exist to produce:' -ForegroundColor Red
foreach ($entry in $unpinned) { Write-Host "  $entry" -ForegroundColor Yellow }
Write-Host ''
Write-Host 'Unpinned, the example passes on ANY compile error -- a typo, or a method' -ForegroundColor Cyan
Write-Host 'that has since been renamed -- and reports green while testing nothing.' -ForegroundColor Cyan
Write-Host 'Add the expected code to the fence, e.g. ```compile_fail,E0502. To find it,' -ForegroundColor Cyan
Write-Host 'pin a placeholder such as E9999 and run the doctests with RUSTC_BOOTSTRAP=1:' -ForegroundColor Cyan
Write-Host 'rustdoc then reports the code the compiler actually raised.' -ForegroundColor Cyan
Write-Host ''
exit 1
