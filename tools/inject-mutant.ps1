# Copyright (c) Mike Grier.
#
# Line-targeted mutant injection, for confirming a test actually kills a mutant.
#
# Deliberately NOT a string replace: `        state.reserved += 1;` occurs four
# times in one file here, and replacing all of them mutated a tested line while
# reporting an untested one as caught -- inverting the conclusion. cargo-mutants
# names a file, a line and a column, so use them.
#
# Judges by exit code, and treats a hang as caught, for the reasons in
# tools/README-sabotage.md.

param(
    [Parameter(Mandatory = $true)][string] $File,
    [Parameter(Mandatory = $true)][int[]] $Line,
    [Parameter(Mandatory = $true)][string] $Find,
    [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Replace,
    [string] $TestFilter = '',
    [string] $Package = 'windows-file-watcher',
    [int] $TimeoutSeconds = 180
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$utf8NoBom = [System.Text.UTF8Encoding]::new($false)
$repo = (git rev-parse --show-toplevel).Replace('/', '\')
$path = Join-Path $repo $File

$original = Get-Content -LiteralPath $path
foreach ($n in $Line) {
    $index = $n - 1
    if ($original[$index] -notmatch [regex]::Escape($Find)) {
        Write-Host ("line {0} does not contain '{1}': {2}" -f $n, $Find, $original[$index].Trim()) -ForegroundColor Red
        exit 2
    }
}

foreach ($n in $Line) {
    $mutated = [System.Collections.ArrayList]::new($original)
    $index = $n - 1
    # Replace only the first occurrence on that one line.
    $pattern = [regex]::Escape($Find)
    $mutated[$index] = [regex]::Replace($mutated[$index], $pattern, $Replace.Replace('$', '$$'), 1)

    [System.IO.File]::WriteAllText($path, (($mutated -join "`n") + "`n"), $utf8NoBom)
    try {
        $args = @('test', '-p', $Package, '--locked')
        if ($TestFilter) { $args += $TestFilter }
        $out = Join-Path $env:TEMP 'mutline.txt'
        $proc = Start-Process -FilePath 'cargo' -ArgumentList $args -WorkingDirectory $repo `
            -PassThru -NoNewWindow -RedirectStandardOutput $out -RedirectStandardError "$out.err"
        if ($proc.WaitForExit($TimeoutSeconds * 1000)) {
            $verdict = if ($proc.ExitCode -eq 0) { '*** SURVIVED ***' } else { 'caught' }
            $detail = "exit $($proc.ExitCode)"
        }
        else {
            Get-CimInstance Win32_Process -Filter "ParentProcessId=$($proc.Id)" -ErrorAction SilentlyContinue |
                ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
            Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
            $verdict = 'caught'
            $detail = "HUNG past ${TimeoutSeconds}s"
        }
    }
    finally {
        [System.IO.File]::WriteAllText($path, (($original -join "`n") + "`n"), $utf8NoBom)
        if ((Get-Content -LiteralPath $path -Raw) -ne (($original -join "`n") + "`n")) {
            # Content compare is approximate across line-ending conventions, so
            # only warn; the authoritative check is the caller's `git status`.
            Write-Host "  (verify $File is restored)" -ForegroundColor DarkYellow
        }
    }

    $colour = if ($verdict -eq 'caught') { 'Green' } else { 'Red' }
    Write-Host ("{0}:{1} '{2}' -> '{3}'  {4} ({5})" -f $File, $n, $Find, $Replace, $verdict, $detail) -ForegroundColor $colour
}
