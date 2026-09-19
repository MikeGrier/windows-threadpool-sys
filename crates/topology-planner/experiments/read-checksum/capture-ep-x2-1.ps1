# Copyright (c) Mike Grier.
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$OutputDirectory,
    [string]$Binary,
    [switch]$PlanOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$workspace = (Resolve-Path (Join-Path $PSScriptRoot '../../../..')).Path
$output = [IO.Path]::GetFullPath($OutputDirectory)
$scratch = [IO.Path]::GetFullPath((Join-Path $workspace '.scratch')) + [IO.Path]::DirectorySeparatorChar
if (-not $output.StartsWith($scratch, [StringComparison]::OrdinalIgnoreCase)) { throw 'OutputDirectory must be below .scratch.' }
if (Test-Path -LiteralPath $output) { throw 'OutputDirectory must not exist.' }
if (-not $Binary) { $Binary = Join-Path $workspace 'target/release/windows-read-checksum-experiment.exe' }
$binaryHash = (Get-FileHash -LiteralPath $Binary -Algorithm SHA256).Hash

function Write-Json([string]$Path, $Value) {
    $stream = [IO.File]::Open($Path, [IO.FileMode]::CreateNew)
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    try { $writer.Write(($Value | ConvertTo-Json -Depth 80).Replace("`r`n", "`n") + [char]10) }
    finally { $writer.Dispose() }
}

function Invoke-Experiment([string[]]$Arguments) {
    if ((Get-FileHash -LiteralPath $Binary -Algorithm SHA256).Hash -ne $binaryHash) { throw 'Executable changed during capture.' }
    $response = & $Binary @Arguments
    if ($LASTEXITCODE -ne 0) { throw "Experiment failed: $response" }
    $response | ConvertFrom-Json
}

function Describe([double[]]$Values) {
    $ordered = @($Values | Sort-Object)
    [ordered]@{ min = $ordered[0]; median = $ordered[[int][Math]::Ceiling($ordered.Count / 2.0) - 1]; max = $ordered[-1] }
}

$discovery = Invoke-Experiment -Arguments @('placements')
$fixture = Join-Path $output 'fixture.dat'
$cases = @()
for ($pairIndex = 0; $pairIndex -lt $discovery.pairs.Count; $pairIndex++) {
    $pair = $discovery.pairs[$pairIndex]
    $nodes = @($pair.memory_nodes | Where-Object { $null -ne $_ } | Sort-Object -Unique)
    foreach ($inputKind in @('buffered_file', 'generated')) {
        $placements = @($null) + $nodes + @($nodes | Sort-Object -Descending) + @($null)
        for ($position = 0; $position -lt $placements.Count; $position++) {
            $node = $placements[$position]
            $label = if ($null -eq $node) { 'heap-control' } else { 'node-' + $node }
            $cases += [ordered]@{
                name = '{0}-pair-{1:D2}-{2:D2}-{3}' -f $inputKind, $pairIndex, $position, $label
                pair_index = $pairIndex
                payload_node = $node
                config = [ordered]@{
                    file = $fixture; input = $inputKind
                    generated_bytes = if ($inputKind -eq 'generated') { 33554449 } else { $null }
                    payload_node = $node; block_bytes = 65536; depth = 8; buffer_count = 32
                    queue_capacity = 4; checksum_passes = 1; batch_size = 1
                    repetitions = 6; timeout_ms = 30000; processors = $pair.processors
                }
            }
        }
    }
}
$plan = [ordered]@{ schema = 'ep-x2-1-plan-v1'; discovery = $discovery; cases = $cases }
if ($PlanOnly) { $plan | ConvertTo-Json -Depth 80; return }
[void][IO.Directory]::CreateDirectory($output)
Write-Json (Join-Path $output 'plan.json') $plan
$fixtureStatus = Invoke-Experiment -Arguments @('fixture', $fixture, '33554449')
if ($fixtureStatus.status -ne 'fixture_created') { throw 'Unexpected fixture status.' }
$results = @()
$buildIdentity = $null
foreach ($case in $cases) {
    $configPath = Join-Path $output ($case.name + '-config.json')
    $reportPath = Join-Path $output ($case.name + '.json')
    Write-Json $configPath $case.config
    try {
        $status = Invoke-Experiment -Arguments @('run', $configPath, $reportPath)
        if ($status.status -ne 'success') { throw 'Unexpected run status.' }
        $capture = [IO.File]::ReadAllText($reportPath) | ConvertFrom-Json
        $identity = $capture.build | ConvertTo-Json -Compress
        if ($null -eq $buildIdentity) { $buildIdentity = $identity }
        if ($identity -ne $buildIdentity) { throw 'Build provenance changed.' }
        if (($capture.topology | ConvertTo-Json -Depth 80 -Compress) -ne ($discovery.topology | ConvertTo-Json -Depth 80 -Compress)) { throw 'Topology snapshot changed; selection no longer describes this capture.' }
        $groups = @($capture.trials | Group-Object arrangement, reversed | ForEach-Object {
            $trials = @($_.Group)
            $residency = @($trials.workers | ForEach-Object { $_.pages_before; $_.pages_after } | Where-Object { $null -ne $_ })
            $placement = if ($null -eq $case.payload_node) { 'unrequested' } else { 'observed_on_requested_node' }
            if ($null -ne $case.payload_node) {
                if (@($residency | Where-Object { $_.unresident_pages -ne 0 -or $_.node_ids_truncated }).Count) { $placement = 'unknown_pages_or_truncated_labels' }
                if (@($residency | Where-Object { -not $_.node_ids_truncated } | ForEach-Object { $_.pages_by_node.PSObject.Properties.Name } | Where-Object { [uint32]$_ -ne [uint32]$case.payload_node }).Count) { $placement = 'observed_node_mismatch' }
            }
            [ordered]@{
                arrangement = $trials[0].arrangement; reversed = $trials[0].reversed
                samples = $trials.Count; placement = $placement
                bytes_per_second = Describe @($trials.bytes_per_second)
                worker_cpu_ns = Describe @($trials.worker_cpu_ns)
                checksum_p99_ns = Describe @($trials | ForEach-Object { $_.checksum_latency.p99_ns })
                source_p99_ns = Describe @($trials | ForEach-Object { $_.observed_read_latency.p99_ns })
                handoff_p99_ns = if ($trials[0].arrangement -eq 'pipeline') { Describe @($trials | ForEach-Object { $_.handoff_latency.p99_ns }) } else { $null }
            }
        })
        $results += [ordered]@{ name = $case.name; pair_index = $case.pair_index; payload_node = $case.payload_node; status = 'success'; groups = $groups }
    } catch {
        $results += [ordered]@{ name = $case.name; status = 'error'; error = $_.Exception.Message }
        break
    }
}
$failures = @($results | Where-Object { $_.status -eq 'error' })
$summary = [ordered]@{
    schema = 'ep-x2-1-summary-v1'
    status = if ($failures.Count) { 'error' } elseif ($discovery.unavailable.Count) { 'available_placements_captured' } else { 'success' }
    executable_sha256 = $binaryHash; script_sha256 = (Get-FileHash -LiteralPath $PSCommandPath -Algorithm SHA256).Hash
    generated_utc = [DateTimeOffset]::UtcNow.ToString('o'); unavailable = $discovery.unavailable
    unrun_cases = @($cases | Select-Object -Skip $results.Count | ForEach-Object { $_.name })
    captures = $results
}
Write-Json (Join-Path $output 'summary.json') $summary
$summary | ConvertTo-Json -Depth 80
if ($failures.Count) { throw 'Placement capture failed; see retained summary and report.' }