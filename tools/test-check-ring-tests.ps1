# Copyright (c) Mike Grier.
<#
.SYNOPSIS
    Tests for [check-ring-tests.ps1](check-ring-tests.ps1). Exits 0 only if
    every case passes.

.DESCRIPTION
    WHY THIS EXISTS. The census decides which lib tests count as opening a
    kernel ring, and it had already missed one constructor once: a helper
    building its ring through `with_version_and_inventory` was classified as
    not opening one, so every test calling that helper bypassed the guard while
    it reported green. The fix widened the constructor pattern, and a PR #113
    review then pointed out that nothing would notice it narrowing again.

    So each case drives the real script, end to end, against a throwaway
    `src/` tree and inventory, and asserts what it recorded. Both directions
    are covered -- what must be counted and what must not -- because a guard
    that has stopped firing passes a one-directional test as happily as a
    correct one.

    WHY NOT PESTER. Same reason as test-run-sabotage.ps1: Windows PowerShell
    5.1 ships Pester 3, whose syntax differs incompatibly from Pester 5.
#>
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:Script = Join-Path $PSScriptRoot 'check-ring-tests.ps1'
$script:Failures = 0

# The single output sink, per the repository's one-sink rule.
function Write-Report {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyString()][string] $Message,
        [ValidateSet('info', 'good', 'bad', 'heading')][string] $Level = 'info'
    )
    switch ($Level) {
        'good' { Write-Host $Message -ForegroundColor Green }
        'bad' { Write-Host $Message -ForegroundColor Red }
        'heading' { Write-Host $Message -ForegroundColor Cyan }
        default { Write-Host $Message -ForegroundColor Gray }
    }
}

function Test-Case {
    param([string] $Name, [scriptblock] $Body)
    try {
        & $Body
        Write-Report "  PASS  $Name" -Level good
    }
    catch {
        $script:Failures++
        Write-Report "  FAIL  $Name" -Level bad
        Write-Report "        $($_.Exception.Message)"
    }
}

function Assert-Equal {
    param($Expected, $Actual, [string] $What)
    if ("$Expected" -ne "$Actual") {
        throw "$What -- expected '$Expected', got '$Actual'"
    }
}

function Assert-Contains {
    param([string[]] $Haystack, [string] $Needle, [string] $What)
    if ($Haystack -notcontains $Needle) {
        throw "$What -- '$Needle' missing from [$($Haystack -join ', ')]"
    }
}

function Assert-Absent {
    param([string[]] $Haystack, [string] $Needle, [string] $What)
    if ($Haystack -contains $Needle) {
        throw "$What -- '$Needle' must not be recorded, got [$($Haystack -join ', ')]"
    }
}

<#
.SYNOPSIS
    A throwaway crate root whose `src/<module>/tests.rs` files hold the given
    Rust text. Returns the root; the source tree is `<root>\src`.
#>
function New-Fixture {
    param([hashtable] $Modules)
    $root = Join-Path ([System.IO.Path]::GetTempPath()) ("crt-" + [guid]::NewGuid().ToString('N'))
    foreach ($module in $Modules.Keys) {
        $dir = Join-Path $root "src\$module"
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        # LF, as rustfmt leaves real sources: the script finds a function's end
        # at a column-0 `}` after a newline.
        $text = ($Modules[$module] -replace "`r`n", "`n")
        [System.IO.File]::WriteAllText((Join-Path $dir 'tests.rs'), $text)
    }
    return $root
}

function Invoke-Census {
    param([string] $Root, [switch] $Update)
    $arguments = @{
        SourceRoot    = (Join-Path $Root 'src')
        InventoryPath = (Join-Path $Root 'inventory.txt')
    }
    if ($Update) { $arguments.Update = $true }
    # `*>&1`, not `2>&1`: the script reports through `Write-Host`, which a plain
    # merge does not capture, so every assertion on the text would pass against
    # silence.
    $output = & $script:Script @arguments *>&1 | Out-String
    return [pscustomobject]@{ ExitCode = $LASTEXITCODE; Output = $output }
}

# What the census records for a fixture, read back from the inventory it wrote.
function Get-Recorded {
    param([string] $Root)
    $result = Invoke-Census -Root $Root -Update
    Assert-Equal 0 $result.ExitCode 'exit code of -Update'
    # The leading comma keeps a one-entry result an array: PowerShell unrolls a
    # returned array, and a bare string has no reliable `.Count` under StrictMode.
    return , @([System.IO.File]::ReadAllLines((Join-Path $Root 'inventory.txt')) |
            Where-Object { $_ -and -not $_.StartsWith('#') })
}

function Remove-Fixture {
    param([string] $Root)
    Remove-Item $Root -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Report "check-ring-tests.ps1 tests on $($PSVersionTable.PSVersion)" -Level heading

# --- What must be counted ----------------------------------------------------

Test-Case 'every constructor spelling in a test body is counted' {
    $root = New-Fixture @{ alpha = @'
#[test]
fn by_new() {
    let ring = IoRing::new(8, 8).expect("ring");
}

#[test]
fn by_with_version() {
    let ring = IoRing::with_version(version, 8, 8).expect("ring");
}

#[test]
fn by_with_inventory() {
    let ring = IoRing::with_inventory(8, 8).expect("ring");
}

#[test]
fn by_with_version_and_inventory() {
    let ring = IoRing::with_version_and_inventory(version, 8, 8).expect("ring");
}

#[test]
fn by_turbofish() {
    let ring = IoRing::<Vec<u8>>::with_inventory(16, 16).expect("ring");
}

#[test]
fn by_alias() {
    let ring = LaneRing::with_inventory(8, 8).expect("ring");
}

// `new` is matched only on names that denote IoRing, so these are the forms
// that slipped through while `new` was anchored to the literal `IoRing`
// (PR #113 review). The aliases are declared here and resolved by the script.
type LaneRing = IoRing<Vec<u8>>;
type WideLane = LaneRing;
type BareRing = IoRing;
use crate::ring::IoRing as Renamed;

#[test]
fn by_turbofish_new() {
    let ring = IoRing::<()>::new(8, 8).expect("ring");
}

#[test]
fn by_nested_turbofish_new() {
    let ring = IoRing::<Vec<u8>, ()>::new(8, 8).expect("ring");
}

#[test]
fn by_alias_new() {
    let ring = LaneRing::new(8, 8).expect("ring");
}

#[test]
fn by_bare_alias_new() {
    let ring = BareRing::new(8, 8).expect("ring");
}

#[test]
fn by_alias_of_alias_new() {
    let ring = WideLane::new(8, 8).expect("ring");
}

#[test]
fn by_renamed_import_new() {
    let ring = Renamed::new(8, 8).expect("ring");
}

#[test]
fn by_path_qualified_new() {
    let ring = crate::IoRing::new(8, 8).expect("ring");
}
'@ }
    try {
        $recorded = Get-Recorded $root
        foreach ($name in 'by_new', 'by_with_version', 'by_with_inventory',
            'by_with_version_and_inventory', 'by_turbofish', 'by_alias', 'by_turbofish_new',
            'by_nested_turbofish_new', 'by_alias_new', 'by_bare_alias_new', 'by_alias_of_alias_new',
            'by_renamed_import_new', 'by_path_qualified_new') {
            Assert-Contains $recorded "alpha::$name" 'constructor spelling'
        }
        Assert-Equal 13 $recorded.Count 'entries'
    }
    finally { Remove-Fixture $root }
}

# The motivating miss: a helper opening its ring through the one constructor
# the pattern had left out.
Test-Case 'a test reaching a ring through a helper is counted, for every constructor' {
    $root = New-Fixture @{ alpha = @'
fn make_new() -> IoRing {
    IoRing::new(8, 8).expect("ring")
}

fn make_versioned() -> IoRing {
    IoRing::with_version(version, 8, 8).expect("ring")
}

fn make_inventory() -> IoRing<Vec<u8>> {
    IoRing::with_inventory(8, 8).expect("ring")
}

fn make_versioned_inventory() -> IoRing<Vec<u8>> {
    IoRing::with_version_and_inventory(version, 8, 8).expect("ring")
}

type LaneRing = IoRing<Vec<u8>>;

fn make_alias_new() -> LaneRing {
    LaneRing::new(8, 8).expect("ring")
}

fn make_turbofish_new() -> IoRing {
    IoRing::<()>::new(8, 8).expect("ring")
}

#[test]
fn via_alias_new() {
    let ring = make_alias_new();
}

#[test]
fn via_turbofish_new() {
    let ring = make_turbofish_new();
}

#[test]
fn via_new() {
    let ring = make_new();
}

#[test]
fn via_versioned() {
    let ring = make_versioned();
}

#[test]
fn via_inventory() {
    let ring = make_inventory();
}

#[test]
fn via_versioned_inventory() {
    let ring = make_versioned_inventory();
}
'@ }
    try {
        $recorded = Get-Recorded $root
        foreach ($name in 'via_new', 'via_versioned', 'via_inventory', 'via_versioned_inventory',
            'via_alias_new', 'via_turbofish_new') {
            Assert-Contains $recorded "alpha::$name" 'helper indirection'
        }
        Assert-Equal 6 $recorded.Count 'entries (helpers themselves are not tests)'
    }
    finally { Remove-Fixture $root }
}

# Recorded on purpose, and pinned so the choice cannot change unnoticed in
# either direction: the script cannot resolve types, and anchoring to `IoRing`
# would miss a ring built through an alias. See the pattern's own comment.
Test-Case 'an unrelated type with a with_version constructor is counted, deliberately' {
    $root = New-Fixture @{ alpha = @'
#[test]
fn unrelated_with_version() {
    let other = Other::with_version(3);
}
'@ }
    try {
        $recorded = Get-Recorded $root
        Assert-Contains $recorded 'alpha::unrelated_with_version' 'over-inclusion is the safe direction'
    }
    finally { Remove-Fixture $root }
}

# --- What must not be counted ------------------------------------------------

Test-Case 'tests that construct no kernel ring are not counted' {
    $root = New-Fixture @{ alpha = @'
#[test]
fn ledger_only() {
    let mut a = Accounting::new();
}

#[test]
fn other_new() {
    let v: Vec<u8> = Vec::new();
}

#[test]
fn null_handle_ring() {
    let ring = IoRing::refused_by_the_kernel();
}

#[test]
fn longer_method_name() {
    let p = Policy::with_versioned(3);
    let q = Policy::with_inventory_size(3);
}

// Names that are NOT the ring type: a type that merely contains it, prose that
// says "IoRing as" without being a `use` rename, a longer name ending in
// IoRing, and a `new` method that only shares the alias's prefix.
type Rings = Vec<IoRing>;
// IoRing as a whole is not a rename.

#[test]
fn container_of_rings_new() {
    let rings = Rings::new();
}

#[test]
fn longer_type_name_new() {
    let r = MockIoRing::new(8, 8);
}

#[test]
fn word_after_prose_new() {
    let w = a::new();
}
'@ }
    try {
        $recorded = Get-Recorded $root
        Assert-Equal 0 $recorded.Count 'entries'
    }
    finally { Remove-Fixture $root }
}

# D-53's own regression: the body used to run to the next attribute, so a plain
# helper after the last test was swallowed into it.
Test-Case 'a ring-opening helper after the last test does not implicate it' {
    $root = New-Fixture @{ alpha = @'
#[test]
fn innocent() {
    let x = 1;
}

fn opens_a_ring() -> IoRing {
    IoRing::new(8, 8).expect("ring")
}
'@ }
    try {
        $recorded = Get-Recorded $root
        Assert-Absent $recorded 'alpha::innocent' 'a test that never calls the helper'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a call is matched on the whole helper name, not a suffix of another' {
    $root = New-Fixture @{ alpha = @'
fn ring() -> IoRing {
    IoRing::new(8, 8).expect("ring")
}

fn spring() -> u32 {
    3
}

#[test]
fn calls_spring() {
    let s = spring();
}

#[test]
fn calls_ring() {
    let r = ring();
}
'@ }
    try {
        $recorded = Get-Recorded $root
        Assert-Absent $recorded 'alpha::calls_spring' 'spring() is not ring()'
        Assert-Contains $recorded 'alpha::calls_ring' 'the real call'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'entries are qualified by the module directory' {
    $body = @'
#[test]
fn same_name() {
    let ring = IoRing::new(8, 8).expect("ring");
}
'@
    $root = New-Fixture @{ alpha = $body; beta = $body }
    try {
        $recorded = Get-Recorded $root
        Assert-Contains $recorded 'alpha::same_name' 'first module'
        Assert-Contains $recorded 'beta::same_name' 'second module'
    }
    finally { Remove-Fixture $root }
}

# --- Verify mode, which is what CI runs --------------------------------------

$opensOne = @'
#[test]
fn opens() {
    let ring = IoRing::new(8, 8).expect("ring");
}
'@

Test-Case 'an unchanged tree verifies clean' {
    $root = New-Fixture @{ alpha = $opensOne }
    try {
        $null = Get-Recorded $root
        $result = Invoke-Census -Root $root
        Assert-Equal 0 $result.ExitCode 'exit code'
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a new ring-opening test fails verification as ADDED' {
    $root = New-Fixture @{ alpha = $opensOne }
    try {
        $null = Get-Recorded $root
        $more = $opensOne + "`n#[test]`nfn also_opens() {`n    let r = IoRing::with_version_and_inventory(v, 8, 8);`n}`n"
        [System.IO.File]::WriteAllText((Join-Path $root 'src\alpha\tests.rs'), $more)
        $result = Invoke-Census -Root $root
        Assert-Equal 1 $result.ExitCode 'exit code'
        if ($result.Output -notmatch 'ADDED\s+alpha::also_opens') {
            throw "the added test must be named, got: $($result.Output)"
        }
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a removed ring-opening test fails verification as REMOVED' {
    $root = New-Fixture @{ alpha = $opensOne }
    try {
        $null = Get-Recorded $root
        [System.IO.File]::WriteAllText((Join-Path $root 'src\alpha\tests.rs'), "#[test]`nfn opens() {`n}`n")
        $result = Invoke-Census -Root $root
        Assert-Equal 1 $result.ExitCode 'exit code'
        if ($result.Output -notmatch 'REMOVED\s+alpha::opens') {
            throw "the removed test must be named, got: $($result.Output)"
        }
    }
    finally { Remove-Fixture $root }
}

Test-Case 'a missing inventory is a configuration error, not a pass' {
    $root = New-Fixture @{ alpha = $opensOne }
    try {
        $result = Invoke-Census -Root $root
        Assert-Equal 2 $result.ExitCode 'exit code'
    }
    finally { Remove-Fixture $root }
}

Write-Report ''
if ($script:Failures -gt 0) {
    Write-Report "$($script:Failures) failure(s)." -Level bad
    exit 1
}
Write-Report 'All cases passed.' -Level good
exit 0
