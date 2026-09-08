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

    Only markers written by someone whose repository permission is `admin` or
    `write` are honoured, checked against the collaborators permission endpoint
    rather than inferred from `author_association` -- GitHub reports
    COLLABORATOR for a read-only collaborator too, so that field is weaker than
    this control claims to be. This repository is public, so anyone who can
    comment could otherwise retire a finding that has no other state anywhere.
    The check fails closed, and markers that were not honoured are counted and
    reported rather than silently dropped -- separately by cause, because "the
    author has no write access" and "this account cannot query permissions at
    all" send a reader to look in different places.

    Two consequences worth knowing before using -MarkProcessed:

    - Running the scan from an account WITHOUT push access to this repository
      makes every marker unverifiable (the endpoint 403s for every login), so
      every marked review is re-reported as outstanding.
    - Posting a marker from a workflow using GITHUB_TOKEN writes it as
      github-actions[bot], whose permission reads `none`, so that marker can
      never be honoured. Measured, not assumed. Mark from a real account.

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

# Walk a property chain over API-supplied data, yielding $null rather than
# throwing when any link is absent or null.
#
# Needed because `Set-StrictMode -Version Latest` turns `$x.a.b` into a
# TERMINATING error the moment `a` is null, and several fields these queries
# return are nullable BY SCHEMA rather than by accident: GraphQL `author` and
# `pullRequestReview` are null for a deleted account, REST `user` likewise, and
# `submitted_at` is null on a PENDING review. Any one of those would leave this
# script through an unhandled throw -- which exits 1, the code that means
# "there are findings". A contributor deleting their GitHub account would
# silently turn this tool into a false positive.
#
# Verified on both hosts that all of these throw under StrictMode without it:
# `$null.login`, `@()[0]`, and `[datetime]$null`.
function Get-Path {
    param($Object, [string[]] $Names)
    $current = $Object
    foreach ($name in $Names) {
        if ($null -eq $current) { return $null }
        $property = $current.PSObject.Properties[$name]
        if ($null -eq $property) { return $null }
        $current = $property.Value
    }
    return $current
}

# `??` is PowerShell 7 only, and these tools run on 5.1 too.
function Get-Text {
    param($Value)
    if ($null -eq $Value) { return '' }
    return [string]$Value
}

Write-Report "scanning pull request #$Pr" -Level heading

$reviews = Invoke-GitHubJson @('api', "repos/$script:Owner/$script:Name/pulls/$Pr/reviews?per_page=100", '--paginate')
$issueComments = Invoke-GitHubJson @('api', "repos/$script:Owner/$script:Name/issues/$Pr/comments?per_page=100", '--paginate')

# Whether this author may retire a finding, by ACTUAL repository permission.
#
# `author_association` is the cheap answer and it is the wrong one. GitHub sets
# `COLLABORATOR` for anyone *invited to collaborate*, with no permission
# qualifier -- a collaborator with `read` or `triage` gets `COLLABORATOR` too --
# so trusting that value would enforce something weaker than the control claims.
# The permission endpoint answers the question actually being asked.
#
# One call per DISTINCT author who posted a marker, cached, and markers are
# rare, so this is a call or two per scan rather than one per comment.
#
# **Fails closed.** Anything other than a confirmed `admin` or `write` -- a 404
# because the author is not a collaborator, a 403 because the caller running
# this scan lacks push access and may not query permissions, a network failure
# -- leaves the marker unhonoured. That direction is deliberate: an unhonoured
# marker over-reports a finding that was in fact handled, which is visible and
# recoverable, while wrongly honouring one silently deletes the only record that
# a finding was never read.
# Three outcomes, not two, because two of them have different causes and only one
# of them is a statement about the author:
#
#   'allowed'      the endpoint answered `admin` or `write`.
#   'denied'       the endpoint answered, and it was `read` or `none`. Measured:
#                  a plain non-collaborator returns exit 0 with `read`, and a bot
#                  account returns exit 0 with `none` -- neither is an error.
#   'unverifiable' the call failed. The endpoint requires the CALLER to have push
#                  access, so an account without it gets a flat 403 for every
#                  login, including the maintainer who wrote the markers.
#
# Collapsing the last two was a defect of exactly the kind this whole tool exists
# to prevent: the run reported "author lacks write access" in a case where it had
# established nothing about the author, and the reader would go and check the
# author's role rather than their own token.
#
# All three still fail closed -- only 'allowed' honours a marker -- because
# over-reporting a handled finding is visible and recoverable, while wrongly
# honouring one silently deletes the only record that a finding was never read.
$script:PermissionCache = @{}
function Get-RetireAuthority {
    param([string] $Login)
    # No author at all (a deleted account leaves `user` null). Nothing to
    # attribute the marker to, which is itself a decided answer rather than an
    # unanswerable one.
    if (-not $Login) { return 'denied' }
    if ($script:PermissionCache.ContainsKey($Login)) { return $script:PermissionCache[$Login] }

    # Deliberately NOT through Invoke-GitHubJson: a non-zero exit here is an
    # ordinary answer rather than a broken instrument, so it must not exit 2.
    $text = Invoke-Native {
        gh api "repos/$script:Owner/$script:Name/collaborators/$Login/permission" --jq '.permission'
    }
    $code = Get-LastExitCode

    $authority = if ($null -eq $code -or $code -ne 0) {
        'unverifiable'
    }
    elseif (@('admin', 'write') -contains (Get-Text ($text -join '')).Trim()) {
        'allowed'
    }
    else {
        'denied'
    }

    $script:PermissionCache[$Login] = $authority
    return $authority
}

# Reviews already recorded as processed, by marker.
#
# This repository is public, so anyone able to comment on a pull request can post
# a marker; and because a suppressed-only review has no state anywhere else --
# which is the whole reason this tool exists -- an unauthorised marker would
# permanently and silently delete the only record that a finding was never read.
# The counter would simply report a smaller number, with nothing to indicate why.
$processed = @{}
$deniedMarkers = 0
$unverifiableMarkers = 0
foreach ($c in $issueComments) {
    $markers = [regex]::Matches((Get-Text $c.body), '<!--\s*copilot-review-processed:\s*(\d+)\s*-->')
    if ($markers.Count -eq 0) { continue }

    $login = Get-Text (Get-Path $c @('user', 'login'))
    # Counted by CAUSE rather than lumped together, and never dropped in silence:
    # a denied marker says something about its author, an unverifiable one says
    # something about the account running this scan, and telling a reader the
    # wrong one sends them to look in the wrong place.
    switch (Get-RetireAuthority $login) {
        'allowed' {
            foreach ($m in $markers) { $processed[[long]$m.Groups[1].Value] = $true }
        }
        'denied' { $deniedMarkers += $markers.Count }
        default { $unverifiableMarkers += $markers.Count }
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
        # A thread with no comments is not a shape this query should produce, but
        # indexing an empty array is an error rather than $null under StrictMode,
        # so it is checked rather than assumed.
        $comments = Get-Path $t @('comments', 'nodes')
        if ($null -eq $comments -or @($comments).Count -eq 0) { continue }
        $c = @($comments)[0]

        $login = Get-Path $c @('author', 'login')
        if ((Get-Text $login) -notmatch '[Cc]opilot') { continue }

        # Null for a deleted review; without it there is nothing to attribute the
        # thread to, so the thread is skipped rather than attributed to review 0.
        $reviewId = Get-Path $c @('pullRequestReview', 'databaseId')
        if ($null -eq $reviewId) { continue }

        $threads += [pscustomobject]@{
            Review     = [long]$reviewId
            IsResolved = $t.isResolved
            IsOutdated = $t.isOutdated
            Path       = $t.path
            Line       = $t.line
            Body       = ((Get-Text (Get-Path $c @('body'))) -replace '\s+', ' ')
        }
    }
    $cursor = if ($page.pageInfo.hasNextPage) { $page.pageInfo.endCursor } else { $null }
} while ($cursor)

$copilotReviews = @($reviews | Where-Object {
        (Get-Text (Get-Path $_ @('user', 'login'))) -match '[Cc]opilot'
    })

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
    # Null on a PENDING review, and `[datetime]$null` is an error rather than a
    # zero date, so the absence is rendered rather than cast.
    $submitted = Get-Path $r @('submitted_at')
    $when = if ($submitted) { ([datetime]$submitted).ToString('yyyy-MM-dd HH:mm') } else { 'pending         ' }

    $suppressedOnly += [pscustomobject]@{
        Id         = [long]$r.id
        When       = $when
        Suppressed = $count
        Inline     = @($threads | Where-Object { $_.Review -eq [long]$r.id }).Count
    }
}

Write-Report ''
Write-Report "Copilot reviews:            $($copilotReviews.Count)"
Write-Report "unresolved threads:         $($openThreads.Count)  ($($current.Count) current, $($outdated.Count) outdated)"
Write-Report "reviews with suppressed:    $(@($copilotReviews | Where-Object { (Get-SuppressedCount (Get-Text $_.body)) -gt 0 }).Count)"
Write-Report "  of those, unprocessed:    $($suppressedOnly.Count)"
if ($deniedMarkers -gt 0) {
    Write-Report "markers not honoured:       $deniedMarkers (author has no write access here)" -Level warn
}
if ($unverifiableMarkers -gt 0) {
    # Says whose problem it is. This fires when the ACCOUNT RUNNING THE SCAN
    # cannot query collaborator permissions, which is a 403 for every login it
    # asks about -- including a maintainer whose markers are perfectly valid --
    # so pointing at the author would send the reader to check the wrong thing.
    Write-Report "markers unverifiable:       $unverifiableMarkers" -Level warn
    Write-Report "  This account could not query collaborator permissions, so no marker" -Level warn
    Write-Report "  could be confirmed and every marked review is re-reported above. That" -Level warn
    Write-Report "  is this token's push access, not a statement about who wrote them." -Level warn
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
