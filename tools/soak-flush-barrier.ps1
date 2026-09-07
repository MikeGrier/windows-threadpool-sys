# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Soak the flush-barrier stress instruments on a rotation, accumulating
    evidence across rounds.

.DESCRIPTION
    The defect these chase has been seen twice in three days, both times under a
    loaded full-workspace run, and never on demand. A single stress run answers
    "did it happen this time"; what is actually needed is a RATE, and which
    conditions move it. That takes repetition, so this runs the instruments in
    rotation and keeps a running tally.

    Each round runs the ignored tests in
    crates/windows-ioring-sys/tests/flush_barrier_stress.rs, one at a time, and
    records pass/fail per instrument. A failing round's full output is preserved
    -- that output contains the event log the test dumps, which is the whole
    point of the exercise and is gone the moment it scrolls past.

    Results accumulate under .scratch/flush-barrier-soak/, which is git-ignored:

      summary.csv          one row per instrument per round
      round-NNN-<name>.log the full output of a FAILING run, kept verbatim

    ROTATION ORDER IS DELIBERATE. Five instruments run in this order: quiet,
    contended, concurrent rings, the unordered control, then the depth sweep. A
    D-23 failure in the quiet instrument would mean something different from one
    under load -- it would say contention is not the trigger at all -- so it is
    worth knowing which of them moved first rather than discovering everything at
    once. The unordered control runs every round on purpose: it is what shows the
    completion queue reorders at all on this machine, without which a clean
    covering result might only mean nothing was there to reorder.

    WHAT IT REPORTS. D-23 (the flush waits for what precedes it) is asserted, and
    a failure there is a real defect. D-24's withdrawn hold-back half is counted
    and never asserted -- a later write completing first is the documented
    one-sided behaviour (D-47), and its rate is the thing worth watching.

    THIS WRITES A LOT. Each trial writes about 32 MiB of unbuffered device I/O,
    and a default round is five instruments at 100 trials each. Budget roughly
    fifteen GiB written per round, and prefer -TrialsPerRound on a small or
    heavily-worn disk.

.PARAMETER Rounds
    How many rotations to run. Default 5. Use 0 for "until stopped" (Ctrl+C).

.PARAMETER TrialsPerRound
    Passed to the tests as IORING_STRESS_TRIALS. Default 100, the tests' own
    default.

.PARAMETER Only
    Run just one instrument, by substring. Useful once a rotation has shown
    which one moves.

.PARAMETER OutputDirectory
    Where to accumulate. Defaults to .scratch/flush-barrier-soak.

.EXAMPLE
    ./tools/soak-flush-barrier.ps1
    ./tools/soak-flush-barrier.ps1 -Rounds 0 -TrialsPerRound 50
    ./tools/soak-flush-barrier.ps1 -Only concurrent -Rounds 20
#>
[CmdletBinding()]
param(
    [int]$Rounds = 5,
    [int]$TrialsPerRound = 100,
    [string]$Only,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'

# Resolved here rather than as a param default: Windows PowerShell 5.1 does not
# populate $PSScriptRoot while evaluating a default on a [CmdletBinding()]
# script, so the default form fails outright under 5.1 while working under 7.
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $PSScriptRoot '..\.scratch\flush-barrier-soak'
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$summary = Join-Path $OutputDirectory 'summary.csv'

# The rotation, in the order described above.
$instruments = @(
    'the_drain_holds_across_many_quiet_trials'
    'the_drain_holds_under_deliberate_disk_contention'
    'the_drain_holds_with_concurrent_rings'
    'the_unordered_control_still_discriminates_under_contention'
    'reordering_rate_by_ring_depth_is_reported'
)
if ($Only) {
    $instruments = @($instruments | Where-Object { $_ -like "*$Only*" })
    if ($instruments.Count -eq 0) {
        Write-Host "::error::no instrument matches '$Only'"
        exit 2
    }
}

if (-not (Test-Path -LiteralPath $summary)) {
    Set-Content -LiteralPath $summary -Value 'timestamp,round,instrument,result,seconds,detail' -Encoding utf8
}

Write-Host "Soaking the flush barrier."
Write-Host "  repository : $repoRoot"
Write-Host "  output     : $OutputDirectory"
Write-Host "  rounds     : $(if ($Rounds -eq 0) { 'until stopped (Ctrl+C)' } else { $Rounds })"
Write-Host "  trials     : $TrialsPerRound per instrument"
Write-Host "  instruments: $($instruments.Count)"
Write-Host ""

# Build once, so a round's timing measures the instrument rather than rustc.
Write-Host "Building the test binary once..."
& cargo test -p windows-ioring-sys --test flush_barrier_stress --no-run --quiet 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Host "::error::the stress test binary does not build"
    exit 1
}

$tally = @{}
foreach ($name in $instruments) { $tally[$name] = @{ Runs = 0; Failures = 0 } }

$round = 0
try {
    while ($Rounds -eq 0 -or $round -lt $Rounds) {
        $round++
        Write-Host "=== round $round ==="

        foreach ($name in $instruments) {
            $started = Get-Date
            $env:IORING_STRESS_TRIALS = "$TrialsPerRound"
            $output = & cargo test -p windows-ioring-sys --test flush_barrier_stress `
                -- --ignored --nocapture --exact $name 2>&1
            $code = $LASTEXITCODE
            Remove-Item Env:\IORING_STRESS_TRIALS -ErrorAction SilentlyContinue
            $seconds = [int]((Get-Date) - $started).TotalSeconds

            $tally[$name].Runs++
            $result = if ($code -eq 0) { 'pass' } else { 'FAIL'; }
            if ($code -ne 0) { $tally[$name].Failures++ }

            # The report line the instrument printed, so the CSV carries the
            # rate rather than only pass/fail.
            #
            # Matched on 'D-23', a decision ID rather than prose. This pattern has
            # now been broken TWICE by rewording the report -- first when it
            # matched 'violation(s) in', then when it matched 'trial(s) violated'
            # and D-47's correction renamed that to 'had a completion cross the
            # flush'. Both times the CSV kept filling with blank detail columns
            # and nothing said so.
            #
            # So the pattern is anchored to something that does not get reworded,
            # AND a miss is now reported instead of silently producing an empty
            # column -- a soak run whose detail is quietly blank is worse than one
            # that fails loudly, because the tally still looks healthy.
            $detail = ($output | Select-String -Pattern 'D-23' |
                Select-Object -First 1).Line
            if ($detail) {
                $detail = $detail.Trim() -replace ',', ';'
            } else {
                $detail = ''
                Write-Host "      warning: no report line matched 'D-23' -- the harness's report wording may have changed; the detail column is empty" -ForegroundColor Yellow
            }

            "{0},{1},{2},{3},{4},{5}" -f (Get-Date -Format 'o'), $round, $name, $result, $seconds, $detail |
                Add-Content -LiteralPath $summary -Encoding utf8

            $mark = if ($code -eq 0) { 'pass' } else { '*** FAIL ***' }
            Write-Host ("  {0,-58} {1,-12} {2,4}s" -f $name, $mark, $seconds)
            if ($detail) { Write-Host "      $detail" }

            # A failing round's output holds the event log. Keep it verbatim --
            # it is the only record of what happened, and re-running will not
            # reproduce the same interleaving.
            if ($code -ne 0) {
                $log = Join-Path $OutputDirectory ("round-{0:D3}-{1}.log" -f $round, $name)
                $output | Out-File -LiteralPath $log -Encoding utf8
                Write-Host "      full output kept at $log"
            }
        }
        Write-Host ""
    }
}
finally {
    Write-Host "=== tally after $round round(s) ==="
    $anyFailure = $false
    foreach ($name in $instruments) {
        $t = $tally[$name]
        if ($t.Runs -eq 0) { continue }
        if ($t.Failures -gt 0) { $anyFailure = $true }
        Write-Host ("  {0,-58} {1} failure(s) in {2} run(s)" -f $name, $t.Failures, $t.Runs)
    }
    Write-Host ""
    Write-Host "  summary: $summary"
    if ($anyFailure) {
        Write-Host "  Failing rounds' full output (with the event logs) is in $OutputDirectory."
    }
}

exit 0
