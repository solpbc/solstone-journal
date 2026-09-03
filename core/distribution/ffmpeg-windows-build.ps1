# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# The native half of the controlled FFmpeg Windows build. The POSIX driver has
# verified and transferred every archive. This script re-verifies them before
# extraction, uses only the carried MSYS2/make/NASM/LLVM inputs plus MSVC, and
# produces two pre-signing executables and a receipt. It never packages, signs,
# publishes, installs, or starts either executable.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [string]$RepositoryRoot,
    [Parameter(Mandatory = $true)] [string]$SourceArchive,
    [Parameter(Mandatory = $true)] [string]$Msys2Archive,
    [Parameter(Mandatory = $true)] [string]$MakeArchive,
    [Parameter(Mandatory = $true)] [string]$NasmArchive,
    [Parameter(Mandatory = $true)] [string]$LlvmArchive,
    [Parameter(Mandatory = $true)] [string]$WorkRoot,
    [Parameter(Mandatory = $true)] [string]$OutputRoot,
    [Parameter(Mandatory = $true)] [string]$ReportRoot,
    [Parameter(Mandatory = $true)] [ValidatePattern('^[0-9a-f]{40}$')] [string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)] [ValidatePattern('^[0-9a-f]{64}$')] [string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)] [string]$BuilderHost
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$firewallRules = [System.Collections.Generic.List[string]]::new()

function Require-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Label is not a regular file: $Path" }
}
function Require-NewRoot([string]$Path, [string]$Label) {
    if (Test-Path -LiteralPath $Path) { throw "$Label already exists and will not be reused: $Path" }
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}
function Get-Sha256([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Invoke-Checked([string]$Label, [string]$Command, [string[]]$Arguments) {
    Write-Host "=== $Label ==="
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Label failed with exit code $LASTEXITCODE" }
}
function Add-NetworkDeny([string]$Program, [int]$Ordinal) {
    Require-File $Program 'network-deny program'
    $name = "solstone-ffmpeg-build-$PID-$Ordinal-$([Guid]::NewGuid().ToString('N'))"
    New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $Program -Profile Any | Out-Null
    $firewallRules.Add($name)
}
function Remove-NetworkDenies {
    foreach ($name in $firewallRules) { Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue }
}
function Convert-ToMsysPath([string]$Path) {
    if ($Path -notmatch '^([A-Za-z]):\\(.*)$') { throw "cannot convert non-absolute Windows path to MSYS path: $Path" }
    return ('/{0}/{1}' -f $Matches[1].ToLowerInvariant(), $Matches[2].Replace('\', '/'))
}
function Find-OneFile([string]$Root, [string]$Filter, [string]$Label) {
    $matches = @(Get-ChildItem -LiteralPath $Root -Filter $Filter -Recurse -File)
    if ($matches.Count -ne 1) { throw "$Label produced $($matches.Count) matching files; expected exactly one" }
    return $matches[0].FullName
}

try {
    Require-File $SourceArchive 'FFmpeg source archive'
    Require-File $Msys2Archive 'MSYS2 base archive'
    Require-File $MakeArchive 'GNU make archive'
    Require-File $NasmArchive 'NASM archive'
    Require-File $LlvmArchive 'LLVM archive'
    Require-NewRoot $WorkRoot 'FFmpeg work root'
    Require-NewRoot $OutputRoot 'FFmpeg output root'
    Require-NewRoot $ReportRoot 'FFmpeg report root'

    Set-Location -LiteralPath $RepositoryRoot
    $observedCommit = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $observedCommit -ne $ExpectedProductCommit) { throw 'repository commit does not match the source-bound driver value' }
    $dirty = & git status --porcelain=v1 --untracked-files=all --ignore-submodules=none
    if ($LASTEXITCODE -ne 0 -or ($null -ne $dirty -and $dirty.Count -ne 0)) { throw 'repository is not a clean transferred checkout' }
    if ((Get-Sha256 (Join-Path $RepositoryRoot 'core/Cargo.lock')) -ne $ExpectedCargoLockSha256) { throw 'repository Cargo.lock does not match the source-bound driver value' }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    Require-File $vswhere 'vswhere'
    $vsInstall = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if ([string]::IsNullOrWhiteSpace($vsInstall)) { throw 'Visual Studio Build Tools with VC.Tools.x86.x64 are required' }
    $vcvars = Join-Path $vsInstall 'VC/Auxiliary/Build/vcvarsall.bat'
    Require-File $vcvars 'vcvarsall'
    $msvcRoot = Get-ChildItem -LiteralPath (Join-Path $vsInstall 'VC/Tools/MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
    if ($null -eq $msvcRoot) { throw 'MSVC tools directory is absent' }
    $toolBin = Join-Path $msvcRoot.FullName 'bin/Hostx64/x64'
    $cl = Join-Path $toolBin 'cl.exe'; $link = Join-Path $toolBin 'link.exe'; $dumpbin = Join-Path $toolBin 'dumpbin.exe'
    Require-File $cl 'MSVC cl.exe'
    Require-File $link 'MSVC link.exe'
    Require-File $dumpbin 'MSVC dumpbin.exe'

    $cargo = (Get-Command cargo.exe -ErrorAction Stop).Source
    $rustc = (Get-Command rustc.exe -ErrorAction Stop).Source
    $recorderBuild = 'call "{0}" x64 >nul && cargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked --offline' -f $vcvars
    Invoke-Checked 'offline build of the FFmpeg receipt recorder' 'cmd.exe' @('/d', '/s', '/c', $recorderBuild)
    $distribution = Join-Path $RepositoryRoot 'core/target/debug/solstone-distribution.exe'
    Require-File $distribution 'FFmpeg receipt recorder'
    Invoke-Checked 'verify transferred FFmpeg inputs' $distribution @(
        'ffmpeg-windows', 'verify-inputs', '--source-archive', $SourceArchive,
        '--msys2-archive', $Msys2Archive, '--make-archive', $MakeArchive,
        '--nasm-archive', $NasmArchive, '--llvm-archive', $LlvmArchive
    )

    $toolsRoot = Join-Path $WorkRoot 'tools'
    New-Item -ItemType Directory -Path $toolsRoot | Out-Null
    Invoke-Checked 'extract verified MSYS2 base' 'tar.exe' @('-xJf', $Msys2Archive, '-C', $toolsRoot)
    $msysRoot = Join-Path $toolsRoot 'msys64'; $msysBin = Join-Path $msysRoot 'usr/bin'
    $bash = Join-Path $msysBin 'bash.exe'; $sh = Join-Path $msysBin 'sh.exe'; $zstd = Join-Path $msysBin 'zstd.exe'; $tar = Join-Path $msysBin 'tar.exe'
    Require-File $bash 'carried MSYS bash'
    Require-File $sh 'carried MSYS sh'
    Require-File $zstd 'carried MSYS zstd'
    Require-File $tar 'carried MSYS tar'
    $makeMsys = Convert-ToMsysPath $MakeArchive; $msysRootMsys = Convert-ToMsysPath $msysRoot
    Invoke-Checked 'extract verified GNU make package with carried MSYS tools' $bash @('--noprofile', '--norc', '-c', "'/usr/bin/zstd' -d -c '$makeMsys' | '/usr/bin/tar' -x -C '$msysRootMsys'")
    $make = Join-Path $msysBin 'make.exe'; Require-File $make 'carried GNU make'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $nasmRoot = Join-Path $toolsRoot 'nasm'; [IO.Compression.ZipFile]::ExtractToDirectory($NasmArchive, $nasmRoot)
    $nasm = Find-OneFile $nasmRoot 'nasm.exe' 'NASM archive'; Require-File $nasm 'carried NASM'
    $nasmDir = Split-Path -Parent $nasm
    Invoke-Checked 'extract verified LLVM archive' 'tar.exe' @('-xJf', $LlvmArchive, '-C', $toolsRoot)
    $libclang = Find-OneFile $toolsRoot 'libclang.dll' 'LLVM archive'; $llvmBin = Split-Path -Parent $libclang

    $ordinal = 1
    foreach ($program in @($cargo, $rustc, $bash, $sh, $make, $nasm, $cl, $link, $dumpbin) | Select-Object -Unique) { Add-NetworkDeny $program $ordinal; $ordinal += 1 }
    Write-Host 'FFMPEG_WINDOWS_NETWORK_DENY=firewall-outbound-block-for-cargo-rustc-msys-make-nasm-msvc'

    $buildCommand = 'call "{0}" x64 >nul && set "PATH={1};%PATH%;{2}" && set "LIBCLANG_PATH={3}" && set "SOLSTONE_FFMPEG_SOURCE_ARCHIVE={4}" && set "SOLSTONE_DISTRIBUTION_OFFLINE=1" && set "FFMPEG_MARCH=" && set "FFMPEG_MTUNE=" && set "CC=cl" && cargo build --manifest-path core\Cargo.toml -p solstone-core --bin solstone-core --release --locked --offline && cargo build --manifest-path core\Cargo.toml -p solstone-core-describe --bin solstone-core-describe --release --locked --offline' -f $vcvars, $nasmDir, $msysBin, $llvmBin, $SourceArchive
    Invoke-Checked 'network-denied MSVC build of the two FFmpeg-bearing executables' 'cmd.exe' @('/d', '/s', '/c', $buildCommand)

    $targetRoot = Join-Path $RepositoryRoot 'core/target/release'
    $core = Join-Path $targetRoot 'solstone-core.exe'; $describe = Join-Path $targetRoot 'solstone-core-describe.exe'
    Require-File $core 'solstone-core.exe'
    Require-File $describe 'solstone-core-describe.exe'
    $outputBin = Join-Path $OutputRoot 'bin'; New-Item -ItemType Directory -Path $outputBin | Out-Null
    Copy-Item -LiteralPath $core -Destination (Join-Path $outputBin 'solstone-core.exe')
    Copy-Item -LiteralPath $describe -Destination (Join-Path $outputBin 'solstone-core-describe.exe')

    $evidenceRoots = @(Get-ChildItem -LiteralPath (Join-Path $RepositoryRoot 'core/target/release/build') -Filter current-run.v1 -Recurse -File | ForEach-Object { $_.Directory.Parent.FullName })
    $audioEvidence = @(); $videoEvidence = @()
    foreach ($evidenceRoot in $evidenceRoots) {
        $record = Get-Content -LiteralPath (Join-Path $evidenceRoot 'solstone-ffmpeg-evidence/current-run.v1') -Raw
        if ($record -notmatch '(?m)^target=x86_64-pc-windows-msvc$' -or $record -notmatch '(?m)^profile=release$' -or $record -notmatch '(?m)^configure_executed=true$') { continue }
        $receiptName = [regex]::Match($record, '(?m)^receipt_filename=(.+)$').Groups[1].Value
        if ([string]::IsNullOrWhiteSpace($receiptName)) { throw "FFmpeg evidence record has no receipt filename: $evidenceRoot" }
        $receiptText = Get-Content -LiteralPath (Join-Path $evidenceRoot "solstone-ffmpeg-evidence/$receiptName") -Raw
        if ($receiptText -match '(?m)^component=CONFIG_OPUS_MUXER$') { $audioEvidence += $evidenceRoot } else { $videoEvidence += $evidenceRoot }
    }
    if ($audioEvidence.Count -ne 1 -or $videoEvidence.Count -ne 1) { throw "FFmpeg build must retain exactly one audio and one video configure evidence root; observed audio=$($audioEvidence.Count) video=$($videoEvidence.Count)" }

    $importLines = @()
    foreach ($exe in @($core, $describe)) {
        $dependents = (& $dumpbin /dependents $exe 2>&1 | Out-String)
        if ($LASTEXITCODE -ne 0) { throw "dumpbin /dependents failed for $exe" }
        $matches = @($dependents -split "`r?`n" | Where-Object { $_ -match '(?i)\b(avcodec|avdevice|avfilter|avformat|avutil|avresample|postproc|swresample|swscale|ffmpeg)[a-z0-9_.-]*\.dll\b' })
        if ($matches.Count -ne 0) { throw "FFmpeg dynamic DLL import retained by ${exe}: $($matches -join '; ')" }
        $importLines += "$([IO.Path]::GetFileName($exe))=no-ffmpeg-dynamic-import"
    }
    $validation = Join-Path $ReportRoot 'ffmpeg-build-validation.log'
    @(
        'schema=solstone.ffmpeg-windows-validation.v1', "product_commit=$ExpectedProductCommit", "cargo_lock_sha256=$ExpectedCargoLockSha256",
        "source_sha256=$(Get-Sha256 $SourceArchive)", "msys2_sha256=$(Get-Sha256 $Msys2Archive)", "make_sha256=$(Get-Sha256 $MakeArchive)",
        "nasm_sha256=$(Get-Sha256 $NasmArchive)", "llvm_sha256=$(Get-Sha256 $LlvmArchive)",
        'network_access=denied-by-firewall-for-cargo-rustc-msys-make-nasm-msvc',
        "audio_evidence_dir=$($audioEvidence[0])", "video_evidence_dir=$($videoEvidence[0])"
    ) + $importLines | Set-Content -LiteralPath $validation -Encoding utf8
    $toolchain = "MSVC $((Get-Item -LiteralPath $cl).VersionInfo.ProductVersion); NASM $((& $nasm -v).Trim()); LLVM libclang $((Get-Item -LiteralPath $libclang).VersionInfo.ProductVersion)"
    $evidence = Join-Path $ReportRoot 'ffmpeg-build-evidence.json'; $receipt = Join-Path $ReportRoot 'ffmpeg-build-receipt.json'
    Invoke-Checked 'persist FFmpeg controlled-build receipt' $distribution @(
        'ffmpeg-windows', 'record', '--source-archive', $SourceArchive, '--msys2-archive', $Msys2Archive,
        '--make-archive', $MakeArchive, '--nasm-archive', $NasmArchive, '--llvm-archive', $LlvmArchive,
        '--audio-evidence-dir', $audioEvidence[0], '--video-evidence-dir', $videoEvidence[0], '--output-root', $OutputRoot,
        '--evidence', $evidence, '--receipt', $receipt, '--validation', $validation, '--product-commit', $ExpectedProductCommit,
        '--cargo-lock-sha256', $ExpectedCargoLockSha256, '--builder-host', $BuilderHost, '--toolchain', $toolchain
    )
    Invoke-Checked 'rehash and re-census persisted FFmpeg receipt output' $distribution @('ffmpeg-windows', 'verify', '--receipt', $receipt, '--output-root', $OutputRoot)
    Write-Host "FFMPEG_WINDOWS_BUILD_OK output=$outputBin receipt=$receipt evidence=$evidence validation=$validation"
}
finally { Remove-NetworkDenies }
