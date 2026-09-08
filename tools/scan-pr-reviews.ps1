# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Report which Copilot reviews on a pull request still need attention, and
    record the ones that have been dealt with.

.DESCRIPTION
    A long-lived pull request accumulates hundreds of Copilot reviews, and
    nothing in the GitHub UI says which have been dealt with. Judging by eye
    does not scale, and judging by "did a commit follow it" is guesswork. This
    reports the two signals that are real, and adds the one GitHub does not.

    A review carries findings in one of two shapes, and they need different
    treatment because GitHub models only one of them:

    INLINE COMMENTS become review threads, which can be RESOLVED. That flag is
    durable, visible in the UI, and queryable, so it is the tag for this shape
    -- there is nothing to invent. A thread also reports `isOutdated`, meaning
    the line it was anchored to has since changed; those are reported
    separately, because an outdated finding is usually one that was fixed and
    never resolved rather than one still waiting.

    SUPPRESSED COMMENTS exist only as prose inside the review body's `<details>`
    block. They create no thread, so there is nothing to resolve; and a review
    is not a reactable object either -- `POST /pulls/{n}/reviews/{id}/reactions`
    is 404, while the same call on an inline comment succeeds. A review whose
    findings were ALL suppressed therefore has no state anywhere saying it was
    read, which is exactly the gap this script closes.

    For those, `-MarkProcessed` posts a pull-request comment carrying a marker:

        <!-- copilot-review-processed: 5136043258 -->

    The marker is an HTML comment, so it does not render, and it lives on the
    pull request rather than in a file or a session, which is what makes it
    survive a new machine, a new contributor, and a new agent session. A later
    run of this script reads those markers back and stops reporting the review.

    Only markers written by someone with write access (author_association OWNER,
    MEMBER or COLLABORATOR) are honoured. This repository is public, so anyone who
    can comment could otherwise retire a finding that has no other state anywhere.
    Markers from other authors are counted and reported, not silently dropped.

.OUTPUTS
    Exit code 0 when nothing is outstanding, 1 when there are findings, and 2 when
    the tool could not run (gh unauthenticated, no such pull request, the marker
    could not be posted). 1 and 2 are kept distinct so a caller can tell a finding
    from a broken instrument.

.PARAMETER Pr
    The pull request number.

.PARAMETER MarkProcessed
    One or more review ids to record as processed. Posts a single comment
    carrying a marker for each, with the summary as its visible text.

.PARAMETER Summary
    The visible text of the marker comment. Required with -MarkProcessed:
    a marker with no account of what was done is a claim with no evidence.

.PARAMETER IncludeOutdated
    Also list unresolved threads whose anchor line has since changed.

.EXAMPLE
    .\tools\scan-pr-reviews.ps1 -Pr 56

.EXAMPLE
    .\tools\scan-pr-reviews.ps1 -Pr 56 -MarkProcessed 5136043258 `
        -Summary 'Both suppressed findings were measured and refuted; see commit abc1234.'
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][int] $Pr,
    [long[]] $MarkProcessed,
    [string] $Summary,
    [switch] $IncludeOutdated
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'common.ps1')

# Exit codes, kept distinct on purpose. `1` is a FINDING -- the scan ran and
# there is outstanding work -- while `2` is a BROKEN INSTRUMENT: `gh` is not
# authenticated, the network is down, the pull request does not exist, the
# marker could not be posted. A caller gating on the exit code has to be able to
# tell those apart, or "no findings" and "the tool never ran" look identical,
# which is the same instrument-versus-finding separation `run-numa-spikes.ps1`
# and `run-sabotage.ps1` already make. Every error path below leaves through
# `Exit-Broken` rather than through a bare `throw`, because an unhandled throw
# from a script also exits 1 and would collide with the finding code.
$script:ExitFindings = 1
$script:ExitBroken = 2

function Exit-Broken {
    param([Parameter(Mandatory = $true)][string] $Message)
    [Console]::Error.WriteLine($Message)
    exit $script:ExitBroken
}

# `$LASTEXITCODE` is UNSET until a native command has run in the session, and
# under `Set-StrictMode -Version Latest` reading an unset variable throws. That
# is not a hypothetical: with `gh` absent from PATH, the call below fails with
# CommandNotFoundException before ever setting it, and the script then died on
# the StrictMode violation rather than on the missing tool -- exiting 1, the
# code that means "there are findings". Reading it through `Get-Variable`
# removes the landmine wherever the code path reaches it.
function Get-LastExitCode {
    $variable = Get-Variable -Name LASTEXITCODE -Scope Global -ErrorAction SilentlyContinue
    if ($null -eq $variable -or $null -eq $variable.Value) { return $null }
    return [int]$variable.Value
}

# `gh` is the whole instrument here, so its absence is checked once, up front,
# rather than being discovered as a confusing symptom further in. Without this,
# a machine without the CLI reported either a StrictMode error about
# `$LASTEXITCODE` or a JSON parse failure -- both describing the wreckage rather
# than the cause, and both exiting 1 as though the scan had found something.
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Exit-Broken @'
gh was not found on PATH, so this tool cannot read the pull request at all.
Install the GitHub CLI (https://cli.github.com) and run `gh auth login`.
'@
}

$script:Owner = 'MikeGrier'
$script:Name = 'windows-threadpool-sys'

# The single output sink, per the repository's one-output-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'detail', 'heading', 'warn')][string] $Level = 'info'
    )
    switch ($Level) {
        'detail' { Write-Host $Message -ForegroundColor DarkGray }
        'heading' { Write-Host $Message -ForegroundColor Cyan }
        'warn' { Write-Host $Message -ForegroundColor Yellow }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Invoke-GitHubJson {
    param([string[]] $Arguments)
    $text = Invoke-Native { gh @Arguments }
    $code = Get-LastExitCode
    if ($null -eq $code -or $code -ne 0) {
        Exit-Broken "gh $($Arguments -join ' ') failed (exit $code): $($text -join ' ')"
    }
    try {
        return ($text -join "`n") | ConvertFrom-Json
    }
    catch {
        # Reached when gh succeeds but returns something that is not JSON -- a
        # proxy's HTML error page is the realistic case. Still the instrument,
        # not a finding.
        Exit-Broken "gh $($Arguments -join ' ') returned unparsable output: $($_.Exception.Message)"
    }
}

# --- marking -----------------------------------------------------------------

if ($MarkProcessed) {
    if (-not $Summary) {
        Exit-Broken 'Summary is required with -MarkProcessed: a marker with no account of what was done is a claim with no evidence.'
    }
    $lines = @($Summary, '')
    foreach ($id in $MarkProcessed) { $lines += "<!-- copilot-review-processed: $id -->" }
    $file = Join-Path ([System.IO.Path]::GetTempPath()) ("mark-" + [guid]::NewGuid().ToString('N') + '.md')
    [System.IO.File]::WriteAllText($file, ($lines -join "`n"), [System.Text.UTF8Encoding]::new($false))
    try {
        $url = Invoke-Native { gh pr comment $Pr --repo "$script:Owner/$script:Name" --body-file $file }
        $code = Get-LastExitCode
        if ($null -eq $code -or $code -ne 0) {
            Exit-Broken "posting the marker comment failed (exit $code): $($url -join ' ')"
        }
        Write-Report "marked processed: $($MarkProcessed -join ', ')"
        Write-Report ($url -join ' ') -Level detail
    }
    finally { Remove-Item $file -ErrorAction SilentlyContinue }
    exit 0
}

# --- scanning ----------------------------------------------------------------

# `??` is PowerShell 7 only, and these tools run on 5.1 too.
function Get-Text {
    param($Value)
    if ($null -eq $Value) { return '' }
    return [string]$Value
}

Write-Report "scanning pull request #$Pr" -Level heading

$reviews = Invoke-GitHubJson @('api', "repos/$script:Owner/$script:Name/pulls/$Pr/reviews?per_page=100", '--paginate')
$issueComments = Invoke-GitHubJson @('api', "repos/$script:Owner/$script:Name/issues/$Pr/comments?per_page=100", '--paginate')

# Reviews already recorded as processed, by marker.
#
# **Only markers from someone with write access are honoured.** This repository
# is public, so anyone able to comment on a pull request can post a marker; and
# because a suppressed-only review has no state anywhere else -- which is the
# whole reason this tool exists -- an unauthenticated marker would permanently
# and silently delete the only record that a finding was never read. The counter
# below would simply report a smaller number, with nothing to indicate why.
#
# `author_association` comes back on every comment from the same request, so
# this costs no extra call. GitHub sets it per comment from the author's
# relationship to the repository at the time of writing: OWNER, MEMBER and
# COLLABORATOR are the ones that imply write access. CONTRIBUTOR means only
# "has had a pull request merged", and NONE is any passer-by; neither is
# sufficient to retire a finding.
$trustedAssociations = @('OWNER', 'MEMBER', 'COLLABORATOR')
$processed = @{}
$ignoredMarkers = 0
foreach ($c in $issueComments) {
    $markers = [regex]::Matches((Get-Text $c.body), '<!--\s*copilot-review-processed:\s*(\d+)\s*-->')
    if ($markers.Count -eq 0) { continue }
    if ($trustedAssociations -notcontains (Get-Text $c.author_association)) {
        # Counted and reported rather than dropped in silence: a marker from an
        # untrusted author is either an honest mistake or an attempt to retire a
        # finding, and both are worth seeing.
        $ignoredMarkers += $markers.Count
        continue
    }
    foreach ($m in $markers) {
        $processed[[long]$m.Groups[1].Value] = $true
    }
}

# Unresolved threads, which is the authoritative state for inline findings.
$query = @'
query($owner:String!, $name:String!, $pr:Int!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    pullRequest(number:$pr) {
      reviewThreads(first:100, after:$cursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          isResolved isOutdated path line
          comments(first:1) {
            nodes { body author { login } pullRequestReview { databaseId } }
          }
        }
      }
    }
  }
}
'@

$threads = @()
$cursor = $null
do {
    $arguments = @('api', 'graphql', '-f', "query=$query",
        '-F', "owner=$script:Owner", '-F', "name=$script:Name", '-F', "pr=$Pr")
    if ($cursor) { $arguments += @('-F', "cursor=$cursor") }
    $page = (Invoke-GitHubJson $arguments).data.repository.pullRequest.reviewThreads
    foreach ($t in $page.nodes) {
        $c = $t.comments.nodes[0]
        if ($c.author.login -notmatch '[Cc]opilot') { continue }
        $threads += [pscustomobject]@{
            Review     = [long]$c.pullRequestReview.databaseId
            IsResolved = $t.isResolved
            IsOutdated = $t.isOutdated
            Path       = $t.path
            Line       = $t.line
            Body       = ($c.body -replace '\s+', ' ')
        }
    }
    $cursor = if ($page.pageInfo.hasNextPage) { $page.pageInfo.endCursor } else { $null }
} while ($cursor)

$copilotReviews = @($reviews | Where-Object { $_.user.login -match '[Cc]opilot' })

# A review's suppressed count is only in its body, as rendered prose.
function Get-SuppressedCount {
    param([string] $Body)
    if ($Body -match 'Suppressed comments?\s*\((\d+)\)') { return [int]$Matches[1] }
    return 0
}

$openThreads = @($threads | Where-Object { -not $_.IsResolved })
$current = @($openThreads | Where-Object { -not $_.IsOutdated })
$outdated = @($openThreads | Where-Object { $_.IsOutdated })

$suppressedOnly = @()
foreach ($r in $copilotReviews) {
    $count = Get-SuppressedCount (Get-Text $r.body)
    if ($count -eq 0) { continue }
    if ($processed.ContainsKey([long]$r.id)) { continue }
    # A review whose inline threads are all resolved may still carry suppressed
    # findings nobody read, so this is judged on the marker alone.
    $suppressedOnly += [pscustomobject]@{
        Id         = [long]$r.id
        When       = ([datetime]$r.submitted_at).ToString('yyyy-MM-dd HH:mm')
        Suppressed = $count
        Inline     = @($threads | Where-Object { $_.Review -eq [long]$r.id }).Count
    }
}

Write-Report ''
Write-Report "Copilot reviews:            $($copilotReviews.Count)"
Write-Report "unresolved threads:         $($openThreads.Count)  ($($current.Count) current, $($outdated.Count) outdated)"
Write-Report "reviews with suppressed:    $(@($copilotReviews | Where-Object { (Get-SuppressedCount (Get-Text $_.body)) -gt 0 }).Count)"
Write-Report "  of those, unprocessed:    $($suppressedOnly.Count)"
if ($ignoredMarkers -gt 0) {
    Write-Report "ignored markers:            $ignoredMarkers (author lacks write access)" -Level warn
}
Write-Report ''

Write-Report '=== unresolved threads on current lines ===' -Level heading
if ($current.Count -eq 0) { Write-Report '  none' -Level detail }
foreach ($t in ($current | Sort-Object Path, Line)) {
    Write-Report ("  review {0}  {1}:{2}" -f $t.Review, $t.Path, $t.Line)
    Write-Report ('      ' + $t.Body.Substring(0, [Math]::Min(200, $t.Body.Length))) -Level detail
}

if ($IncludeOutdated) {
    Write-Report ''
    Write-Report '=== unresolved threads whose anchor line has changed ===' -Level heading
    Write-Report '  Usually fixed and never resolved; resolve them to clear this list.' -Level detail
    foreach ($t in ($outdated | Sort-Object Path, Line)) {
        Write-Report ("  review {0}  {1}:{2}" -f $t.Review, $t.Path, $t.Line)
    }
}

Write-Report ''
Write-Report '=== reviews with suppressed comments and no processed marker ===' -Level heading
Write-Report '  Suppressed findings create no thread, so nothing else records that they' -Level detail
Write-Report '  were read. Mark one with -MarkProcessed once it has been dealt with.' -Level detail
if ($suppressedOnly.Count -eq 0) { Write-Report '  none' -Level detail }
foreach ($r in ($suppressedOnly | Sort-Object When)) {
    Write-Report ("  {0}  {1}  suppressed={2}  inline={3}" -f $r.Id, $r.When, $r.Suppressed, $r.Inline)
}

Write-Report ''
if ($current.Count -gt 0 -or $suppressedOnly.Count -gt 0) {
    Write-Report 'Outstanding items above.' -Level warn
    exit $script:ExitFindings
}
Write-Report 'Nothing outstanding.'
exit 0
