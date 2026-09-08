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

    THIS WRITES A LOT. Each trial writes about 32 MiB of unbuffered device I/O.
    A default round is not simply five times 100, because two instruments
    subdivide the budget (see -TrialsPerRound): on an 8-thread machine it is
    100 + 100 + 100 quiet/contended/control, 96 concurrent, and 75 across the
    depth sweep -- 471 trials, so roughly fifteen GiB written per round. Prefer
    -TrialsPerRound on a small or heavily-worn disk.

.PARAMETER Rounds
    How many rotations to run. Default 5. Use 0 for "until stopped" (Ctrl+C).

.PARAMETER TrialsPerRound
    Passed to the tests as IORING_STRESS_TRIALS. Default 100, the tests' own
    default.

    A BUDGET, NOT A PER-INSTRUMENT COUNT. Quiet, contended and the unordered
    control each run exactly this many trials. The other two subdivide it:
    concurrent rings runs (N / threads).max(5) per thread across `threads`
    threads, and the depth sweep runs (N / 4).max(5) per depth across three
    depths -- so at 40 on an 8-thread machine the depth sweep does 30 trials, not
    40. The .max(5) floor also dominates below about 20, where raising the value
    changes some instruments and not others.

    This matters when comparing a rate between instruments in summary.csv, and
    when budgeting the writes below. The harness's own `trials` documentation
    carries the table.

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
    # Validated rather than passed through: IORING_STRESS_TRIALS=0 makes every
    # instrument run nothing, divide by zero when reporting, and pass. The
    # harness rejects it too, but failing here names the parameter the operator
    # actually typed instead of surfacing as a panic inside a test.
    [ValidateRange(1, [int]::MaxValue)]
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

# Every cargo invocation names the manifest explicitly, so the script works from
# any working directory. Without it, running this from anywhere but the repo root
# fails with "could not find Cargo.toml in <cwd> or any parent directory" -- and
# the failure lands in the build step, where it reads like a broken tree rather
# than a wrong directory.
$manifest = Join-Path $repoRoot 'Cargo.toml'
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$OutputDirectory = (Resolve-Path -LiteralPath $OutputDirectory).Path
$summary = Join-Path $OutputDirectory 'summary.csv'

# Sentinel written into the CSV when the report line could not be found. An
# empty field is ambiguous -- it reads the same as "this run produced no detail"
# -- and this one is unmistakable to whoever later analyses the file.
$script:NoDetail = '<<NO REPORT LINE MATCHED>>'

# The single output sink. Every message this tool emits goes through here, so
# the destination and the formatting stay separable from the call sites that
# produce the content -- the repository's one-output-sink rule, and the same
# shape as tools/run-mutants.ps1 and tools/run-numa-spikes.ps1.
function Write-Report {
    param(
        [Parameter(Mandatory = $true, ValueFromPipeline = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'detail', 'heading', 'warning', 'error')][string] $Level = 'info'
    )
    process {
        # The only Write-Host calls in this script, by design.
        switch ($Level) {
            'error' { Write-Host "::error::$Message" }
            'warning' { Write-Host "::warning::$Message" -ForegroundColor Yellow }
            'heading' { Write-Host $Message -ForegroundColor Cyan }
            'detail' { Write-Host $Message -ForegroundColor DarkGray }
            default { Write-Host $Message }
        }
    }
}

# `Invoke-Native` and `ConvertTo-OutputLines`, which every capture site here goes
# through. Dot-sourced rather than imported: a module copy of that guard does not
# reach the scriptblock it is handed, and silently fails on 5.1 alone. The full
# argument, and the measurement behind it, is in that file.
#
# What it costs THIS script, recorded here because the shape is specific to the
# soak: cargo writes to stderr routinely -- "Compiling ...", and the "did not
# finalize incremental compilation session directory" notes this workspace emits
# constantly. Measured: under 5.1 the script died in the build step with
# NativeCommandError before running a single instrument, having written only the
# CSV header. Under 7 the same script completed.
#
# Used by every capture site rather than one: this was originally fixed at the
# build step alone, leaving the per-instrument run with the same defect, which is
# how two copies of one rule drift apart.
. (Join-Path $PSScriptRoot 'common.ps1')

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
        Write-Report "no instrument matches '$Only'" -Level error
        exit 2
    }
}

# Every file this script writes goes through this encoding, so the bytes are the
# same whichever shell ran it. `-Encoding utf8` is not portable between the two:
# on Windows PowerShell 5.1 it means UTF-8 *with* BOM and on 7 it means without,
# measured here as EF BB BF against nothing for the same Set-Content call. A CSV
# that sometimes starts with a BOM is a parsing hazard for whatever reads it
# later. Same reasoning and same form as tools/run-numa-spikes.ps1.
$script:Utf8NoBom = New-Object System.Text.UTF8Encoding $false

if (-not (Test-Path -LiteralPath $summary)) {
    [System.IO.File]::WriteAllText(
        $summary,
        "timestamp,round,instrument,result,seconds,detail`r`n",
        $script:Utf8NoBom)
}

Write-Report "Soaking the flush barrier." -Level heading
Write-Report "  repository : $repoRoot"
Write-Report "  output     : $OutputDirectory"
Write-Report "  rounds     : $(if ($Rounds -eq 0) { 'until stopped (Ctrl+C)' } else { $Rounds })"
Write-Report "  trials     : $TrialsPerRound per instrument"
Write-Report "  instruments: $($instruments.Count)"
Write-Report ""

# Build once, so a round's timing measures the instrument rather than rustc.
#
# The output is captured rather than discarded, and printed on failure. An
# earlier version sent it to Out-Null and reported only "does not build", which
# is the same message for a missing toolchain, a wrong directory and a genuine
# compile error -- and the one case where the operator most needs the detail.
Write-Report "Building the test binary once..."
$buildOutput = Invoke-Native {
    cargo test --manifest-path $manifest `
        -p windows-ioring-sys --test flush_barrier_stress --no-run
}
if ($LASTEXITCODE -ne 0) {
    Write-Report "the stress test binary does not build" -Level error
    $buildOutput | ForEach-Object { Write-Report "  $_" -Level detail }
    exit 1
}

$tally = @{}
foreach ($name in $instruments) { $tally[$name] = @{ Runs = 0; Failures = 0 } }

$round = 0
try {
    while ($Rounds -eq 0 -or $round -lt $Rounds) {
        $round++
        Write-Report "=== round $round ===" -Level heading

        foreach ($name in $instruments) {
            $started = Get-Date
            $env:IORING_STRESS_TRIALS = "$TrialsPerRound"
            $output = Invoke-Native {
                cargo test --manifest-path $manifest `
                    -p windows-ioring-sys --test flush_barrier_stress `
                    -- --ignored --nocapture --exact $name
            }
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
            # The FIRST match is taken, which makes the instrument's print order
            # part of this contract: an instrument printing several reports must
            # print its representative one first. That has been wrong twice --
            # the concurrent-rings instrument recorded worker 0 and the depth
            # sweep recorded depth 128, each in a column labelled with the whole
            # instrument. The rule is now stated in the harness's module docs
            # too, since it constrains that file rather than this one.
            #
            # So the pattern is anchored to something that does not get reworded,
            # AND a miss is recorded rather than passed over. Two separate
            # signals, because they reach different people: a warning for whoever
            # is watching the run, and a sentinel in the CSV for whoever analyses
            # it later. An empty field would be ambiguous -- indistinguishable
            # from a run that legitimately produced no detail -- and is exactly
            # the shape that let this go unnoticed twice.
            $detail = ($output | Select-String -Pattern 'D-23' |
                Select-Object -First 1).Line
            if ($detail) {
                $detail = $detail.Trim() -replace ',', ';'
            } else {
                $detail = $script:NoDetail
                Write-Report "no report line matched 'D-23' -- the harness's report wording may have changed; the CSV records $script:NoDetail" -Level warning
            }

            $row = "{0},{1},{2},{3},{4},{5}" -f (Get-Date -Format 'o'), $round, $name, $result, $seconds, $detail
            [System.IO.File]::AppendAllText($summary, "$row`r`n", $script:Utf8NoBom)

            $mark = if ($code -eq 0) { 'pass' } else { '*** FAIL ***' }
            Write-Report ("  {0,-58} {1,-12} {2,4}s" -f $name, $mark, $seconds)
            if ($detail) { Write-Report "      $detail" -Level detail }

            # A failing round's output holds the event log. Keep it verbatim --
            # it is the only record of what happened, and re-running will not
            # reproduce the same interleaving.
            if ($code -ne 0) {
                $log = Join-Path $OutputDirectory ("round-{0:D3}-{1}.log" -f $round, $name)
                [System.IO.File]::WriteAllLines($log, [string[]]$output, $script:Utf8NoBom)
                Write-Report "      full output kept at $log" -Level detail
            }
        }
        Write-Report ""
    }
}
finally {
    Write-Report "=== tally after $round round(s) ===" -Level heading
    $anyFailure = $false
    foreach ($name in $instruments) {
        $t = $tally[$name]
        if ($t.Runs -eq 0) { continue }
        if ($t.Failures -gt 0) { $anyFailure = $true }
        Write-Report ("  {0,-58} {1} failure(s) in {2} run(s)" -f $name, $t.Failures, $t.Runs)
    }
    Write-Report ""
    Write-Report "  summary: $summary"
    if ($anyFailure) {
        Write-Report "  Failing rounds' full output (with the event logs) is in $OutputDirectory."
    }
}

exit 0
