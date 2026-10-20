# Copyright (c) Michael Grier
#
# tools/snapshot-learn-toc.ps1 -- records where Microsoft Learn files each Win32 symbol, as of the
# day it runs, so that "which section does this belong to" is a lookup in a file and not a
# search of a website.
#
# The repository places a safe wrapper of a platform API by the section Learn documents it under
# (WIN32-LEARN-MAP.md names the rule). Learn's section names and its headers drift, so the rule
# only classifies: neither string appears in a crate name or a public path, and the dated
# snapshot is the one place that carries them. A rename is a new snapshot, never a release.
#
# What it reads: https://learn.microsoft.com/en-us/windows/win32/toc.json for the areas under
# Develop > Desktop App Technologies, then each area's own toc.json, and every node in them that
# links to a reference page of a function, structure, enumeration, callback, interface or class.
# What it writes: a text file of one block per node that lists symbols, headed by the path of
# section titles that reach it, one symbol per line.
#
# Usage:
#   pwsh tools/snapshot-learn-toc.ps1                       # whole of Desktop App Technologies
#   pwsh tools/snapshot-learn-toc.ps1 -Area 'sysinfo','sync' # only these area folders
#
# Exits 0 on success and 1 when a request fails or a toc has an unexpected shape.

[CmdletBinding()]
param(
    # Where the snapshot is written. Default: WIN32-LEARN-MAP-DATA.txt at the repository root.
    [string]$Out,
    # Restrict to these area folders (the first segment of an area's link, e.g. sysinfo).
    [string[]]$Area,
    # Pause between requests, in milliseconds, so the crawl is polite.
    [int]$DelayMs = 150
)

$ErrorActionPreference = 'Stop'
$Root = Split-Path -Parent $PSScriptRoot
if (-not $Out) { $Out = Join-Path $Root 'WIN32-LEARN-MAP-DATA.txt' }
$Base = 'https://learn.microsoft.com/en-us/windows/win32'

# 7-bit ASCII only, as the repository's planning documents are: common punctuation is spelled
# out and anything else is written as \uXXXX, so the file survives every code page.
function ConvertTo-Ascii([string]$text) {
    $text = $text -replace [char]0x2019, "'" -replace [char]0x2018, "'" -replace [char]0x201C, '"' -replace [char]0x201D, '"'
    $text = $text -replace [char]0x2013, '-' -replace [char]0x2014, '--' -replace [char]0x00A0, ' '
    $builder = New-Object System.Text.StringBuilder
    foreach ($ch in $text.ToCharArray()) {
        if ([int]$ch -lt 128) { [void]$builder.Append($ch) } else { [void]$builder.AppendFormat('\u{0:X4}', [int]$ch) }
    }
    $builder.ToString()
}

function Get-Toc([string]$url) {
    Start-Sleep -Milliseconds $DelayMs
    $response = Invoke-WebRequest -UseBasicParsing -Uri $url -TimeoutSec 90
    $response.Content | ConvertFrom-Json
}

# Reference pages are /api/<header>/<kind>-<header>-<name>: nf function, ns structure, ne enumeration,
# nc callback, nn interface, nl class, nm method group, nd delegate-like entries, ni a control code
# (IOCTL_ and FSCTL_).
$ApiLink = [regex]'/api/(?<header>[^/]+)/(?<kind>n[a-z])-(?<slug>[^/#?]+)$'
$KindNames = @{ nf = 'function'; ns = 'struct'; ne = 'enum'; nc = 'callback'; nn = 'interface'; nl = 'class'; nm = 'method'; nd = 'delegate'; ni = 'control-code' }

$script:Lines = New-Object System.Collections.Generic.List[string]
$script:Counts = @{}

function Add-Group($path, $symbols) {
    $script:Lines.Add('')
    $script:Lines.Add('# ' + (ConvertTo-Ascii ($path -join ' > ')))
    foreach ($symbol in $symbols) { $script:Lines.Add($symbol) }
}

function Walk($node, [string[]]$path) {
    $path = $path + $node.toc_title
    $symbols = New-Object System.Collections.Generic.List[string]
    foreach ($child in @($node.children)) {
        if (-not $child) { continue }
        $match = if ($child.href) { $ApiLink.Match($child.href) } else { $null }
        if ($match -and $match.Success) {
            $kind = $match.Groups['kind'].Value
            $header = $match.Groups['header'].Value.ToLowerInvariant() + '.h'
            $title = ConvertTo-Ascii $child.toc_title
            $slug = $match.Groups['slug'].Value
            # The slug is "<header>-<name>" in lowercase. Say so when the name Learn lists differs
            # from it, as for the A and W forms (GetProfileInt is getprofileinta).
            $name = $slug -replace ('^' + [regex]::Escape($match.Groups['header'].Value.ToLowerInvariant()) + '-'), ''
            $flat = ($child.toc_title -replace '[^A-Za-z0-9]', '').ToLowerInvariant()
            $note = if (($name -replace '[^a-z0-9]', '') -ne $flat) { "`t=" + $name } else { '' }
            # Not named `$kindName`: PowerShell variable names are case-insensitive, so that would
            # be the lookup table itself.
            $label = if ($KindNames.ContainsKey($kind)) { $KindNames[$kind] } else { $kind }
            $symbols.Add("$label`t$header`t$title$note")
            $script:Counts[$label] = 1 + [int]$script:Counts[$label]
        } else {
            Walk $child $path
        }
    }
    if ($symbols.Count -gt 0) {
        Add-Group $path ($symbols | Sort-Object)
    }
}

$top = Get-Toc "$Base/toc.json"
$develop = $top.items[0].children | Where-Object toc_title -eq 'Develop'
$technologies = $develop.children | Where-Object toc_title -eq 'Desktop App Technologies'
if (-not $technologies) { Write-Error 'Desktop App Technologies is not where it was in the top toc.'; exit 1 }

# An area is a node whose link is relative and names a folder with its own toc.json.
function Get-Areas($node, [string[]]$path) {
    foreach ($child in @($node.children)) {
        if (-not $child) { continue }
        $here = $path + $child.toc_title
        # A link with no slash is a page at the root of the reference and has no toc of its own.
        if ($child.href -and $child.href -notmatch '^(/|https?:)' -and $child.href -match '/') {
            [pscustomobject]@{ Path = $here; Folder = ($child.href -split '/')[0]; Href = $child.href }
        } elseif ($child.children) {
            Get-Areas $child $here
        }
    }
}

$areas = @(Get-Areas $technologies @('Desktop App Technologies')) | Where-Object { $_.Folder -and $_.Folder -ne 'desktop-app-technologies' }
if ($Area) { $areas = @($areas | Where-Object { $Area -contains $_.Folder }) }
$seen = @{}
$skipped = @()
$failures = 0
foreach ($entry in $areas) {
    if ($seen.ContainsKey($entry.Folder)) { continue }
    $seen[$entry.Folder] = $true
    try {
        $toc = Get-Toc "$Base/$($entry.Folder)/toc.json"
        foreach ($item in $toc.items) { Walk $item $entry.Path[0..($entry.Path.Count - 2)] }
        Write-Host ("{0,-28} {1}" -f $entry.Folder, ($entry.Path -join ' > '))
    } catch {
        if ($_.Exception.Message -match '404') {
            # A portal whose folder has no toc of its own: its symbols are not in this record.
            $skipped += $entry.Folder
        } else {
            $failures++
            Write-Warning "$($entry.Folder): $($_.Exception.Message)"
        }
    }
}

$stamp = Get-Date -Format 'yyyy-MM-dd'
$summary = ($script:Counts.GetEnumerator() | Sort-Object Name | ForEach-Object { "$($_.Name)=$($_.Value)" }) -join ' '
$header = @(
    'WIN32-LEARN-MAP-DATA: where Microsoft Learn files each Win32 symbol.',
    "Snapshot: $stamp. Source: $Base/toc.json and each area's toc.json. Regenerate with tools/snapshot-learn-toc.ps1.",
    'Layout: "# " lines are the path of section titles; under each, "kind<TAB>header<TAB>name[<TAB>=slug]" per symbol.',
    '"=slug" appears only where the name in Learn differs from the page slug (the A and W forms).',
    "Areas: $($seen.Count - $skipped.Count). Symbols: $summary.",
    "Not covered, no toc of their own: $(($skipped | Sort-Object) -join ', ')."
)
$text = (($header + $script:Lines) -join "`n") + "`n"
[System.IO.File]::WriteAllText($Out, $text, (New-Object System.Text.UTF8Encoding($false)))
Write-Host "Wrote $Out ($([math]::Round($text.Length / 1KB)) KiB, $($seen.Count - $skipped.Count) areas, $($skipped.Count) without a toc, $failures failures)."
if ($failures -gt 0) { exit 1 }
