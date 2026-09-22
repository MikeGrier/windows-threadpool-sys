# Copyright (c) Mike Grier
#
# tools/check-borrow-surface.ps1 -- keeps windows-ioring-sys's borrow-returning
# public API from growing unnoticed.
#
# Population C -- what safe code is *permitted* to do -- is the defect class no
# runtime technique reaches, because nothing has to execute for the hole to
# exist. windows-ioring-sys has shipped four of them, all the same shape: a
# public signature that allowed an operation nobody intended.
#
#   D-35  `get_mut` returned `&mut Vec<u8>`, which permits `reserve`, `resize`
#         and reassignment, where only byte writes were intended.
#   D-36  `get` returned an unchecked `&[u8]` while the kernel might still be
#         writing into that buffer.
#   D-43  `EventDelivery::ring` returned `&Mutex<IoRing>`, and any `&mut IoRing`
#         permits whole-value assignment -- so safe code could replace the ring
#         and silently stop delivery.
#   D-45  `get` returned a slice living as long as the borrow, while the check
#         that guarded it held only at the instant of the call.
#
# All four arrived through ordinary, well-reviewed changes. What was missing
# was not diligence, it was a specific question being asked at a specific
# moment. M18.1 asked it once, over the whole surface; this script is what makes
# it recur, because a rule that lives only in a document is a rule that depends
# on somebody remembering to apply it -- which is exactly how the four above
# got in.
#
# The mechanism is a committed inventory. Every public signature in
# `crates/windows-ioring-sys/src` that carries a borrow is listed in
# BORROW-SURFACE.txt. This script regenerates that list from the source and
# fails if it differs. Adding or widening such a signature therefore cannot land
# quietly: CI stops, and the author has to answer the question in
# DESIGN-INSTRUCTIONS.md and record the answer before the inventory can be
# updated.
#
# THREE SHAPES ARE INSPECTED, and the second and third were added by M21+.1
# after a review found the check silent on a change that used both:
#
#   1. An inherent `pub fn` whose RETURN type carries a borrow. The original
#      rule, and what D-35/D-36/D-43/D-45 all were.
#   2. Any method of a `pub trait`, on either side of the signature. Trait
#      items are declared `fn`, not `pub fn`, so the rule above never matched
#      one -- a public trait method returning `&[u8]` was invisible.
#   3. A PARAMETER carrying an explicit lifetime, such as `&mut RingWait<'_>`.
#      This is the direction the original rule could not see, and it is the
#      wider exposure of the two: a return value goes to a known caller, while
#      a borrow-carrying parameter of a public trait method is handed to
#      arbitrary safe code the crate has never seen.
#
# A plain `&T` parameter is deliberately NOT reported. Lending a reference to a
# callee is the caller's business and not this defect class; what matters is a
# borrow-carrying wrapper whose lifetime the crate chose. The explicit-lifetime
# test is what separates them.
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

# Collect one entry per public signature that carries a borrow.
#
# Signatures wrap across lines, so accumulate from the `fn` line until the line
# that closes the signature -- the one ending in `{` (a body) or `;` (a trait
# item without one).
function Get-BorrowSurface {
    param([string]$Root)

    $entries = @()
    $files = Get-ChildItem -Path $Root -Recurse -Filter '*.rs' | Sort-Object FullName

    foreach ($file in $files) {
        $relative = $file.FullName.Substring($repoRoot.Length + 1) -replace '\\', '/'
        $lines = [System.IO.File]::ReadAllLines($file.FullName)
        $index = 0

        # Depth tracking for `pub trait` blocks. Trait items inherit the
        # trait's visibility and are declared `fn`, not `pub fn`, so the only
        # way to recognise one is to know we are inside such a block.
        $traitDepth = -1
        $depth = 0

        while ($index -lt $lines.Length) {
            $line = $lines[$index]

            # A `pub trait` opens a block whose items are public. `unsafe` and
            # `auto` may sit between; a supertrait list may follow.
            if ($traitDepth -lt 0 -and $line -match '^\s*pub(\s+(unsafe|auto))*\s+trait\s') {
                $traitDepth = $depth
            }

            $isTraitItem = ($traitDepth -ge 0) -and ($line -match '^\s*(unsafe\s+)?fn\s')
            $isInherent = $line -match '^\s*pub(\s+(unsafe|const|async))*\s+fn\s'

            if (-not $isTraitItem -and -not $isInherent) {
                # Track braces only on lines that are not signature starts; a
                # signature's own braces are consumed by the accumulator below.
                $depth += ([regex]::Matches($line, '\{')).Count
                $depth -= ([regex]::Matches($line, '\}')).Count
                if ($traitDepth -ge 0 -and $depth -le $traitDepth) {
                    $traitDepth = -1
                }
                $index++
                continue
            }

            # Accumulate the whole signature.
            #
            # A signature ends at the first `{` -- wherever it appears, including
            # a one-line body such as `pub fn f() -> &[u8] { &[] }` -- or at a
            # `;` for a trait item with no body. Testing the accumulated text
            # rather than "the line ends with `{`" is what handles the one-line
            # form; the earlier test ran off the end of the file on it.
            $signature = ''
            $cursor = $index
            while ($cursor -lt $lines.Length) {
                $signature += ' ' + $lines[$cursor].Trim()
                if ($signature -match '\{' -or $signature -match ';\s*$') {
                    break
                }
                $cursor++
            }
            if ($cursor -ge $lines.Length) {
                $cursor = $lines.Length - 1
            }
            foreach ($consumed in $index..$cursor) {
                $depth += ([regex]::Matches($lines[$consumed], '\{')).Count
                $depth -= ([regex]::Matches($lines[$consumed], '\}')).Count
            }
            if ($traitDepth -ge 0 -and $depth -le $traitDepth) {
                $traitDepth = -1
            }
            $index = $cursor + 1

            $signature = ($signature -replace '\s+', ' ').Trim()

            if ($signature -notmatch '\bfn\s+([A-Za-z_][A-Za-z0-9_]*)') {
                continue
            }
            $name = $Matches[1]

            # The return type is what follows the last `->` before the body.
            # `-1` distinguishes "no arrow" from "arrow at position 0".
            $arrow = $signature.LastIndexOf('->')
            $returns = ''
            if ($arrow -ge 0) {
                $returns = $signature.Substring($arrow + 2)
                # Strip a body, including the one-line form `{ &[] }` whose
                # braces sit on the same line as the return type.
                $brace = $returns.IndexOf('{')
                if ($brace -ge 0) {
                    $returns = $returns.Substring(0, $brace)
                }
                $returns = ($returns -replace '\s*;\s*$', '')
                $returns = ($returns -replace '\s*where\b.*$', '').Trim()
            }

            # A borrow is a reference, or a lifetime parameter carried by a
            # wrapper such as `Batch<'_>` or `RingScope<'_>` -- which is exactly
            # where D-43's fix lives, so a `&`-only scan would miss it.
            $returnBorrows = $returns -and ($returns -match '&' -or $returns -match "'")

            $parameters = Get-ParameterList -Signature $signature
            # Only an EXPLICIT lifetime counts in parameter position. A plain
            # `&T` is the caller lending to us, which is not this defect class;
            # a wrapper whose lifetime the crate chose, such as
            # `&mut RingWait<'_>`, is.
            $parameterBorrows = $parameters -and ($parameters -match "'")

            if (-not $returnBorrows -and -not $parameterBorrows) {
                continue
            }

            # Returns and parameters stay distinguishable in the inventory, and
            # a return-only entry keeps the format it had before M21+.1 so
            # widening the check did not churn the rows it already covered.
            if ($parameterBorrows) {
                $entries += "{0} :: {1}({2}) -> {3}" -f $relative, $name, $parameters, ($returns ? $returns : '()')
            }
            else {
                $entries += "{0} :: {1} -> {2}" -f $relative, $name, $returns
            }
        }
    }

    return , ($entries | Sort-Object)
}

# The parameter list of `$Signature`, with the receiver removed.
#
# `&self` and `&mut self` are on every method and say nothing about this defect
# class, so reporting them would bury the entries that matter.
function Get-ParameterList {
    param([string]$Signature)

    $open = $Signature.IndexOf('(')
    if ($open -lt 0) {
        return ''
    }

    # Balance parentheses: a parameter type may contain its own, as in
    # `impl FnOnce(*mut c_void, usize) -> HRESULT`.
    $depth = 0
    $close = -1
    for ($i = $open; $i -lt $Signature.Length; $i++) {
        if ($Signature[$i] -eq '(') { $depth++ }
        elseif ($Signature[$i] -eq ')') {
            $depth--
            if ($depth -eq 0) { $close = $i; break }
        }
    }
    if ($close -lt 0) {
        return ''
    }

    $inner = $Signature.Substring($open + 1, $close - $open - 1).Trim()
    # Drop the receiver, however it is spelled.
    $inner = $inner -replace "^&\s*('[a-z_][a-z0-9_]*\s*)?(mut\s+)?self\s*,?\s*", ''
    $inner = $inner -replace '^mut\s+self\s*,?\s*', ''
    $inner = $inner -replace '^self\s*,?\s*', ''
    return $inner.Trim()
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
Write-Host 'asked, because four shipped defects (D-35, D-36, D-43, D-45) all entered' -ForegroundColor Cyan
Write-Host 'as ordinary reviewed changes to this surface:' -ForegroundColor Cyan
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
