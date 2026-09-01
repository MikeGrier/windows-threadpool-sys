# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Line-targeted mutant injection, for confirming a test actually kills a
    mutant that cargo-mutants reported as surviving.

.DESCRIPTION
    Deliberately NOT a whole-file string replace: `        state.reserved += 1;`
    occurs four times in one file here, and replacing all of them mutated a
    tested line while reporting an untested one as caught -- inverting the
    conclusion. cargo-mutants names a file, a line and a column, so use them.

    Three rules are encoded here because each was learned by getting it wrong.

    ONE OCCURRENCE, OR SAY WHICH. Targeting a line is not enough when the
    pattern appears on it twice: the tool would mutate whichever it reached
    first and report a verdict about the other. A line with more than one match
    is refused unless -Column names the one meant. An earlier version passed a
    replacement count to the STATIC [regex]::Replace overload, whose fourth
    parameter is RegexOptions -- so the `1` meant IgnoreCase, and every
    occurrence on the line was replaced. There is no static overload taking a
    count at all, which is why the replacement is now done by offset.

    THE BASELINE MUST BE GREEN FIRST. This judges by exit code, so an unrelated
    compile error or a pre-existing failure would be recorded as the mutant
    being "caught" -- a false clean bill of health for a test that does not
    exist. The unmodified suite runs once before the first mutation, and a red
    one aborts the run.

    ALL FEATURES, BY DEFAULT. A mutation inside `#[cfg(feature = "...")]` code
    is compiled out along with its tests when that feature is off, so the suite
    passes trivially and the mutant is reported as surviving. Measured on this
    workspace: 57 of 61 `windows-topology-sys` survivors and 147 of 247
    `windows-file-watcher` survivors were that artifact. The documented mutation
    workflow runs with all features, and so does this.

    A hang counts as caught, for the reasons in tools/README-sabotage.md: a
    missing wakeup does not fail a test, it stops one.

.PARAMETER File
    Repository-relative source file to mutate.

.PARAMETER Line
    One or more 1-based line numbers, each mutated and tested in turn.

.PARAMETER Column
    Optional 1-based column per line, as cargo-mutants reports it, naming which
    occurrence to replace when the line carries more than one. Supply either one
    column for every line, or none at all.

.PARAMETER Find
    Text to replace. Matched literally, never as a regex.

.PARAMETER Replace
    Literal replacement text. Empty removes the matched text.

.PARAMETER TestFilter
    Optional positional filter passed to `cargo test`.

.PARAMETER Package
    Crate to test.

.PARAMETER DefaultFeaturesOnly
    Test with default features instead of all of them. Off by default; read the
    feature note above before using it.

.PARAMETER TimeoutSeconds
    Bound on each test run. A run that exceeds it is killed and counted caught.

.OUTPUTS
    Exits non-zero if any mutant survived, or if the run could not be trusted.
#>

param(
    [Parameter(Mandatory = $true)][string] $File,
    [Parameter(Mandatory = $true)][int[]] $Line,
    [int[]] $Column = @(),
    [Parameter(Mandatory = $true)][string] $Find,
    [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Replace,
    [string] $TestFilter = '',
    [string] $Package = 'windows-file-watcher',
    [switch] $DefaultFeaturesOnly,
    [int] $TimeoutSeconds = 180
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)

# The single output sink. Every message this tool emits goes through here, so
# the destination and the formatting stay separable from the call sites that
# produce the content -- the repository's one-output-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'warn', 'bad')][string] $Level = 'info'
    )
    $colour = switch ($Level) {
        'good' { 'Green' }
        'warn' { 'DarkYellow' }
        'bad' { 'Red' }
        default { 'Gray' }
    }
    Write-Host $Message -ForegroundColor $colour
}

# Runs `cargo test` under a wall-clock bound and reports which of three outcomes
# occurred. A hang is distinct from a failure because it is what a lost-wakeup
# mutant looks like, and collapsing the two would hide that.
function Invoke-Suite {
    param([string] $Repository, [string[]] $CargoArgs, [int] $Seconds)

    $out = Join-Path $env:TEMP ('mutline-' + [guid]::NewGuid().ToString('N') + '.txt')
    $proc = Start-Process -FilePath 'cargo' -ArgumentList $CargoArgs -WorkingDirectory $Repository `
        -PassThru -NoNewWindow -RedirectStandardOutput $out -RedirectStandardError "$out.err"
    try {
        if ($proc.WaitForExit($Seconds * 1000)) {
            $outcome = if ($proc.ExitCode -eq 0) { 'passed' } else { 'failed' }
            return [pscustomobject]@{ Outcome = $outcome; Code = $proc.ExitCode }
        }
        Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" -ErrorAction SilentlyContinue |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        return [pscustomobject]@{ Outcome = 'hung'; Code = $null }
    }
    finally {
        Remove-Item -LiteralPath $out, "$out.err" -Force -ErrorAction SilentlyContinue
    }
}

$repo = (git rev-parse --show-toplevel).Replace('/', '\')
$path = Join-Path $repo $File

if ($Column.Count -ne 0 -and $Column.Count -ne $Line.Count) {
    Write-Report '-Column must name a column for every line, or be omitted entirely.' -Level bad
    exit 2
}

$cargoArgs = @('test', '-p', $Package, '--locked')
if (-not $DefaultFeaturesOnly) { $cargoArgs += '--all-features' }
if ($TestFilter) { $cargoArgs += $TestFilter }

$original = Get-Content -LiteralPath $path

# Resolve every target before touching anything, so a stale line number cannot
# leave the tree half-patched or produce a verdict about the wrong expression.
$targets = @()
for ($i = 0; $i -lt $Line.Count; $i++) {
    $n = $Line[$i]
    $index = $n - 1
    if ($index -lt 0 -or $index -ge $original.Count) {
        Write-Report ('line {0} is outside {1}, which has {2} lines' -f $n, $File, $original.Count) -Level bad
        exit 2
    }

    $hits = [regex]::Matches($original[$index], [regex]::Escape($Find))
    if ($hits.Count -eq 0) {
        Write-Report ("line {0} does not contain '{1}': {2}" -f $n, $Find, $original[$index].Trim()) -Level bad
        exit 2
    }

    $columns = ($hits | ForEach-Object { $_.Index + 1 }) -join ', '
    if ($Column.Count -ne 0) {
        # cargo-mutants reports 1-based columns; .NET match indices are 0-based.
        $wanted = $Column[$i] - 1
        $chosen = $hits | Where-Object { $_.Index -eq $wanted } | Select-Object -First 1
        if (-not $chosen) {
            Write-Report ("line {0} has no '{1}' at column {2}; it starts at column(s) {3}" -f `
                    $n, $Find, $Column[$i], $columns) -Level bad
            exit 2
        }
    }
    elseif ($hits.Count -gt 1) {
        # Refused rather than guessed. Mutating the wrong occurrence yields a
        # verdict about an expression nobody asked about, and it reads exactly
        # like a real result.
        Write-Report ("line {0} contains '{1}' {2} times (columns {3}); name one with -Column" -f `
                $n, $Find, $hits.Count, $columns) -Level bad
        exit 2
    }
    else {
        $chosen = $hits[0]
    }

    $targets += [pscustomobject]@{
        Line = $n; Index = $index; Start = $chosen.Index; Length = $chosen.Length
    }
}

# The baseline, before the first mutation. Judging a mutant by exit code is only
# sound if the unmodified tree exits zero; otherwise every mutant is "caught"
# for a reason that has nothing to do with the tests.
Write-Report 'Baseline: running the unmodified suite.'
$baseline = Invoke-Suite -Repository $repo -CargoArgs $cargoArgs -Seconds $TimeoutSeconds
if ($baseline.Outcome -ne 'passed') {
    Write-Report ('Baseline is NOT green ({0}); every verdict below would be meaningless. Fix the suite first.' -f `
            $baseline.Outcome) -Level bad
    exit 3
}
Write-Report 'Baseline is green. Injecting.'

$survivors = 0
foreach ($target in $targets) {
    $mutated = [System.Collections.ArrayList]::new($original)
    $text = [string] $mutated[$target.Index]
    $mutated[$target.Index] = $text.Remove($target.Start, $target.Length).Insert($target.Start, $Replace)

    try {
        # Inside the guarded region, so a write that throws part-way through --
        # leaving the file truncated -- still reaches the restoring `finally`.
        [System.IO.File]::WriteAllText($path, (($mutated -join "`n") + "`n"), $utf8NoBom)
        $run = Invoke-Suite -Repository $repo -CargoArgs $cargoArgs -Seconds $TimeoutSeconds
        $verdict = if ($run.Outcome -eq 'passed') { '*** SURVIVED ***' } else { 'caught' }
        $detail = if ($run.Outcome -eq 'hung') { "HUNG past ${TimeoutSeconds}s" } else { "exit $($run.Code)" }
    }
    finally {
        [System.IO.File]::WriteAllText($path, (($original -join "`n") + "`n"), $utf8NoBom)
        if ((Get-Content -LiteralPath $path -Raw) -ne (($original -join "`n") + "`n")) {
            # Content compare is approximate across line-ending conventions, so
            # only warn; the authoritative check is the caller's `git status`.
            Write-Report "  (verify $File is restored)" -Level warn
        }
    }

    $level = if ($verdict -eq 'caught') { 'good' } else { $survivors++; 'bad' }
    Write-Report ("{0}:{1} '{2}' -> '{3}'  {4} ({5})" -f `
            $File, $target.Line, $Find, $Replace, $verdict, $detail) -Level $level
}

if ($survivors -gt 0) { exit 1 }
exit 0
