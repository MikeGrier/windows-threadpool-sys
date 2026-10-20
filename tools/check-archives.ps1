# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Lint every COMPLETED-CHECKLIST.md: heading style, open boxes, and immutable
    headings and anchors.

.DESCRIPTION
    The archives are append-only history, read by people and linked to by
    anchor, and the defects that reached review in them were each decidable by
    a script (root CHECKLIST.md, M34.18). Four rules, settled by the engineer on
    2026-10-19:

    1. A heading is preceded by a blank line. CommonMark renders it either way,
       so this is style -- applied to every heading level, since the reason is
       the same at each.
    2. A heading is never glued onto the end of another line, which is the shape
       that does NOT render: an archived heading carries an `<a id>` anchor, or
       is a `## Moved` group heading, so those two forms are what is looked for
       mid-line.
    3. No `- [ ]` remains. An archived item is done; a verbatim copy of an item
       "as it stood" is ticked rather than exempted.
    4. Headings and anchors are immutable. Against the merge base with -Base,
       every heading line and every `<a id>` anchor an archive had must still be
       there. Other lines may change: this repository's own rules require
       repointing a link when its target moves, and archives carry factual
       corrections. A deleted archive breaks the rule too.

    Lines inside fenced code blocks are not headings or boxes, and are skipped.

.PARAMETER Root
    The repository to check. Defaults to the one this script is in.

.PARAMETER Base
    A git ref to compare against for rule 4; the check uses its merge base with
    HEAD. Defaults to `origin/main`. Pass `-NoHistory` to skip rule 4 instead,
    for a tree with no such ref.

.PARAMETER NoHistory
    Skip rule 4.

.NOTES
    Exits 0 when every archive passes, 1 when any rule is broken, and 2 for a
    usage or configuration error (no repository, an unresolvable -Base).
#>
[CmdletBinding()]
param(
    [string] $Root,
    [string] $Base = 'origin/main',
    [switch] $NoHistory
)

$ErrorActionPreference = 'Stop'

if (-not $Root) {
    $Root = (& git -C $PSScriptRoot rev-parse --show-toplevel 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $Root) {
        Write-Host "check-archives: not inside a git repository; pass -Root." -ForegroundColor Red
        exit 2
    }
}
$Root = (Resolve-Path -LiteralPath $Root).Path

$archivePattern = '(^|/)COMPLETED-CHECKLIST\.md$'
$headingPattern = '^#{1,6} '
$anchorPattern = '<a id="([^"]+)"></a>'
# Rule 2's two shapes, each preceded by something other than the start of a line.
$gluedPattern = '\S\s*(#{2,6} <a id="|## Moved )'
$openBoxPattern = '^\s*[-*] \[ \]'
$fencePattern = '^\s*(```|~~~)'

<#
.SYNOPSIS
    The lines of a text, each with whether it sits inside a fenced code block.
#>
function Get-Lines {
    param([string] $Text)
    $inFence = $false
    $lines = $Text -split "\r?\n"
    for ($i = 0; $i -lt $lines.Count; $i++) {
        $isFence = $lines[$i] -match $fencePattern
        [pscustomobject]@{
            Number = $i + 1
            Text   = $lines[$i]
            Code   = $inFence -or $isFence
        }
        if ($isFence) { $inFence = -not $inFence }
    }
}

<#
.SYNOPSIS
    Every heading line and every anchor id in a text, outside fenced code.
#>
function Get-Landmarks {
    param([string] $Text)
    $headings = New-Object System.Collections.Generic.List[string]
    $anchors = New-Object System.Collections.Generic.List[string]
    foreach ($line in (Get-Lines $Text)) {
        if ($line.Code) { continue }
        if ($line.Text -match $headingPattern) { $headings.Add($line.Text.TrimEnd()) }
        foreach ($m in [regex]::Matches($line.Text, $anchorPattern)) { $anchors.Add($m.Groups[1].Value) }
    }
    [pscustomobject]@{ Headings = $headings; Anchors = $anchors }
}

# Untracked files too, so an archive created and not yet added is checked before
# its first commit rather than after it.
$archives = @(& git -C $Root ls-files --cached --others --exclude-standard | Where-Object { $_ -match $archivePattern } | Sort-Object -Unique)
if ($LASTEXITCODE -ne 0) {
    Write-Host "check-archives: git ls-files failed in $Root." -ForegroundColor Red
    exit 2
}

$problems = New-Object System.Collections.Generic.List[string]

# Rules 1 to 3: the files as they stand.
foreach ($archive in $archives) {
    # A tracked archive deleted from the working tree has nothing to lint here;
    # rule 4 is what reports it.
    $path = Join-Path $Root $archive
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { continue }
    $text = [System.IO.File]::ReadAllText($path)
    $previous = $null
    foreach ($line in (Get-Lines $text)) {
        if (-not $line.Code) {
            if ($line.Text -match $headingPattern -and $line.Number -gt 1 -and $previous.Trim() -ne '') {
                $problems.Add("${archive}:$($line.Number): HEADING WITHOUT A BLANK LINE ABOVE: $($line.Text)")
            }
            if ($line.Text -notmatch $headingPattern -and $line.Text -match $gluedPattern) {
                $problems.Add("${archive}:$($line.Number): HEADING GLUED ONTO A LINE: $($line.Text)")
            }
            if ($line.Text -match $openBoxPattern) {
                $problems.Add("${archive}:$($line.Number): OPEN BOX IN AN ARCHIVE: $($line.Text)")
            }
        }
        $previous = $line.Text
    }
}

# Rule 4: against the merge base.
if (-not $NoHistory) {
    $baseCommit = (& git -C $Root merge-base $Base HEAD 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $baseCommit) {
        Write-Host "check-archives: cannot find the merge base of '$Base' and HEAD in $Root." -ForegroundColor Red
        Write-Host "Fetch the base, name another with -Base, or pass -NoHistory to skip the immutability rule." -ForegroundColor Red
        exit 2
    }
    $baseArchives = @(& git -C $Root ls-tree -r --name-only $baseCommit | Where-Object { $_ -match $archivePattern })
    foreach ($archive in $baseArchives) {
        $current = Join-Path $Root $archive
        if (-not (Test-Path -LiteralPath $current -PathType Leaf)) {
            $problems.Add("${archive}: ARCHIVE DELETED since $($baseCommit.Substring(0, 12))")
            continue
        }
        $before = Get-Landmarks ((& git -C $Root show "${baseCommit}:$archive") -join "`n")
        $after = Get-Landmarks ([System.IO.File]::ReadAllText($current))

        $remaining = New-Object System.Collections.Generic.List[string]
        $after.Headings | ForEach-Object { $remaining.Add($_) }
        foreach ($heading in $before.Headings) {
            if (-not $remaining.Remove($heading)) {
                $problems.Add("${archive}: HEADING CHANGED OR DELETED since $($baseCommit.Substring(0, 12)): $heading")
            }
        }
        foreach ($anchor in $before.Anchors) {
            if (-not $after.Anchors.Contains($anchor)) {
                $problems.Add("${archive}: ANCHOR REMOVED since $($baseCommit.Substring(0, 12)): $anchor")
            }
        }
    }
}

if ($problems.Count -eq 0) {
    $history = if ($NoHistory) { 'history not checked' } else { "headings and anchors unchanged since $($baseCommit.Substring(0, 12))" }
    Write-Host "Archives clean: $($archives.Count) file(s), $history."
    exit 0
}

foreach ($problem in $problems) { Write-Host $problem -ForegroundColor Red }
Write-Host ""
Write-Host "$($problems.Count) problem(s) in the archives. See CHECKLIST.md M34.18 for the rules." -ForegroundColor Red
exit 1
