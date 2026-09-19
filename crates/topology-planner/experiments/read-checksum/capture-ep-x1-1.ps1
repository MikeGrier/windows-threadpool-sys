# Copyright (c) Mike Grier.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputDirectory,
    [ValidateSet('EP-X1.1', 'EP-X1.2')][string]$Study = 'EP-X1.1',
    [switch]$PlanOnly,
    [switch]$SummarizeOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
$scratch = [IO.Path]::GetFullPath((Join-Path $workspace '.scratch')) + [IO.Path]::DirectorySeparatorChar
if (-not $output.StartsWith($scratch, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'OutputDirectory must be below the workspace .scratch directory.'
}
if ((Test-Path -LiteralPath $output) -and -not $SummarizeOnly) { throw 'OutputDirectory must not exist.' }
if ($PlanOnly -and $SummarizeOnly) { throw 'PlanOnly and SummarizeOnly are mutually exclusive.' }

function Write-Json([string]$Path, $Value) {
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew)
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    try { $writer.Write(($Value | ConvertTo-Json -Depth 30).Replace("`r`n", "`n") + [char]10) }
    finally { $writer.Dispose() }
}

function Invoke-Experiment([string[]]$Arguments, [string]$ExpectedStatus = 'success') {
    if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -ne $binaryHash) {
        throw 'Executable changed during the capture sequence.'
    }
    $status = & $binary @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Experiment failed: $status" }
    if (($status | ConvertFrom-Json).status -ne $ExpectedStatus) { throw "Unexpected status: $status" }
}

function Describe([double[]]$Values) {
    $ordered = @($Values | Sort-Object)
    [ordered]@{
        min = $ordered[0]
        median = $ordered[[int][Math]::Ceiling($ordered.Count / 2.0) - 1]
        max = $ordered[-1]
    }
}

$fixture = Join-Path $output 'fixture.dat'
function New-Config {
    [ordered]@{
        file = $fixture
        block_bytes = 65536
        depth = 8
        queue_capacity = 4
        checksum_passes = 1
        repetitions = 6
        timeout_ms = 30000
        processors = $null
    }
}

$cases = @()
if ($Study -eq 'EP-X1.1') {
    foreach ($name in @('control-before', 'compute-heavy', 'control-after')) {
        $config = New-Config
        $config.repetitions = 12
        if ($name -eq 'compute-heavy') { $config.checksum_passes = 16 }
        $cases += [ordered]@{ Name = $name; Sweep = 'repeatability'; Point = $name; ChangedFields = @(); Config = $config }
    }
} else {
    $sweeps = @(
        @{ Name = 'block'; Field = 'block_bytes'; Values = @(16384, 262144) },
        @{ Name = 'compute'; Field = 'checksum_passes'; Values = @(4, 16) },
        @{ Name = 'depth'; Field = 'depth'; Values = @(2, 32) },
        @{ Name = 'queue'; Field = 'queue_capacity'; Values = @(1, 16) },
        @{ Name = 'batch'; Field = 'batch_size'; Values = @(4, 16) },
        @{ Name = 'queue-batch'; Field = 'interaction'; Values = @() }
    )
    foreach ($sweep in $sweeps) {
        $sequence = @('control', 'first', 'second', 'second', 'first', 'control')
        if ($sweep.Field -eq 'interaction') {
            $sequence = @('control', 'queue', 'batch', 'both', 'both', 'batch', 'queue', 'control')
        }
        for ($position = 0; $position -lt $sequence.Count; $position++) {
            $point = $sequence[$position]
            $config = New-Config
            $config.buffer_count = 32
            $config.batch_size = 1
            $changed = @()
            if ($sweep.Field -eq 'interaction') {
                if ($point -in @('queue', 'both')) { $config.queue_capacity = 1; $changed += 'queue_capacity' }
                if ($point -in @('batch', 'both')) { $config.batch_size = 16; $changed += 'batch_size' }
            } elseif ($point -ne 'control') {
                $index = [int]($point -eq 'second')
                $config[$sweep.Field] = $sweep.Values[$index]
                $changed += $sweep.Field
                if ($sweep.Field -eq 'block_bytes') {
                    $config.buffer_count = 2097152 / $config.block_bytes
                    $changed += 'buffer_count'
                }
            }
            $cases += [ordered]@{
                Name = '{0}-{1:D2}-{2}' -f $sweep.Name, $position, $point
                Sweep = $sweep.Name; Point = $point; ChangedFields = $changed; Config = $config
            }
        }
    }
}
$plan = [ordered]@{ study = $Study; fixture_bytes = 33554449; cases = $cases }
if ($PlanOnly) { $plan | ConvertTo-Json -Depth 30; return }
$binary = Join-Path $workspace 'target/release/windows-read-checksum-experiment.exe'
if ($SummarizeOnly) {
    $plan = [IO.File]::ReadAllText((Join-Path $output 'matrix.json')) | ConvertFrom-Json
    $cases = $plan.cases
    $Study = $plan.study
    $previous = [IO.File]::ReadAllText((Join-Path $output 'summary.json')) | ConvertFrom-Json
    $binaryHash = $previous.executable_sha256
    $captureScriptHash = if ($previous.PSObject.Properties['capture_script_sha256']) { $previous.capture_script_sha256 } else { $previous.script_sha256 }
} else {
    $binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
    $captureScriptHash = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    [void][IO.Directory]::CreateDirectory($output)
    Write-Json (Join-Path $output 'matrix.json') $plan
    Invoke-Experiment -Arguments @('fixture', $fixture, $plan.fixture_bytes.ToString()) -ExpectedStatus 'fixture_created'
}
$processors = $null
$buildIdentity = $null
$summaries = @()
foreach ($case in $cases) {
    $config = $case.Config
    $config.processors = $processors
    $configPath = Join-Path $output ($case.Name + '-config.json')
    $reportPath = Join-Path $output ($case.Name + '.json')
    if (-not $SummarizeOnly) {
        Write-Json $configPath $config
        Invoke-Experiment -Arguments @('run', $configPath, $reportPath)
    }
    $capture = [IO.File]::ReadAllText($reportPath) | ConvertFrom-Json
    $identity = @{ build = $capture.build; processors = $capture.processors } | ConvertTo-Json -Depth 30 -Compress
    if ($null -eq $buildIdentity) { $buildIdentity = $identity }
    if ($identity -ne $buildIdentity) { throw 'Build identity or processor pair changed.' }
    $processors = $capture.processors
    $groups = @($capture.trials | Group-Object arrangement, reversed | ForEach-Object {
        $trials = @($_.Group)
        [ordered]@{
            arrangement = $trials[0].arrangement
            reversed = $trials[0].reversed
            samples = $trials.Count
            bytes_per_second = Describe @($trials | ForEach-Object { $_.bytes_per_second })
            worker_cpu_ns = Describe @($trials | ForEach-Object { $_.worker_cpu_ns })
            cpu_per_wall = Describe @($trials | ForEach-Object { $_.worker_cpu_ns / $_.work_wall_ns })
            checksum_p99_ns = Describe @($trials | ForEach-Object { $_.checksum_latency.p99_ns })
            read_p99_ns = Describe @($trials | ForEach-Object { $_.observed_read_latency.p99_ns })
            compute_p99_ns = Describe @($trials | ForEach-Object { $_.compute_time.p99_ns })
            handoff_p99_ns = if ($trials[0].arrangement -eq 'pipeline') { Describe @($trials | ForEach-Object { $_.handoff_latency.p99_ns }) } else { $null }
            handoff_full_observations = Describe @($trials | ForEach-Object { ($_.workers.handoff_full_observations | Measure-Object -Sum).Sum })
            buffer_pressure_observations = Describe @($trials | ForEach-Object { ($_.workers.buffer_pressure_observations | Measure-Object -Sum).Sum })
            largest_worker_leased_bytes = Describe @($trials | ForEach-Object { ($_.workers.peak_leased_buffers | Measure-Object -Maximum).Maximum * $config.block_bytes })
        }
    })
    $paired = @(foreach ($reversed in @($false, $true)) {
        foreach ($repetition in 0..($config.repetitions - 1)) {
            $trials = @($capture.trials | Where-Object { $_.reversed -eq $reversed -and $_.repetition -eq $repetition })
            $direct = $trials | Where-Object arrangement -eq 'direct'
            $pipeline = $trials | Where-Object arrangement -eq 'pipeline'
            $independent = $trials | Where-Object arrangement -eq 'independent'
            [ordered]@{
                repetition = $repetition
                reversed = $reversed
                pipeline_minus_direct_bytes_per_second = $pipeline.bytes_per_second - $direct.bytes_per_second
                independent_minus_pipeline_bytes_per_second = $independent.bytes_per_second - $pipeline.bytes_per_second
            }
        }
    })
    $summaries += [ordered]@{ artifact = $case.Name + '.json'; sweep = $case.Sweep; point = $case.Point; changed_fields = $case.ChangedFields; groups = $groups; paired = $paired }
}
$effects = @(foreach ($sweep in ($summaries | Group-Object { $_.sweep })) {
    $controls = @($sweep.Group | Where-Object point -eq 'control')
    if ($controls.Count -eq 0) { continue }
    foreach ($treatment in $controls[0].groups) {
        $controlGroups = @($controls.groups | Where-Object { $_.arrangement -eq $treatment.arrangement -and $_.reversed -eq $treatment.reversed })
        $controlMin = ($controlGroups.bytes_per_second.min | Measure-Object -Minimum).Minimum
        $controlMax = ($controlGroups.bytes_per_second.max | Measure-Object -Maximum).Maximum
        foreach ($point in ($sweep.Group | Where-Object point -ne 'control' | Group-Object { $_.point })) {
            $groups = @($point.Group.groups | Where-Object { $_.arrangement -eq $treatment.arrangement -and $_.reversed -eq $treatment.reversed })
            $pointMin = ($groups.bytes_per_second.min | Measure-Object -Minimum).Minimum
            $pointMax = ($groups.bytes_per_second.max | Measure-Object -Maximum).Maximum
            [ordered]@{
                sweep = $sweep.Name; point = $point.Name
                arrangement = $treatment.arrangement; reversed = $treatment.reversed
                control_bytes_per_second = @{ min = $controlMin; max = $controlMax; capture_medians = @($controlGroups.bytes_per_second.median) }
                point_bytes_per_second = @{ min = $pointMin; max = $pointMax; capture_medians = @($groups.bytes_per_second.median) }
                ranges_overlap = $pointMin -le $controlMax -and $controlMin -le $pointMax
            }
        }
    }
})
$summary = [ordered]@{
    schema = 'read-checksum-sweep-summary-v2'
    study = $Study
    executable_sha256 = $binaryHash
    capture_script_sha256 = $captureScriptHash
    script_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    generated_utc = [DateTimeOffset]::UtcNow.ToString('o')
    median_method = 'nearest_rank'
    captures = $summaries
    effects = $effects
}
$summaryName = if ($SummarizeOnly) { 'summary-recomputed.json' } else { 'summary.json' }
Write-Json (Join-Path $output $summaryName) $summary
$summary | ConvertTo-Json -Depth 30