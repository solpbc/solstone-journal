# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Network-denied native half of the controlled Windows parakeet.cpp build.
# It accepts only driver-verified inputs and records a pre-signing server plus
# the exact copied model. It neither launches a server nor creates a package.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$RepositoryRoot,
    [Parameter(Mandatory = $true)][string]$SourceArchive,
    [Parameter(Mandatory = $true)][string]$CmakeArchive,
    [Parameter(Mandatory = $true)][string]$Model,
    [Parameter(Mandatory = $true)][string]$WorkRoot,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [Parameter(Mandatory = $true)][string]$ReportRoot,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)][string]$BuilderHost
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$firewallRules = [System.Collections.Generic.List[string]]::new()

function Require-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "$Label is not a file: $Path" }
}
function Require-NewRoot([string]$Path, [string]$Label) {
    if (Test-Path -LiteralPath $Path) { throw "$Label already exists and will not be reused: $Path" }
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}
function Get-Sha256([string]$Path) { return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Invoke-Checked([string]$Label, [string]$Command, [string[]]$Arguments) {
    Write-Host "=== $Label ==="; & $Command @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Label failed with exit code $LASTEXITCODE" }
}
function Add-NetworkDeny([string]$Program, [int]$Ordinal) {
    Require-File $Program 'network-deny program'
    $name = "solstone-parakeet-build-$PID-$Ordinal-$([Guid]::NewGuid().ToString('N'))"
    New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $Program -Profile Any | Out-Null
    $firewallRules.Add($name)
}
function Remove-NetworkDenies { foreach ($name in $firewallRules) { Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue } }

try {
    foreach ($item in @(@($SourceArchive, 'Parakeet source archive'), @($CmakeArchive, 'CMake archive'), @($Model, 'Parakeet model'))) { Require-File $item[0] $item[1] }
    Require-NewRoot $WorkRoot 'Parakeet work root'; Require-NewRoot $OutputRoot 'Parakeet output root'; Require-NewRoot $ReportRoot 'Parakeet report root'
    Set-Location -LiteralPath $RepositoryRoot
    if ((& git rev-parse HEAD).Trim() -ne $ExpectedProductCommit) { throw 'repository commit does not match source-bound driver value' }
    $dirty = & git status --porcelain=v1 --untracked-files=all --ignore-submodules=none
    if ($LASTEXITCODE -ne 0 -or ($null -ne $dirty -and $dirty.Count -ne 0)) { throw 'repository is not a clean transferred checkout' }
    if ((Get-Sha256 (Join-Path $RepositoryRoot 'core/Cargo.lock')) -ne $ExpectedCargoLockSha256) { throw 'repository Cargo.lock does not match source-bound driver value' }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'; Require-File $vswhere 'vswhere'
    $vsInstall = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if ([string]::IsNullOrWhiteSpace($vsInstall)) { throw 'Visual Studio Build Tools with VC.Tools.x86.x64 are required' }
    $vcvars = Join-Path $vsInstall 'VC/Auxiliary/Build/vcvarsall.bat'; Require-File $vcvars 'vcvarsall'
    $msvcRoot = Get-ChildItem -LiteralPath (Join-Path $vsInstall 'VC/Tools/MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
    if ($null -eq $msvcRoot) { throw 'MSVC tools directory is absent' }
    $toolBin = Join-Path $msvcRoot.FullName 'bin/Hostx64/x64'; $cl = Join-Path $toolBin 'cl.exe'; $link = Join-Path $toolBin 'link.exe'; $msbuild = Join-Path $vsInstall 'MSBuild/Current/Bin/MSBuild.exe'; $git = (Get-Command git.exe -ErrorAction Stop).Source
    Require-File $cl 'MSVC cl.exe'; Require-File $link 'MSVC link.exe'; Require-File $msbuild 'MSBuild.exe'
    $cargoBuild = 'call "{0}" x64 >nul && cargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked --offline' -f $vcvars
    Invoke-Checked 'offline build of Parakeet receipt recorder' 'cmd.exe' @('/d', '/s', '/c', $cargoBuild)
    $distribution = Join-Path $RepositoryRoot 'core/target/debug/solstone-distribution.exe'; Require-File $distribution 'Parakeet receipt recorder'
    Invoke-Checked 'verify transferred Parakeet inputs' $distribution @('parakeet-windows', 'verify-inputs', '--source-archive', $SourceArchive, '--cmake-archive', $CmakeArchive, '--model', $Model)

    $sourceRoot = Join-Path $WorkRoot 'source'; $cmakeRoot = Join-Path $WorkRoot 'cmake'; $buildRoot = Join-Path $WorkRoot 'build'
    New-Item -ItemType Directory -Path $sourceRoot, $cmakeRoot | Out-Null
    Invoke-Checked 'extract Parakeet source archive' 'tar.exe' @('-xzf', $SourceArchive, '-C', $sourceRoot)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive, $cmakeRoot)
    $cmake = @(Get-ChildItem -LiteralPath $cmakeRoot -Filter cmake.exe -Recurse -File)
    if ($cmake.Count -ne 1) { throw 'controlled CMake archive did not produce exactly one cmake.exe' }
    $cmakeExe = $cmake[0].FullName
    $ordinal = 1; foreach ($program in @($cmakeExe, $msbuild, $cl, $link, $git) | Select-Object -Unique) { Add-NetworkDeny $program $ordinal; $ordinal += 1 }
    Write-Host 'PARAKEET_WINDOWS_NETWORK_DENY=firewall-outbound-block-for-cmake-msbuild-cl-link-git'
    Invoke-Checked 'configure static CPU-only Parakeet server' $cmakeExe @(
        '-S', $sourceRoot, '-B', $buildRoot, '-G', 'Visual Studio 17 2022', '-A', 'x64',
        '-DPARAKEET_BUILD_TESTS=OFF', '-DPARAKEET_BUILD_CLI=OFF', '-DPARAKEET_BUILD_SERVER=ON', '-DPARAKEET_SHARED=OFF', '-DBUILD_SHARED_LIBS=OFF',
        '-DPARAKEET_GGML_CUDA=OFF', '-DPARAKEET_GGML_METAL=OFF', '-DPARAKEET_GGML_VULKAN=OFF', '-DPARAKEET_GGML_HIP=OFF',
        '-DGGML_NATIVE=OFF', '-DGGML_LLAMAFILE=OFF', '-DBASH_EXECUTABLE=FALSE', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL'
    )
    $cache = Join-Path $buildRoot 'CMakeCache.txt'; Require-File $cache 'Parakeet CMake cache'; $cacheText = Get-Content -LiteralPath $cache -Raw
    foreach ($entry in @('PARAKEET_BUILD_TESTS=OFF', 'PARAKEET_BUILD_CLI=OFF', 'PARAKEET_BUILD_SERVER=ON', 'PARAKEET_SHARED=OFF', 'BUILD_SHARED_LIBS=OFF', 'PARAKEET_GGML_CUDA=OFF', 'PARAKEET_GGML_METAL=OFF', 'PARAKEET_GGML_VULKAN=OFF', 'PARAKEET_GGML_HIP=OFF', 'GGML_NATIVE=OFF', 'GGML_LLAMAFILE=OFF', 'CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL')) { if ($cacheText -notmatch "(?m)^$([regex]::Escape($entry.Split('=')[0]))(?::[A-Z_]+)?=$([regex]::Escape($entry.Split('=')[1]))\r?$") { throw "CMake cache does not retain $entry" } }
    Invoke-Checked 'build static Parakeet server' $cmakeExe @('--build', $buildRoot, '--config', 'Release', '--target', 'parakeet-server', '--parallel', '2')
    $server = @(Get-ChildItem -LiteralPath $buildRoot -Filter 'parakeet-server.exe' -Recurse -File)
    if ($server.Count -ne 1) { throw "Parakeet build produced $($server.Count) parakeet-server.exe files; expected exactly one" }
    $ggmlDll = @(Get-ChildItem -LiteralPath $buildRoot -Filter 'ggml*.dll' -Recurse -File)
    if ($ggmlDll.Count -ne 0) { throw "Parakeet static build produced dynamic ggml DLLs: $($ggmlDll.FullName -join '; ')" }
    $outputBin = Join-Path $OutputRoot 'bin'; $outputModels = Join-Path $OutputRoot 'models'; New-Item -ItemType Directory -Path $outputBin, $outputModels | Out-Null
    $outputServer = Join-Path $outputBin 'parakeet-server.exe'; $outputModel = Join-Path $outputModels 'tdt-0.6b-v3-q8_0.gguf'
    Copy-Item -LiteralPath $server[0].FullName -Destination $outputServer
    Copy-Item -LiteralPath $Model -Destination $outputModel
    $dumpbin = Join-Path $toolBin 'dumpbin.exe'; Require-File $dumpbin 'dumpbin.exe'
    $dependents = (& $dumpbin /dependents $outputServer 2>&1 | Out-String); if ($LASTEXITCODE -ne 0) { throw 'dumpbin /dependents failed' }
    $forbidden = @($dependents -split "`r?`n" | Where-Object { $_ -match '(?i)(ggml|cuda|nvcuda|cudart|cublas|cudnn|nvrtc|vulkan|opencl|tensorrt|directml)' })
    if ($forbidden.Count -ne 0) { throw "Parakeet server has forbidden imports: $($forbidden -join '; ')" }
    $validation = Join-Path $ReportRoot 'parakeet-build-validation.log'
    @('schema=solstone.parakeet-windows-validation.v1', "product_commit=$ExpectedProductCommit", "cargo_lock_sha256=$ExpectedCargoLockSha256", "source_sha256=$(Get-Sha256 $SourceArchive)", "cmake_cache_sha256=$(Get-Sha256 $cache)", "model_sha256=$(Get-Sha256 $Model)", 'network_access=denied-by-firewall-for-cmake-msbuild-cl-link-git', 'dynamic_ggml_dlls=none', 'forbidden_accelerator_imports=none', "server_sha256=$(Get-Sha256 $outputServer)", "copied_model_sha256=$(Get-Sha256 $outputModel)") | Set-Content -LiteralPath $validation -Encoding utf8
    $toolchain = "$((& $cmakeExe --version | Select-Object -First 1).Trim()); $((Get-Item -LiteralPath $cl).VersionInfo.ProductVersion)"
    $evidence = Join-Path $ReportRoot 'parakeet-build-evidence.json'; $receipt = Join-Path $ReportRoot 'parakeet-build-receipt.json'
    Invoke-Checked 'persist Parakeet controlled-build receipt' $distribution @('parakeet-windows', 'record', '--source-archive', $SourceArchive, '--cmake-archive', $CmakeArchive, '--model', $Model, '--cmake-cache', $cache, '--output-root', $OutputRoot, '--evidence', $evidence, '--receipt', $receipt, '--validation', $validation, '--product-commit', $ExpectedProductCommit, '--cargo-lock-sha256', $ExpectedCargoLockSha256, '--builder-host', $BuilderHost, '--toolchain', $toolchain)
    Invoke-Checked 'rehash persisted Parakeet receipt output' $distribution @('parakeet-windows', 'verify', '--receipt', $receipt, '--output-root', $OutputRoot)
    Write-Host "PARAKEET_WINDOWS_BUILD_OK server=$outputServer receipt=$receipt evidence=$evidence validation=$validation"
} finally { Remove-NetworkDenies }
