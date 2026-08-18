# Copyright (c) Michael Grier
#
# tools/check-encoding.ps1 -- text hygiene for tracked files. Fails if a file
# is not valid UTF-8, contains a stray C0 control character, glues a doc-comment
# marker onto the end of a line of code, or contains characteristic mojibake
# digraphs.
#
# The last two are not encoding faults. They are here because this is the check
# CI already runs over every tracked file, and because each of them is damage a
# text-rewriting tool produced and every other check passed.
#
# encoding-check: allow-mojibake  (this file contains literal examples
# of mojibake patterns in regexes and comments)
#
# Usage:
#   pwsh tools/check-encoding.ps1                 # check every tracked file
#   pwsh tools/check-encoding.ps1 -Path src/foo   # check one file or dir
#
# Exits 0 on success, 1 when encoding issues are found, and 2 for
# usage/configuration errors (unknown path, or not in a git repo without
# -Path).  Designed for both local use after a fallback edit and for CI
# invocation on every pull request.

[CmdletBinding()]
param(
    # Optional path to restrict the check to.  Default: all files tracked
    # by git.
    [string]$Path
)

$ErrorActionPreference = 'Stop'

# Common UTF-8-misread-as-Windows-1252 digraphs / trigraphs.  These are
# not exhaustive but catch the vast majority of real-world corruption:
#   Ã  prefix     -- most Latin-1 letters misread (Ã©, Ãª, Ã¨, Ã , Ã¯, ...)
#   â€  prefix    -- typographic punctuation (em-dash, en-dash, smart quotes, ellipsis, bullet)
#   â"  prefix    -- box drawing
#   Â<NBSP>       -- stray NBSP in front of an ASCII char
$Patterns = @(
    [pscustomobject]@{ Name = 'Latin-1 mojibake (Ã...)';     Regex = '[\u00C3][\u0080-\u00BF]' }
    [pscustomobject]@{ Name = 'Punctuation mojibake (â€...)'; Regex = '\u00E2\u20AC[\u0080-\u20FF]' }
    [pscustomobject]@{ Name = 'Box-draw mojibake (â"...)';   Regex = '\u00E2\u201D[\u0080-\u20FF]' }
    [pscustomobject]@{ Name = 'NBSP mojibake (Â<sp>)';     Regex = '\u00C2\u00A0' }
)

# File extensions we consider "text" and therefore subject to the check.
# Binary files (images, archives, .binlog fixtures, etc.) are skipped.
$TextExtensions = @(
    '.rs', '.toml', '.md', '.txt', '.json', '.yaml', '.yml',
    '.ps1', '.psm1', '.psd1', '.sh', '.cfg', '.ini', '.ts',
    '.lock', '.gitignore', '.gitattributes', '.vscodeignore'
)

function Test-IsTextFile([string]$file) {
    $ext = [System.IO.Path]::GetExtension($file).ToLowerInvariant()
    if ($ext -and $TextExtensions -contains $ext) { return $true }
    # Files with no extension that look like text (LICENSE, README, etc.).
    $name = [System.IO.Path]::GetFileName($file)
    if (-not $ext -and $name -match '^(LICENSE|README|CHANGELOG|AUTHORS|NOTICE|MAINTAINERS|CODEOWNERS)') {
        return $true
    }
    return $false
}

function Get-TargetFiles {
    if ($Path) {
        if (Test-Path -LiteralPath $Path -PathType Leaf) {
            return @((Resolve-Path -LiteralPath $Path).Path)
        }
        if (Test-Path -LiteralPath $Path -PathType Container) {
            return Get-ChildItem -LiteralPath $Path -Recurse -File |
                Where-Object { Test-IsTextFile $_.FullName } |
                ForEach-Object { $_.FullName }
        }
        Write-Error "Path not found: $Path"
        exit 2
    }
    # Default: all files tracked by git.
    $repoRoot = (& git rev-parse --show-toplevel 2>$null)
    if (-not $repoRoot) {
        Write-Error 'Not inside a git repository; pass -Path explicitly.'
        exit 2
    }
    Push-Location $repoRoot
    try {
        $tracked = & git ls-files
        return $tracked |
            Where-Object { Test-IsTextFile $_ } |
            ForEach-Object { Join-Path $repoRoot $_ }
    } finally {
        Pop-Location
    }
}

$files = @(Get-TargetFiles)
$failures = @()
$strictUtf8 = [System.Text.UTF8Encoding]::new($false, $true)

# Files that legitimately contain mojibake digraphs as documentation /
# regex content opt out of the pattern check by including this marker.
# They are still validated as UTF-8.
$AllowMojibakeMarker = 'encoding-check: allow-mojibake'

foreach ($file in $files) {
    if (-not (Test-Path -LiteralPath $file)) { continue }
    $bytes = [System.IO.File]::ReadAllBytes($file)
    if ($bytes.Length -eq 0) { continue }

    # 1. Must be valid UTF-8.
    try {
        $text = $strictUtf8.GetString($bytes)
    } catch {
        $failures += "INVALID UTF-8: $file ($($_.Exception.Message))"
        continue
    }

    # 2. Must not contain stray C0 control characters or DEL.
    #
    #    Tab, line feed and carriage return are the only ones a text file has
    #    any business containing. The rest are invisible in every editor and
    #    diff, so they survive review unless something looks for them.
    #
    #    This exists because a form feed (0x0C) was committed into two source
    #    comments: a PowerShell replacement contained a backtick followed by
    #    `f`, which PowerShell reads as its form-feed escape. Such a character
    #    is valid UTF-8 and is not mojibake, so both checks above passed it.
    $control = [regex]::Match($text, '[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]')
    if ($control.Success) {
        $prefix = $text.Substring(0, $control.Index)
        $line = ($prefix -split "`n").Count
        $code = '0x{0:X2}' -f [int][char]$control.Value
        $failures += "CONTROL CHARACTER: $file [$code at line $line]"
        continue
    }

    # 3. Must not glue a doc-comment marker onto the end of a line of code.
    #
    #    `let x = 1;///` compiles -- it is only an `unused_doc_comment`
    #    warning, and doctest warnings do not fail a build -- so this survives
    #    every check the toolchain applies. It is never intentional: it is what
    #    a mis-joined edit looks like, and one lived in a doc example for nine
    #    review rounds before being spotted by eye.
    if ([System.IO.Path]::GetExtension($file) -eq '.rs') {
        $glued = [regex]::Match($text, '(?m)^.*\S///\s*$')
        if ($glued.Success) {
            $prefix = $text.Substring(0, $glued.Index)
            $line = ($prefix -split "`n").Count
            $failures += "GLUED DOC COMMENT: $file [line $line]"
            continue
        }
    }

    # 4. Must not contain characteristic mojibake patterns -- unless the
    #    file explicitly opts out.
    if ($text.Contains($AllowMojibakeMarker)) { continue }
    foreach ($p in $Patterns) {
        if ([regex]::IsMatch($text, $p.Regex)) {
            $failures += "MOJIBAKE: $file [$($p.Name)]"
            break
        }
    }
}

if ($failures.Count -gt 0) {
    foreach ($f in $failures) { Write-Host $f -ForegroundColor Red }
    Write-Host ''
    Write-Host "Encoding check failed: $($failures.Count) file(s) flagged out of $($files.Count) checked." -ForegroundColor Red
    exit 1
}

Write-Host "Encoding check passed: $($files.Count) file(s) clean." -ForegroundColor Green
exit 0
