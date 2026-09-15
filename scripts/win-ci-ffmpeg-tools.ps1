# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Stages the pinned FFmpeg Windows build toolchain for the native journal gate.
#
# The native host carries MSVC but deliberately carries no MSYS2 shell, GNU
# make, NASM or libclang as ambient build state, and the vendored
# `ffmpeg-sys-next` build script calls bare `sh`/`make` and lets bindgen look
# for `LIBCLANG_PATH`. A cache-miss vendored FFmpeg build therefore needs those
# four tools supplied by the run itself.
#
# The archives are the same four `builder-inputs.toml` pins the controlled
# producer uses, fetched and sha256-verified on the driver host by
# `solstone-distribution acquire ffmpeg-windows-tools` and transferred here;
# acquisition does not run on Windows. This script re-verifies them against the
# transferred checkout's own `builder-inputs.toml` through the same recorder
# code path the producer's `verify-inputs` reaches, extracts them exactly the
# way `core/distribution/windows-produce.ps1` does, and caches the extracted
# tree under a root named for the pinned identities, so moving a pin stages a
# new tree instead of reusing tools the checkout no longer admits.
#
# `stage` prepares the tree and writes a `set`-only cmd fragment. `assert`
# runs after the caller applies that fragment and proves three things from the
# caller's own environment: that `sh`, `make` and `nasm` resolve to the staged
# copies; that `cl` and `link` still resolve to the host toolchain, because the
# staged MSYS2 bin has to precede the inherited PATH to beat Git for Windows'
# own `sh.exe` and carries a coreutils `link.exe` of its own; and that the
# staged shell actually runs -- a flattened bin-only layout resolves `sh.exe`
# but fails FFmpeg's own configure on a generated script's shebang, because
# `msys-2.0.dll` derives its POSIX root from its own directory.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateSet('stage','assert')][string]$Mode,
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$ToolsRoot,
    [string]$InputRoot,
    [string]$Recorder,
    [string]$EnvironmentScript
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$utf8=[Text.UTF8Encoding]::new($false,$true)
$schema='solstone.ffmpeg-windows-tools.v1'
$receiptName='staged-tools.json'

function Require-PlainPath([string]$Path) {
    # Parentheses are literal inside the quoted `set` fragment; the rest would
    # not survive it, so they are refused here rather than mis-expanded there.
    if ($Path -notmatch '^[A-Za-z]:\\' -or $Path -match '["''%!&|<>^`$\x00-\x1f]') {
        throw "plain absolute drive path required by the staged build environment: $Path"
    }
}
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "required file absent: $Path" }
}
function Msys-Path([string]$Path) {
    Require-PlainPath $Path
    return '/'+$Path.Substring(0,1).ToLowerInvariant()+'/'+$Path.Substring(3).Replace('\','/')
}
function One-File([string]$Root,[string]$Name) {
    $files=@(Get-ChildItem -LiteralPath $Root -Filter $Name -File -Recurse)
    if ($files.Count -ne 1) { throw "expected exactly one $Name under $Root, found $($files.Count)" }
    return $files[0].FullName
}
function Rebase-Path([string]$Path,[string]$From,[string]$To) {
    # Extraction happens in a work directory that is renamed into place, so
    # every path recorded in the receipt has to name where the file will live,
    # not where it was written.
    if (-not $Path.StartsWith($From.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)) {
        throw "staged path outside the work root: $Path"
    }
    return (Join-Path $To $Path.Substring($From.TrimEnd('\').Length+1))
}
function Invoke-Tool([string]$Label,[string]$Command,[string[]]$Arguments) {
    # The exit code read here is the one this launch produced: no pipeline sits
    # between the call and the check, and a nonzero status throws rather than
    # letting a later step report this step's success.
    Require-File $Command
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Label exited $LASTEXITCODE" }
}
function Read-Receipt([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return $null }
    if ((Get-Item -LiteralPath $Path).Length -gt 131072) { return $null }
    try { $receipt=[IO.File]::ReadAllText($Path,$utf8) | ConvertFrom-Json } catch { return $null }
    if ($null -eq $receipt -or $receipt.schema -cne $schema) { return $null }
    return $receipt
}
function Assert-FirstApplication([string]$Name,[string]$Expected) {
    $applications=@(Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue)
    if ($applications.Count -lt 1) { throw "no PATH application for ${Name}; the staged copy is $Expected" }
    $source=$applications[0].Source
    if ($source -isnot [string] -or [string]::IsNullOrEmpty($source)) {
        throw "first PATH application has no scalar nonempty source: $Name"
    }
    if (-not $source.Equals($Expected,[StringComparison]::OrdinalIgnoreCase)) {
        throw "unexpected first PATH resolution for ${Name}: $source (expected $Expected)"
    }
}

Require-PlainPath $ToolsRoot
$RepositoryRoot=(Resolve-Path -LiteralPath $RepositoryRoot).ProviderPath
$systemTar=Join-Path $env:SystemRoot 'System32\tar.exe'

if ($Mode -eq 'assert') {
    $receiptPath=Join-Path $ToolsRoot $receiptName
    $receipt=Read-Receipt $receiptPath
    if ($null -eq $receipt) { throw "no staged FFmpeg tool receipt at $receiptPath; run this script in stage mode first" }
    foreach ($name in @('sh','make','nasm')) {
        Assert-FirstApplication "$name.exe" $receipt.resolved.$name
    }
    # Both directions matter. MSYS2's usr/bin carries a coreutils `link.exe`,
    # so a staged tree that wins for `sh` can silently take the linker with it;
    # these assert the host toolchain still wins where it must.
    foreach ($name in @('cl','link')) {
        Assert-FirstApplication "$name.exe" $receipt.host.$name
        if ($receipt.host.$name.StartsWith($ToolsRoot.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)) {
            throw "host $name.exe resolved inside the staged tool root: $($receipt.host.$name)"
        }
    }
    if ($env:LIBCLANG_PATH -cne $receipt.resolved.libclang_dir) {
        throw "LIBCLANG_PATH is $($env:LIBCLANG_PATH); the staged tools record $($receipt.resolved.libclang_dir)"
    }
    # A resolvable sh.exe is not a working POSIX root. The probe is the check
    # that catches the layout failure a PATH assertion cannot see.
    $probe=& $receipt.resolved.sh '-c' 'printf FFMPEG_SH_READY'
    if ($LASTEXITCODE -ne 0) { throw "staged shell probe exited $LASTEXITCODE" }
    if ($probe -cne 'FFMPEG_SH_READY') { throw "staged shell probe returned $probe" }
    Write-Output "JOURNAL_WIN_CI_FFMPEG_TOOLS=executed/pass"
    exit 0
}

if ([string]::IsNullOrEmpty($InputRoot)) { throw 'stage mode requires -InputRoot' }
if ([string]::IsNullOrEmpty($Recorder)) { throw 'stage mode requires -Recorder' }
if ([string]::IsNullOrEmpty($EnvironmentScript)) { throw 'stage mode requires -EnvironmentScript' }
Require-PlainPath $InputRoot
Require-File $Recorder

# Identity comes from the checkout's own pin table, read by the same code the
# producer's input verification uses; this script never parses the pins itself.
Push-Location -LiteralPath $RepositoryRoot
try {
    $verification=& $Recorder 'ffmpeg-windows' 'verify-tools' '--input-root' $InputRoot
    $verificationStatus=$LASTEXITCODE
} finally { Pop-Location }
if ($verificationStatus -ne 0) { throw "staged FFmpeg tool verification exited $verificationStatus" }
$verified=($verification -join "`n") | ConvertFrom-Json
if ($verified.schema -cne $schema) { throw "unexpected tool verification schema $($verified.schema)" }
if (@($verified.tools).Count -ne 4) { throw "expected four verified tool archives, found $(@($verified.tools).Count)" }
$archive=@{}
foreach ($tool in $verified.tools) { $archive[$tool.key]=$tool }
foreach ($key in @('msys2_base','make','nasm','llvm')) {
    if (-not $archive.ContainsKey($key)) { throw "tool verification omitted $key" }
    Require-File $archive[$key].path
}

$staged=Join-Path $ToolsRoot $verified.fingerprint
$receiptPath=Join-Path $staged $receiptName
$existing=Read-Receipt $receiptPath
$disposition='reused'
if ($null -ne $existing -and $existing.fingerprint -ceq $verified.fingerprint) {
    foreach ($name in @('sh','make','nasm','bash','libclang')) {
        if (-not (Test-Path -LiteralPath $existing.resolved.$name -PathType Leaf)) { $existing=$null; break }
    }
} else {
    $existing=$null
}

if ($null -eq $existing) {
    $disposition='extracted'
    if (-not (Test-Path -LiteralPath $ToolsRoot)) { New-Item -ItemType Directory -Path $ToolsRoot -ErrorAction Stop | Out-Null }
    if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Recurse -Force -ErrorAction Stop }
    $work=Join-Path $ToolsRoot ('.staging-'+[Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $work -ErrorAction Stop | Out-Null
    try {
        # Same order, same extractors and same destination shape as
        # core/distribution/windows-produce.ps1. msys2-base keeps its real
        # <root>\usr\bin layout; the make package is unpacked by the staged
        # msys2's own zstd and tar so it lands inside that root correctly.
        Invoke-Tool 'msys2-extract' $systemTar @('-xJf',$archive['msys2_base'].path,'-C',$work)
        $msysRoot=Join-Path $work 'msys64'
        $msysBin=Join-Path $msysRoot 'usr\bin'
        $bash=Join-Path $msysBin 'bash.exe'
        Require-File $bash
        $makeExtract="'/usr/bin/zstd' -d -c '$(Msys-Path $archive['make'].path)' | '/usr/bin/tar' -x -C '$(Msys-Path $msysRoot)'"
        Invoke-Tool 'make-extract' $bash @('--noprofile','--norc','-o','pipefail','-c',$makeExtract)
        $nasmRoot=Join-Path $work 'nasm'
        New-Item -ItemType Directory -Path $nasmRoot -ErrorAction Stop | Out-Null
        Invoke-Tool 'nasm-extract' $systemTar @('-xf',$archive['nasm'].path,'-C',$nasmRoot)
        Invoke-Tool 'llvm-extract' $systemTar @('-xJf',$archive['llvm'].path,'-C',$work)
        $nasm=One-File $nasmRoot 'nasm.exe'
        $libclang=One-File $work 'libclang.dll'
        $sh=Join-Path $msysBin 'sh.exe'
        $make=Join-Path $msysBin 'make.exe'
        foreach ($required in @($sh,$make,$nasm,$libclang)) { Require-File $required }
        $finalBash=Rebase-Path $bash $work $staged
        $finalSh=Rebase-Path $sh $work $staged
        $finalMake=Rebase-Path $make $work $staged
        $finalNasm=Rebase-Path $nasm $work $staged
        $finalLibclang=Rebase-Path $libclang $work $staged
        $receipt=[ordered]@{
            schema=$schema
            fingerprint=$verified.fingerprint
            tools=@($verified.tools | ForEach-Object { [ordered]@{key=$_.key;filename=$_.filename;sha256=$_.sha256;size=$_.size} })
            resolved=[ordered]@{
                bash=$finalBash
                sh=$finalSh
                make=$finalMake
                nasm=$finalNasm
                nasm_dir=(Split-Path -Parent $finalNasm)
                libclang=$finalLibclang
                libclang_dir=(Split-Path -Parent $finalLibclang)
                msys_bin=(Rebase-Path $msysBin $work $staged)
            }
        }
        # The receipt is written into the work tree, so a tree that is present
        # under its fingerprint is a tree that finished extracting.
        [IO.File]::WriteAllText((Join-Path $work $receiptName),(ConvertTo-Json -InputObject $receipt -Depth 10),$utf8)
        Move-Item -LiteralPath $work -Destination $staged -ErrorAction Stop
        $work=$null
    } finally {
        if ($null -ne $work -and (Test-Path -LiteralPath $work)) { Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue }
    }
    $existing=Read-Receipt $receiptPath
    if ($null -eq $existing) { throw "staged FFmpeg tool receipt did not publish at $receiptPath" }
    # Every recorded path is proved at its published location, so a receipt that
    # describes a tree nobody can reach fails here rather than at the assert.
    foreach ($name in @('bash','sh','make','nasm','libclang')) { Require-File $existing.resolved.$name }
    # A pin move leaves the previous tree behind, and an extracted LLVM release
    # is multiple gigabytes. Only fingerprint-named siblings and abandoned
    # staging directories are removed, and only right after a fresh extraction
    # published under the box fence that serializes native work here.
    foreach ($stale in @(Get-ChildItem -LiteralPath $ToolsRoot -Directory)) {
        if ($stale.Name -ceq $verified.fingerprint) { continue }
        if ($stale.Name -cmatch '^[0-9a-f]{16}$' -or $stale.Name -cmatch '^\.staging-[0-9a-f]{32}$') {
            Remove-Item -LiteralPath $stale.FullName -Recurse -Force -ErrorAction SilentlyContinue
            Write-Output ("JOURNAL_WIN_CI_FFMPEG_TOOLS_REAPED=" + $stale.Name)
        }
    }
}

foreach ($path in @($existing.resolved.nasm_dir,$existing.resolved.libclang_dir,$existing.resolved.msys_bin)) { Require-PlainPath $path }

# The staged MSYS2 bin has to sit ahead of the inherited PATH, because Git for
# Windows ships its own `sh.exe` there and the vendored FFmpeg build resolves
# `sh` and `make` by name. That same directory also carries a coreutils
# `link.exe`, so the host compiler and linker are pinned ahead of it rather
# than left to whatever order the caller happened to inherit.
$hostTools=[ordered]@{}
foreach ($name in @('cl','link')) {
    $found=@(Get-Command "$name.exe" -CommandType Application -ErrorAction SilentlyContinue)
    if ($found.Count -lt 1) { throw "stage mode needs the host build environment on PATH; $name.exe did not resolve" }
    $hostTools[$name]=$found[0].Source
    Require-PlainPath $found[0].Source
}
$hostDirectories=@()
foreach ($source in $hostTools.Values) {
    $directory=Split-Path -Parent $source
    if ($hostDirectories -notcontains $directory) { $hostDirectories+=$directory }
}
# The host entries are environment-derived, not pin-derived, so they live only
# in this per-run copy and never in the cached per-fingerprint receipt.
$published=[ordered]@{schema=$existing.schema;fingerprint=$existing.fingerprint;tools=$existing.tools;resolved=$existing.resolved;host=$hostTools}
# The assert pass reads this copy, so it names the tree the caller will use.
[IO.File]::WriteAllText((Join-Path $ToolsRoot $receiptName),(ConvertTo-Json -InputObject $published -Depth 10),$utf8)
$fragment="set `"PATH=$($hostDirectories -join ';');$($existing.resolved.msys_bin);$($existing.resolved.nasm_dir);%PATH%`"`r`n"+
    "set `"LIBCLANG_PATH=$($existing.resolved.libclang_dir)`"`r`n"
[IO.File]::WriteAllText($EnvironmentScript,$fragment,$utf8)

foreach ($tool in $existing.tools) {
    Write-Output ("JOURNAL_WIN_CI_FFMPEG_TOOL_"+$tool.key.ToUpperInvariant()+"="+$tool.filename+" sha256="+$tool.sha256+" size="+$tool.size)
}
Write-Output "JOURNAL_WIN_CI_FFMPEG_TOOLS_FINGERPRINT=$($existing.fingerprint)"
Write-Output "JOURNAL_WIN_CI_FFMPEG_TOOLS_ROOT=$staged"
Write-Output "JOURNAL_WIN_CI_FFMPEG_TOOLS_STAGE=$disposition"
exit 0
