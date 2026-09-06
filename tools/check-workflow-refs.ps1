# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Verifies that every crate, binary, example, and script a workflow names
    actually exists.

.DESCRIPTION
    A workflow is the one part of this repository that no local command
    exercises. `cargo check`, `cargo clippy`, and every other tool here read
    Rust and manifests; none of them read `.github/workflows/`, so a step that
    names a `--bin` which does not exist is syntactically fine, passes every
    local gate, and fails only on a hosted runner minutes after the push.

    That is not hypothetical. This script exists because a change lifted a
    workflow wholesale from a feature branch, which carried three steps naming
    probe binaries that lived only on that branch. Nothing local could have
    caught it: the YAML was valid, the crate built, the tests passed. The build
    went red on `no bin target named 'probe-topology'`.

    The class is broader than that one mistake. Any rename or deletion of a
    crate, a binary, an example, or a `tools/` script silently breaks whatever
    workflow still names the old one, and the break surfaces on the next push
    rather than at the edit. This closes the loop by resolving each reference
    against `cargo metadata` and the filesystem.

    Checked, per workflow file:

      -p / --package    must name a workspace package
      --bin             must name a bin target of some workspace package
      --example         must name an example target of some workspace package
      ./tools/<script>  must exist on disk

    References containing a GitHub expression (`${{ ... }}`) are skipped, since
    their value is not known until the workflow runs.

.PARAMETER WorkflowDirectory
    Directory of workflow files to check. Defaults to `.github/workflows`.

.EXAMPLE
    ./tools/check-workflow-refs.ps1
#>
[CmdletBinding()]
param(
    [string]$WorkflowDirectory
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

# Resolved here rather than as a param default: Windows PowerShell 5.1 does not
# populate $PSScriptRoot while evaluating a default on a [CmdletBinding()]
# script, so the default form fails outright under 5.1 while working under 7.
if (-not $WorkflowDirectory) {
    $WorkflowDirectory = Join-Path $PSScriptRoot '..\.github\workflows'
}

if (-not (Test-Path -LiteralPath $WorkflowDirectory)) {
    Write-Host "No workflow directory at $WorkflowDirectory; nothing to check."
    exit 0
}

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')

# One `cargo metadata` call answers every crate/bin/example question. `--no-deps`
# keeps it to workspace members, which is the only thing a workflow can name.
$metadataJson = & cargo metadata --no-deps --format-version 1 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Host "::error::cargo metadata failed, so workflow references cannot be resolved" 
    exit 2
}
$metadata = $metadataJson | ConvertFrom-Json

$packages = @($metadata.packages | ForEach-Object { $_.name })
$bins = @()
$examples = @()
foreach ($package in $metadata.packages) {
    foreach ($target in $package.targets) {
        if ($target.kind -contains 'bin') { $bins += $target.name }
        elseif ($target.kind -contains 'example') { $examples += $target.name }
    }
}

# Each rule is (regex, what the captured name must resolve against, a resolver).
# Keeping them in one table means adding a fourth kind of reference is a row,
# not another copy of the reporting loop below.
#
# CargoOnly restricts a rule to lines that actually invoke cargo. Without it,
# `-p` is far too greedy to be used as a signal on its own: `mkdir -p dist` in a
# packaging step matched the package rule and reported `dist` as a missing
# crate. The flag is not on the tools rule because `./tools/x.ps1` is
# unambiguous wherever it appears.
$rules = @(
    @{
        Name      = 'package'
        Pattern   = '(?:^|\s)(?:-p|--package)\s+([^\s]+)'
        CargoOnly = $true
        Exists    = { param($n) $packages -contains $n }
        Detail    = { param($n) "no such workspace package" }
    },
    @{
        Name      = 'bin'
        Pattern   = '--bin\s+([^\s]+)'
        CargoOnly = $true
        Exists    = { param($n) $bins -contains $n }
        Detail    = { param($n) "no bin target with this name in any workspace package" }
    },
    @{
        Name      = 'example'
        Pattern   = '--example\s+([^\s]+)'
        CargoOnly = $true
        Exists    = { param($n) $examples -contains $n }
        Detail    = { param($n) "no example target with this name in any workspace package" }
    },
    @{
        Name      = 'tools script'
        Pattern   = '\./tools/([A-Za-z0-9._-]+)'
        CargoOnly = $false
        Exists    = { param($n) Test-Path -LiteralPath (Join-Path $repoRoot "tools\$n") }
        Detail    = { param($n) "no such file under tools/" }
    }
)

$failures = New-Object System.Collections.Generic.List[string]
$checked = 0
$workflows = @(Get-ChildItem -LiteralPath $WorkflowDirectory -Filter '*.yml' -File)

foreach ($workflow in $workflows) {
    $lines = [System.IO.File]::ReadAllLines($workflow.FullName)

    # Rejoin shell line-continuations before matching. A `run:` block splits one
    # command across physical lines with a trailing backslash, and the release
    # workflows here do exactly that:
    #
    #     cargo build --release --locked \
    #       --target "${{ matrix.target }}" \
    #       -p windows-placement-probe --bin placement-probe
    #
    # Matching physically would test the CargoOnly rules against a line with no
    # `cargo` on it and skip all six references in that file -- the checker
    # would report success having never looked. Each logical line keeps the
    # physical line its command began on, so a failure still points somewhere.
    $logicalLines = New-Object System.Collections.Generic.List[object]
    $index = 0
    while ($index -lt $lines.Count) {
        $startLine = $index + 1
        $text = $lines[$index]
        while ($text -match '\\\s*$' -and ($index + 1) -lt $lines.Count) {
            $text = ($text -replace '\\\s*$', ' ') + $lines[$index + 1].Trim()
            $index++
        }
        $logicalLines.Add([pscustomobject]@{ Text = $text; StartLine = $startLine })
        $index++
    }

    foreach ($logical in $logicalLines) {
        $line = $logical.Text
        foreach ($rule in $rules) {
            if ($rule.CargoOnly -and $line -notmatch '\bcargo\b') { continue }

            foreach ($match in [regex]::Matches($line, $rule.Pattern)) {
                $name = $match.Groups[1].Value.Trim('"', "'")

                # A value the workflow computes at run time cannot be resolved
                # here, and guessing would produce false failures. Two shapes:
                # a GitHub expression, and a shell variable such as the
                # `cargo publish -p "$CRATE_NAME"` the release workflow uses.
                if ($name -match '\$\{\{' -or $name -match '\$') { continue }

                $checked++
                if (-not (& $rule.Exists $name)) {
                    $detail = & $rule.Detail $name
                    $failures.Add(("{0}:{1}: {2} '{3}' -- {4}" -f $workflow.Name, $logical.StartLine, $rule.Name, $name, $detail))
                }
            }
        }
    }
}

if ($failures.Count -gt 0) {
    foreach ($failure in $failures) {
        Write-Host "::error::$failure"
    }
    Write-Host ""
    Write-Host "$($failures.Count) workflow reference(s) do not resolve."
    Write-Host "A workflow naming something that does not exist fails on a runner, not here,"
    Write-Host "so fix the reference or restore what it names before pushing."
    exit 1
}

Write-Host "Workflow references resolve: $checked reference(s) across $($workflows.Count) workflow file(s)."
exit 0
