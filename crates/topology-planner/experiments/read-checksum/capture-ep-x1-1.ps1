# Copyright (c) Mike Grier.
[CmdletBinding()]
param([Parameter(Mandatory)][string]$OutputDirectory)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
$scratch = [IO.Path]::GetFullPath((Join-Path $workspace '.scratch')) + [IO.Path]::DirectorySeparatorChar
if (-not $output.StartsWith($scratch, [StringComparison]::OrdinalIgnoreCase)) {
    throw 'OutputDirectory must be below the workspace .scratch directory.'
}
if (Test-Path -LiteralPath $output) { throw 'OutputDirectory must not exist.' }
$binary = Join-Path $workspace 'target/release/windows-read-checksum-experiment.exe'
$binaryHash = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash
[void][IO.Directory]::CreateDirectory($output)

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
Invoke-Experiment -Arguments @('fixture', $fixture, '33554449') -ExpectedStatus 'fixture_created'
$processors = $null
$summaries = @()
foreach ($case in @(
    @{ Name = 'control-before'; Passes = 1 },
    @{ Name = 'compute-heavy'; Passes = 16 },
    @{ Name = 'control-after'; Passes = 1 }
)) {
    $config = [ordered]@{
        file = $fixture
        block_bytes = 65536
        depth = 8
        queue_capacity = 4
        checksum_passes = $case.Passes
        repetitions = 12
        timeout_ms = 30000
        processors = $processors
    }
    $configPath = Join-Path $output ($case.Name + '-config.json')
    $reportPath = Join-Path $output ($case.Name + '.json')
    Write-Json $configPath $config
    Invoke-Experiment -Arguments @('run', $configPath, $reportPath)
    $capture = [IO.File]::ReadAllText($reportPath) | ConvertFrom-Json
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
    $summaries += [ordered]@{ artifact = $case.Name + '.json'; groups = $groups; paired = $paired }
}
$summary = [ordered]@{
    schema = 'ep-x1-1-summary-v1'
    executable_sha256 = $binaryHash
    script_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    generated_utc = [DateTimeOffset]::UtcNow.ToString('o')
    median_method = 'nearest_rank'
    captures = $summaries
}
Write-Json (Join-Path $output 'summary.json') $summary
$summary | ConvertTo-Json -Depth 30