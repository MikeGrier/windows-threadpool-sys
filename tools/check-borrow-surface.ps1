# Copyright (c) Mike Grier
#
# tools/check-borrow-surface.ps1 -- keeps windows-ioring-sys's borrow-returning
# public API from growing unnoticed.
#
# Population C -- what safe code is *permitted* to do -- is the defect class no
# runtime technique reaches, because nothing has to execute for the hole to
# exist. windows-ioring-sys has shipped three of them, all the same shape: a
# public method whose return type allowed an operation nobody intended.
#
#   D-35  `get_mut` returned `&mut Vec<u8>`, which permits `reserve`, `resize`
#         and reassignment, where only byte writes were intended.
#   D-36  `get` returned an unchecked `&[u8]` while the kernel might still be
#         writing into that buffer.
#   D-43  `EventDelivery::ring` returned `&Mutex<IoRing>`, and any `&mut IoRing`
#         permits whole-value assignment -- so safe code could replace the ring
#         and silently stop delivery.
#
# All three arrived through ordinary, well-reviewed changes. What was missing
# was not diligence, it was a specific question being asked at a specific
# moment. M18.1 asked it once, over the whole surface; this script is what makes
# it recur, because a rule that lives only in a document is a rule that depends
# on somebody remembering to apply it -- which is exactly how the three above
# got in.
#
# The mechanism is a committed inventory. Every public function in
# `crates/windows-ioring-sys/src` whose return type carries a borrow -- a
# reference, or a lifetime parameter such as `Batch<'_>` -- is listed in
# BORROW-SURFACE.txt. This script regenerates that list from the source and
# fails if it differs. Adding or widening such a method therefore cannot land
# quietly: CI stops, and the author has to answer the question in
# DESIGN-INSTRUCTIONS.md and record the answer before the inventory can be
# updated.
#
# This deliberately checks *shape*, not correctness. It cannot tell a safe
# accessor from a dangerous one -- only that the surface changed and a human
# owes an answer. That is the whole job: the question, asked reliably.
#
#   ./tools/check-borrow-surface.ps1            # verify (CI)
#   ./tools/check-borrow-surface.ps1 -Update    # regenerate after answering

[CmdletBinding()]
param(
    [switch]$Update
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$sourceRoot = Join-Path $repoRoot 'crates\windows-ioring-sys\src'
$inventoryPath = Join-Path $repoRoot 'crates\windows-ioring-sys\BORROW-SURFACE.txt'
$instructions = 'crates/windows-ioring-sys/DESIGN-INSTRUCTIONS.md'

if (-not (Test-Path $sourceRoot)) {
    Write-Host "CONFIG ERROR: source root not found: $sourceRoot" -ForegroundColor Red
    exit 2
}

# Collect one entry per public function whose return type carries a borrow.
#
# Signatures wrap across lines, so accumulate from `pub fn` until the line that
# closes the signature -- the one ending in `{` (a body) or `;` (a trait item).
function Get-BorrowSurface {
    param([string]$Root)

    $entries = @()
    $files = Get-ChildItem -Path $Root -Recurse -Filter '*.rs' | Sort-Object FullName

    foreach ($file in $files) {
        $relative = $file.FullName.Substring($repoRoot.Length + 1) -replace '\\', '/'
        $lines = [System.IO.File]::ReadAllLines($file.FullName)
        $index = 0

        while ($index -lt $lines.Length) {
            $line = $lines[$index]

            if ($line -notmatch '^\s*pub(\s+(unsafe|const|async))*\s+fn\s') {
                $index++
                continue
            }

            # Accumulate the whole signature.
            $signature = ''
            $cursor = $index
            while ($cursor -lt $lines.Length) {
                $signature += ' ' + $lines[$cursor].Trim()
                if ($lines[$cursor] -match '\{\s*$' -or $lines[$cursor] -match ';\s*$') {
                    break
                }
                $cursor++
            }
            $index = $cursor + 1

            $signature = ($signature -replace '\s+', ' ').Trim()

            if ($signature -notmatch '\bfn\s+([A-Za-z_][A-Za-z0-9_]*)') {
                continue
            }
            $name = $Matches[1]

            # The return type is what follows the last `->` before the body.
            $arrow = $signature.LastIndexOf('->')
            if ($arrow -lt 0) {
                continue
            }
            $returns = $signature.Substring($arrow + 2)
            $returns = ($returns -replace '\s*\{\s*$', '') -replace '\s*;\s*$', ''
            $returns = ($returns -replace '\s*where\b.*$', '').Trim()

            # A borrow is a reference, or a lifetime parameter carried by a
            # wrapper such as `Batch<'_>` or `RingScope<'_>` -- which is exactly
            # where D-43's fix lives, so a `&`-only scan would miss it.
            if ($returns -notmatch '&' -and $returns -notmatch "'") {
                continue
            }

            $entries += "{0} :: {1} -> {2}" -f $relative, $name, $returns
        }
    }

    return , ($entries | Sort-Object)
}

$current = Get-BorrowSurface -Root $sourceRoot

if ($Update) {
    $header = @(
        '# windows-ioring-sys: public functions returning a borrow.',
        '#',
        '# GENERATED by tools/check-borrow-surface.ps1 -Update. Do not hand-edit.',
        '#',
        "# Adding or widening an entry here obliges the answer described in",
        "# $instructions. The list exists so that obligation cannot be",
        '# forgotten: CI regenerates it and fails when it disagrees with the source.'
    )
    $body = $header + $current
    [System.IO.File]::WriteAllText($inventoryPath, ($body -join "`n") + "`n")
    Write-Host "Updated $inventoryPath ($($current.Count) entries)." -ForegroundColor Green
    exit 0
}

if (-not (Test-Path $inventoryPath)) {
    Write-Host "CONFIG ERROR: inventory not found: $inventoryPath" -ForegroundColor Red
    Write-Host "Create it with: ./tools/check-borrow-surface.ps1 -Update" -ForegroundColor Yellow
    exit 2
}

$recorded = @([System.IO.File]::ReadAllLines($inventoryPath) |
    Where-Object { $_ -and -not $_.StartsWith('#') })

# `@(...)` on both: under StrictMode a pipeline yielding nothing is `$null` and
# one yielding a single string is a bare string, neither of which has `.Count`.
$added = @($current | Where-Object { $recorded -notcontains $_ })
$removed = @($recorded | Where-Object { $current -notcontains $_ })

if ($added.Count -eq 0 -and $removed.Count -eq 0) {
    Write-Host "Borrow surface unchanged ($($current.Count) entries)." -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host 'windows-ioring-sys: the borrow-returning public surface changed.' -ForegroundColor Red
Write-Host ''

foreach ($entry in $added) {
    Write-Host "  ADDED    $entry" -ForegroundColor Yellow
}
foreach ($entry in $removed) {
    Write-Host "  REMOVED  $entry" -ForegroundColor Yellow
}

Write-Host ''
Write-Host 'This is not an error in itself. It is the moment the question has to be' -ForegroundColor Cyan
Write-Host 'asked, because three shipped defects (D-35, D-36, D-43) all entered as' -ForegroundColor Cyan
Write-Host 'ordinary reviewed changes to this surface:' -ForegroundColor Cyan
Write-Host ''
Write-Host '    What can safe code do with this, and does the registration or the' -ForegroundColor White
Write-Host '    kernel still hold anything it could invalidate?' -ForegroundColor White
Write-Host ''
Write-Host "Answer it as $instructions requires, add the row to the" -ForegroundColor Cyan
Write-Host 'borrow-surface audit in DESIGN-NOTES.md, then run:' -ForegroundColor Cyan
Write-Host ''
Write-Host '    ./tools/check-borrow-surface.ps1 -Update' -ForegroundColor White
Write-Host ''
exit 1
