# Copyright (c) Mike Grier.
<#
    Shared support for the probation tier's results, dot-sourced by
    run-tier.ps1, publish-probation-results.ps1 and report-probation.ps1. Not a script.

    A probation test is one that has never run anywhere, so it starts on probation and is
    promoted to the long-haul tier once it has passed enough runs (DESIGN-NOTES.md,
    "long-haul-tier"). These functions turn a tier run's libtest output into outcomes, keep
    a record of each test's runs across runs, and report that record. The record is derived
    data and is published to a branch of its own, `probation-results`; the decision is in
    DESIGN-NOTES.md, "probation-results".

    The record, `results.json`:

        { "version": 1, "latestRun": "<id>", "latestAt": "<time>", "latestCommit": "<sha>",
          "tests": [ { "test": "<target source path>::<test path>",
              "runs": N, "passes": N, "streak": N, "last": "pass|fail",
              "firstAt": "<time>", "lastAt": "<time>", "lastCommit": "<sha>", "lastRun": "<id>" } ] }

    The tests are a list and not an object keyed by name: a test name is case-sensitive and two
    can differ only by case, which PowerShell's object properties and its ordered dictionaries
    both treat as one. In memory the tests are a dictionary with an ordinal comparer for that
    reason; on disk they are a list in ordinal order.

    A test is keyed by the source path of the target its executable was built from, relative
    to the workspace root, and its path inside it. cargo's own `Running` header names only a
    path relative to the package, which two crates share.
#>

. (Join-Path $PSScriptRoot 'common.ps1')

# What a test did in a run. Named once; these are what the record and the outcomes file hold.
$script:ProbationPass = 'pass'
$script:ProbationFail = 'fail'

# What the report calls a test. A candidate is one whose latest result is a pass and whose
# current run of passes has reached the threshold.
$script:StatusCandidate = 'candidate'
$script:StatusAccruing = 'accruing'
$script:StatusFailing = 'failing'
$script:StatusAbsent = 'not in the latest run'

# The version of the record's layout. Changing the layout is a breaking change for whoever
# holds a record in this form.
$script:ProbationRecordVersion = 1

# The executables a build produced, by file name, to the workspace-relative source path of
# the target each was built from. `BuildJsonPath` is `cargo test --no-run
# --message-format=json`'s stdout.
function Get-ProbationTargetMap {
    param([string] $BuildJsonPath, [string] $RepositoryRoot)
    $root = [System.IO.Path]::GetFullPath($RepositoryRoot).TrimEnd('\', '/')
    $map = @{}
    foreach ($line in [System.IO.File]::ReadAllLines($BuildJsonPath)) {
        if (-not $line.StartsWith('{')) { continue }
        $message = $null
        try { $message = $line | ConvertFrom-Json } catch { continue }
        if ($message.reason -ne 'compiler-artifact' -or -not $message.executable -or -not $message.profile.test) { continue }
        $source = [System.IO.Path]::GetFullPath([string]$message.target.src_path)
        if ($source.StartsWith($root + '\', [System.StringComparison]::OrdinalIgnoreCase)) { $source = $source.Substring($root.Length + 1) }
        $map[[System.IO.Path]::GetFileName([string]$message.executable)] = ($source -replace '\\', '/')
    }
    return $map
}

# What each test of a run did, from the capture of `cargo test ... -- --format json`
# (stdout and stderr to one file, so cargo's `Running` headers fall between the executables'
# events in the order they happened). Returns `Outcomes`, each `Key`, `Outcome` and `Note`,
# and `Problems`, the lines it could not read.
#
# A test that started and was never reported failed to finish -- its executable stopped
# under it, by a crash or an abort -- and is a failure, not an absence: counting it as
# nothing would let a test that kills its process look like one that was never run. A test
# of an executable that stopped BEFORE it started is not known to exist and is not counted.
function ConvertFrom-ProbationCapture {
    param([string] $CapturePath, [hashtable] $TargetMap)
    $outcomes = New-Object System.Collections.Generic.List[object]
    $problems = New-Object System.Collections.Generic.List[string]
    $pending = [ordered]@{}
    $target = $null

    $closeTarget = {
        foreach ($name in @($pending.Keys)) {
            $outcomes.Add([pscustomobject]@{ Key = "${target}::$name"; Outcome = $script:ProbationFail; Note = 'started and never reported an outcome: its executable stopped' })
        }
        $pending.Clear()
    }

    foreach ($line in [System.IO.File]::ReadAllLines($CapturePath)) {
        $header = [regex]::Match($line, '^\s*Running\s.*\((?<exe>[^()]+)\)\s*$')
        if ($header.Success) {
            & $closeTarget
            $file = [System.IO.Path]::GetFileName($header.Groups['exe'].Value)
            if (-not $TargetMap.ContainsKey($file)) { throw "The run reported an executable the build did not produce: $file" }
            $target = $TargetMap[$file]
            continue
        }
        if (-not $line.TrimStart().StartsWith('{')) { continue }
        $event = $null
        try { $event = $line | ConvertFrom-Json } catch { $problems.Add("not JSON: $line"); continue }
        if ($event.type -eq 'suite' -and ($event.event -eq 'ok' -or $event.event -eq 'failed')) { & $closeTarget; continue }
        if ($event.type -ne 'test') { continue }
        if ($null -eq $target) { throw "A test event came before any executable was named: $line" }
        $name = [string]$event.name
        switch ([string]$event.event) {
            'started' { $pending[$name] = $true }
            'ok' { $pending.Remove($name); $outcomes.Add([pscustomobject]@{ Key = "${target}::$name"; Outcome = $script:ProbationPass; Note = '' }) }
            'failed' {
                $pending.Remove($name)
                $note = if ($event.PSObject.Properties['stdout']) { [string]$event.stdout } else { '' }
                $outcomes.Add([pscustomobject]@{ Key = "${target}::$name"; Outcome = $script:ProbationFail; Note = $note })
            }
            'ignored' { $pending.Remove($name) }
        }
    }
    & $closeTarget # the end of the capture closes the last executable
    # ToArray and not @(): on an empty generic list `@()` throws "Argument types do not match".
    return [pscustomobject]@{ Outcomes = $outcomes.ToArray(); Problems = $problems.ToArray() }
}

# A dictionary that tells keys apart by their exact characters, which `[ordered]@{}` does not.
function New-ProbationTable {
    return New-Object System.Collections.Specialized.OrderedDictionary -ArgumentList ([System.StringComparer]::Ordinal)
}

# The keys of `Table` in ordinal order, so that a record is the same bytes whichever host or culture
# wrote it, and a report lists tests the same way.
function Get-ProbationOrdinalKeys {
    param($Table)
    $keys = [string[]]@($Table.Keys)
    [Array]::Sort($keys, [System.StringComparer]::Ordinal)
    return $keys
}

function New-ProbationRecord {
    return [ordered]@{
        version      = $script:ProbationRecordVersion
        latestRun    = ''
        latestAt     = ''
        latestCommit = ''
        tests        = (New-ProbationTable)
    }
}

# Reads `Path` (absent or empty is a record with nothing in it) into the shape above.
function Read-ProbationRecord {
    param([string] $Path)
    $record = New-ProbationRecord
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $record }
    $text = [System.IO.File]::ReadAllText($Path)
    if ($text.Trim().Length -eq 0) { return $record }
    return ConvertFrom-ProbationRecordText $text
}

# A time as text. PowerShell 7 reads an ISO 8601 string out of JSON as a DateTime, and written back
# it would be in the host's own format; the record holds the text it was given.
function ConvertTo-ProbationTimeText {
    param($Value)
    if ($Value -is [DateTime]) { return $Value.ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ', [System.Globalization.CultureInfo]::InvariantCulture) }
    return [string]$Value
}

function ConvertFrom-ProbationRecordText {
    param([string] $Text)
    $record = New-ProbationRecord
    $json = $Text | ConvertFrom-Json
    if ($json.version -ne $script:ProbationRecordVersion) {
        throw "The record is version $($json.version); this reads version $($script:ProbationRecordVersion)."
    }
    $record.latestRun = [string]$json.latestRun
    $record.latestAt = ConvertTo-ProbationTimeText $json.latestAt
    $record.latestCommit = [string]$json.latestCommit
    foreach ($v in $json.tests) {
        $record.tests[[string]$v.test] = [ordered]@{
            runs       = [int]$v.runs
            passes     = [int]$v.passes
            streak     = [int]$v.streak
            last       = [string]$v.last
            firstAt    = ConvertTo-ProbationTimeText $v.firstAt
            lastAt     = ConvertTo-ProbationTimeText $v.lastAt
            lastCommit = [string]$v.lastCommit
            lastRun    = [string]$v.lastRun
        }
    }
    return $record
}

# The record as text: UTF-8, LF, keys in ordinal order, so that two records with the same
# content are the same bytes and can be compared as text.
function ConvertTo-ProbationRecordText {
    param($Record)
    $tests = New-Object System.Collections.Generic.List[object]
    foreach ($key in (Get-ProbationOrdinalKeys $Record.tests)) {
        $t = $Record.tests[$key]
        $tests.Add([ordered]@{
                test = $key; runs = $t.runs; passes = $t.passes; streak = $t.streak; last = $t.last
                firstAt = $t.firstAt; lastAt = $t.lastAt; lastCommit = $t.lastCommit; lastRun = $t.lastRun
            })
    }
    $json = [ordered]@{
        version = $script:ProbationRecordVersion; latestRun = $Record.latestRun; latestAt = $Record.latestAt
        latestCommit = $Record.latestCommit; tests = $tests.ToArray()
    } | ConvertTo-Json -Depth 6
    return ($json -replace "`r`n", "`n") + "`n"
}

# `Record` with a run applied: each outcome adds a run, a pass adds a pass and a run of
# passes, a failure ends the run of passes. A test the run did not report is left as it is,
# and the record's latest run moves on, which is how the report knows a test was not in it.
# Applying a run the record already has as its latest changes nothing, so a publisher that
# retries from a fresh fetch cannot count a run twice. `Record` is changed and returned.
function Add-ProbationRun {
    param($Record, [object[]] $Outcomes, [string] $RunId, [string] $Commit, [string] $At)
    if ($Record.latestRun -eq $RunId) { return $Record }
    foreach ($outcome in $Outcomes) {
        if (-not $Record.tests.Contains($outcome.Key)) {
            $Record.tests[$outcome.Key] = [ordered]@{
                runs = 0; passes = 0; streak = 0; last = ''; firstAt = $At; lastAt = ''; lastCommit = ''; lastRun = ''
            }
        }
        $t = $Record.tests[$outcome.Key]
        $t.runs++
        if ($outcome.Outcome -eq $script:ProbationPass) { $t.passes++; $t.streak++; $t.last = $script:ProbationPass }
        else { $t.streak = 0; $t.last = $script:ProbationFail }
        $t.lastAt = $At
        $t.lastCommit = $Commit
        $t.lastRun = $RunId
    }
    $Record.latestRun = $RunId
    $Record.latestAt = $At
    $Record.latestCommit = $Commit
    return $Record
}

# One row per test: its counts and what it is, by `Threshold`, the passes in a row a test
# needs before it is a candidate for promotion. The threshold is a working position of the
# report and not a decision (DESIGN-NOTES.md, "probation-results"): it is a parameter so
# that what the first results suggest can be tried without a code change.
function Get-ProbationReport {
    param($Record, [int] $Threshold)
    if ($Threshold -lt 1) { throw "The threshold is a number of passes in a row and must be at least 1; got $Threshold." }
    foreach ($key in (Get-ProbationOrdinalKeys $Record.tests)) {
        $t = $Record.tests[$key]
        $status = if ($t.lastRun -ne $Record.latestRun) { $script:StatusAbsent }
        elseif ($t.last -eq $script:ProbationFail) { $script:StatusFailing }
        elseif ($t.streak -ge $Threshold) { $script:StatusCandidate }
        else { $script:StatusAccruing }
        [pscustomobject]@{ Test = $key; Runs = $t.runs; Passes = $t.passes; Streak = $t.streak; Last = $t.last; Status = $status }
    }
}

# The report as markdown lines.
function Format-ProbationReport {
    param($Record, [int] $Threshold)
    $rows = @(Get-ProbationReport -Record $Record -Threshold $Threshold)
    $lines = New-Object System.Collections.Generic.List[string]
    $lines.Add('### Probation tier')
    $lines.Add('')
    if ($rows.Count -eq 0) {
        $lines.Add('No test has been on probation.')
        return $lines
    }
    $latest = if ($Record.latestRun) { "run $($Record.latestRun) at $($Record.latestCommit)" } else { 'no run' }
    $lines.Add("$($rows.Count) test(s), latest $latest. A candidate has passed $Threshold run(s) in a row; the threshold is a working position, not a decision.")
    $lines.Add('')
    $lines.Add('| Test | Runs | Passes | In a row | Last | Status |')
    $lines.Add('|---|---|---|---|---|---|')
    foreach ($row in $rows) { $lines.Add("| ``$($row.Test)`` | $($row.Runs) | $($row.Passes) | $($row.Streak) | $($row.Last) | $($row.Status) |") }
    return $lines
}

# The branch `Branch` of `Remote` as the remote holds it now, asked of the remote itself and
# never of a local ref, which is whatever the last fetch left. The branch not existing is an
# answer (`Present` false, `Error` $null) and is not the remote being unreachable (`Error`
# set): a publisher that read an unreachable remote as an empty branch would write a root
# commit over history it could not see. Returns `Present`, `Error`, `Commit` and `Text`, the
# tip's results.json, $null when the tip has none.
function Get-ProbationBranch {
    param([string] $RepositoryRoot, [string] $Remote, [string] $Branch, [string] $File)
    $result = {
        param($present, $err, $commit, $text)
        [pscustomobject]@{ Present = $present; Error = $err; Commit = $commit; Text = $text }
    }
    # The full ref: ls-remote matches a pattern against the tail of each ref's name.
    $asked = Invoke-NativeSplit { git -C $RepositoryRoot ls-remote --exit-code --heads $Remote "refs/heads/$Branch" }
    # 2 is `--exit-code`'s "no matching ref", the one answer that means absent.
    if ($asked.ExitCode -eq 2) { return & $result $false $null $null $null }
    if ($asked.ExitCode -ne 0) {
        return & $result $false "git could not ask '$Remote' about '$Branch' (exit $($asked.ExitCode)): $($asked.Stderr)" $null $null
    }
    $ref = "refs/remotes/$Remote/$Branch"
    $fetched = Invoke-NativeSplit { git -C $RepositoryRoot fetch --quiet $Remote "+refs/heads/${Branch}:$ref" }
    if ($fetched.ExitCode -ne 0) {
        return & $result $false "git could not fetch '$Branch' from '$Remote' (exit $($fetched.ExitCode)): $($fetched.Stderr)" $null $null
    }
    $found = @(Invoke-NativeStdout { git -C $RepositoryRoot rev-parse --verify $ref })
    if ($LASTEXITCODE -ne 0 -or $found.Count -eq 0) { return & $result $false "git could not read '$ref' after fetching it" $null $null }
    $commit = ([string]$found[0]).Trim()
    return & $result $true $null $commit (Get-GitBlobUtf8 -RepositoryRoot $RepositoryRoot -Spec "${commit}:$File")
}

# The names of the branch and its file, once. Changing either changes what a clone fetches.
$script:ProbationBranch = 'probation-results'
$script:ProbationFile = 'results.json'
