# Copyright (c) Mike Grier
#
# tools/check-ring-tests.ps1 -- keeps windows-ioring-sys's population of
# ring-opening *lib* tests from growing unnoticed.
#
# D-49 recorded the defect: 63 of 131 lib tests opened a real kernel ring, so
# `cargo test --lib` did not mean what its name implies. The repository's own
# Quality rule already classifies an operating-system API as an external
# boundary, which makes those integration tests living in the unit-test
# location.
#
# M24.2, M24.7 and M24.3 took it to 41. What matters now is that it does not
# climb back, and the reason it climbed in the first place is that nothing was
# watching -- not that anyone was careless.
#
# WHY THIS IS AN INVENTORY AND NOT A ZERO-CHECK. The obvious rule, and the one
# M24.5 originally assumed, is "no lib test constructs an IoRing". That rule is
# false and cannot be made true by effort:
#
#   * `event_delivery` needs a real ring and the thread pool.
#   * `ring`'s injected-failure cluster transforms a REAL completion on
#     purpose -- fabricating one is the unsoundness the seam exists to avoid,
#     and one of those tests says so in its own assertion message.
#   * `batch` needs a `Batch`, which needs the handle for its `Build*` calls.
#   * Several reach `#[cfg(test)] pub(crate)` helpers that exist only inside
#     the crate.
#
# A zero-check would fail on day one and could only be satisfied by deleting
# real coverage. So the check records *which* tests open a ring, and fails when
# that set changes -- the same mechanism, and for the same reason, as
# check-borrow-surface.ps1.
#
# WHY PER-TEST RATHER THAN PER-FILE. Two thirds of the remainder lives in
# `ring/tests.rs`. A file-level allow-list would permit that file to grow
# without limit, which is where a new ring-opening test would most naturally
# land. A bare count was rejected too: add-one-remove-one nets to zero and
# passes, and a number in a file is derived data nobody can check by reading.
#
# This deliberately checks *population*, not correctness. It cannot tell a test
# that needs a ring from one that merely uses it -- only that the set changed
# and a human owes an answer:
#
#     Does this test need the kernel, or only a ring-shaped thing? If the
#     latter, narrow what it reaches for (M24.7) or move it to tests/ (M24.3).
#
#   ./tools/check-ring-tests.ps1            # verify (CI)
#   ./tools/check-ring-tests.ps1 -Update    # regenerate after answering

[CmdletBinding()]
param(
    [switch]$Update
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent $PSScriptRoot
$sourceRoot = Join-Path $repoRoot 'crates\windows-ioring-sys\src'
$inventoryPath = Join-Path $repoRoot 'crates\windows-ioring-sys\RING-OPENING-LIB-TESTS.txt'

if (-not (Test-Path $sourceRoot)) {
    Write-Host "CONFIG ERROR: source root not found: $sourceRoot" -ForegroundColor Red
    exit 2
}

# One entry per `#[test]` in `src/**/tests.rs` whose body reaches a ring, either
# directly or through a helper in the same file that does.
function Get-RingOpeningTests {
    param([string]$Root)

    $entries = New-Object System.Collections.Generic.List[string]

    foreach ($file in (Get-ChildItem -Path $Root -Recurse -Filter 'tests.rs' | Sort-Object FullName)) {
        $text = [System.IO.File]::ReadAllText($file.FullName)
        $module = $file.Directory.Name

        # Helpers in this file that construct a ring themselves. A test calling
        # one of these opens a ring just as surely as one saying so inline,
        # which is the shape a per-file grep for `IoRing::new` would miss.
        $helpers = New-Object System.Collections.Generic.List[string]
        foreach ($match in [regex]::Matches($text, '(?m)^\s*fn\s+(\w+)')) {
            $start = $match.Index + $match.Length
            $stop = $text.IndexOf("`n}", $start)
            if ($stop -lt 0) { $stop = $text.Length }
            $body = $text.Substring($start, [Math]::Min(4000, $stop - $start))
            if ($body -match '(IoRing::new|::with_inventory|::with_version)\s*\(') { $helpers.Add($match.Groups[1].Value) | Out-Null }
        }

        $blocks = $text -split '#\[test\]'
        for ($i = 1; $i -lt $blocks.Count; $i++) {
            $block = $blocks[$i]
            $named = [regex]::Match($block, 'fn\s+(\w+)')
            if (-not $named.Success) { continue }
            $name = $named.Groups[1].Value

            # The test's own body ends at the first closing brace in column 0.
            # NOT at the next `#[`: a plain helper defined after the last test
            # in a file would otherwise be swallowed into that test's body, and
            # a helper containing `IoRing::new` would report the innocent test
            # above it as ring-opening. That false positive was produced by this
            # script's own bidirectional check before it was fixed. `cargo fmt`
            # is enforced here, so a column-0 `}` is reliably a function end.
            $end = $block.IndexOf("`n}")
            $body = if ($end -gt 0) { $block.Substring(0, $end) } else { $block }

            $opensRing = $body -match '(IoRing::new|::with_inventory|::with_version)\s*\('
            if (-not $opensRing) {
                foreach ($helper in $helpers) {
                    if ($helper -eq $name) { continue }
                    if ($body -match "\b$([regex]::Escape($helper))\s*\(") { $opensRing = $true; break }
                }
            }

            if ($opensRing) { $entries.Add("$module::$name") | Out-Null }
        }
    }

    return @($entries | Sort-Object)
}

$current = Get-RingOpeningTests -Root $sourceRoot

if ($Update) {
    $header = @(
        '# windows-ioring-sys: lib tests that open a real kernel ring.',
        '#',
        '# GENERATED by tools/check-ring-tests.ps1 -Update. Do not hand-edit.',
        '#',
        '# These are integration tests living in the unit-test location (D-49).',
        '# The list exists so the population cannot grow unnoticed, which is how',
        '# it reached 63 before anyone counted. Adding an entry obliges an',
        '# answer: does this test need the kernel, or only a ring-shaped thing?'
    )
    $body = $header + $current
    [System.IO.File]::WriteAllText($inventoryPath, ($body -join "`n") + "`n")
    Write-Host "Updated $inventoryPath ($($current.Count) entries)." -ForegroundColor Green
    exit 0
}

if (-not (Test-Path $inventoryPath)) {
    Write-Host "CONFIG ERROR: inventory not found: $inventoryPath" -ForegroundColor Red
    Write-Host "Create it with: ./tools/check-ring-tests.ps1 -Update" -ForegroundColor Yellow
    exit 2
}

$recorded = @([System.IO.File]::ReadAllLines($inventoryPath) |
    Where-Object { $_ -and -not $_.StartsWith('#') })

# `@(...)` on both: under StrictMode a pipeline yielding nothing is `$null` and
# one yielding a single string is a bare string, neither of which has `.Count`.
$added = @($current | Where-Object { $recorded -notcontains $_ })
$removed = @($recorded | Where-Object { $current -notcontains $_ })

if ($added.Count -eq 0 -and $removed.Count -eq 0) {
    Write-Host "Ring-opening lib tests unchanged ($($current.Count) entries)." -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host 'windows-ioring-sys: the set of lib tests that open a kernel ring changed.' -ForegroundColor Red
Write-Host ''

foreach ($entry in $added) {
    Write-Host "  ADDED    $entry" -ForegroundColor Yellow
}
foreach ($entry in $removed) {
    Write-Host "  REMOVED  $entry" -ForegroundColor Yellow
}

Write-Host ''
if ($added.Count -gt 0) {
    Write-Host 'An ADDED entry is the moment the question has to be asked, because a' -ForegroundColor Cyan
    Write-Host 'lib test that opens a ring is an integration test in the unit-test' -ForegroundColor Cyan
    Write-Host 'location (D-49), and the population reached 63 that way:' -ForegroundColor Cyan
    Write-Host ''
    Write-Host '    Does this test need the KERNEL, or only a ring-shaped thing?' -ForegroundColor White
    Write-Host ''
    Write-Host '    If only the latter, narrow what it reaches for -- Accounting takes' -ForegroundColor White
    Write-Host '    the ledger rather than the ring for exactly this reason (M24.7) --' -ForegroundColor White
    Write-Host '    or move it to tests/ if it uses only public API (M24.3).' -ForegroundColor White
    Write-Host ''
}
if ($removed.Count -gt 0) {
    Write-Host 'A REMOVED entry is progress and needs no justification, only the' -ForegroundColor Cyan
    Write-Host 'regeneration below so the inventory keeps describing the source.' -ForegroundColor Cyan
    Write-Host ''
}
Write-Host 'Then run:' -ForegroundColor Cyan
Write-Host '    ./tools/check-ring-tests.ps1 -Update' -ForegroundColor White
Write-Host ''
exit 1
