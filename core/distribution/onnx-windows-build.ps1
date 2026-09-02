# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Network-denied native half of the controlled Windows ONNX Runtime build.
# This consumes only driver-verified archives and records a pre-signing DLL;
# it never packages, signs, loads, or advertises the dependency.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$RepositoryRoot,
    [Parameter(Mandatory = $true)][string]$SourceArchive,
    [Parameter(Mandatory = $true)][string]$MirrorArchive,
    [Parameter(Mandatory = $true)][string]$CmakeArchive,
    [Parameter(Mandatory = $true)][string]$PythonArchive,
    [Parameter(Mandatory = $true)][string]$ProtocArchive,
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
    $name = "solstone-onnx-build-$PID-$Ordinal-$([Guid]::NewGuid().ToString('N'))"
    New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $Program -Profile Any | Out-Null
    $firewallRules.Add($name)
}
function Remove-NetworkDenies { foreach ($name in $firewallRules) { Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue } }

try {
    foreach ($item in @(@($SourceArchive, 'ONNX source archive'), @($MirrorArchive, 'ONNX mirror archive'), @($CmakeArchive, 'CMake archive'), @($PythonArchive, 'Python archive'), @($ProtocArchive, 'protoc archive'))) { Require-File $item[0] $item[1] }
    Require-NewRoot $WorkRoot 'ONNX work root'; Require-NewRoot $OutputRoot 'ONNX output root'; Require-NewRoot $ReportRoot 'ONNX report root'
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
    Invoke-Checked 'offline build of ONNX receipt recorder' 'cmd.exe' @('/d', '/s', '/c', $cargoBuild)
    $distribution = Join-Path $RepositoryRoot 'core/target/debug/solstone-distribution.exe'; Require-File $distribution 'ONNX receipt recorder'
    Invoke-Checked 'verify transferred ONNX inputs' $distribution @('onnx-windows', 'verify-inputs', '--source-archive', $SourceArchive, '--mirror-archive', $MirrorArchive, '--cmake-archive', $CmakeArchive, '--python-archive', $PythonArchive, '--protoc-archive', $ProtocArchive)

    $sourceRoot = Join-Path $WorkRoot 'source'; $mirrorRoot = Join-Path $WorkRoot 'mirror'; $cmakeRoot = Join-Path $WorkRoot 'cmake'; $pythonRoot = Join-Path $WorkRoot 'python'; $protocRoot = Join-Path $WorkRoot 'protoc'
    New-Item -ItemType Directory -Path $sourceRoot, $mirrorRoot, $cmakeRoot, $pythonRoot, $protocRoot | Out-Null
    Invoke-Checked 'extract ONNX source archive' 'tar.exe' @('-xzf', $SourceArchive, '-C', $sourceRoot)
    Invoke-Checked 'extract ONNX CMake mirror' 'tar.exe' @('-xzf', $MirrorArchive, '-C', $mirrorRoot)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive, $cmakeRoot); [IO.Compression.ZipFile]::ExtractToDirectory($PythonArchive, $pythonRoot); [IO.Compression.ZipFile]::ExtractToDirectory($ProtocArchive, $protocRoot)
    $cmake = @(Get-ChildItem -LiteralPath $cmakeRoot -Filter cmake.exe -Recurse -File); $python = @(Get-ChildItem -LiteralPath $pythonRoot -Filter python.exe -Recurse -File); $protoc = @(Get-ChildItem -LiteralPath $protocRoot -Filter protoc.exe -Recurse -File)
    if ($cmake.Count -ne 1 -or $python.Count -ne 1 -or $protoc.Count -ne 1) { throw 'controlled tool archives did not produce exactly one cmake.exe, python.exe, and protoc.exe' }
    $cmakeExe = $cmake[0].FullName; $pythonExe = $python[0].FullName; $protocExe = $protoc[0].FullName
    $pythonPth = @(Get-ChildItem -LiteralPath $pythonRoot -Filter 'python*._pth' -File)
    if ($pythonPth.Count -ne 1) { throw 'controlled Python archive did not produce exactly one python*._pth file' }
    # The embedded interpreter intentionally ignores ambient PYTHONPATH. Add
    # only the reviewed extracted ONNX build-driver and tools directories so
    # build.py can import its source-owned modules; do not enable site-packages
    # or pip.
    Add-Content -LiteralPath $pythonPth[0].FullName -Value (Join-Path $sourceRoot 'tools/ci_build') -Encoding ascii
    Add-Content -LiteralPath $pythonPth[0].FullName -Value (Join-Path $sourceRoot 'tools/python') -Encoding ascii
    $ordinal = 1; foreach ($program in @($cmakeExe, $pythonExe, $protocExe, $msbuild, $cl, $link, $git) | Select-Object -Unique) { Add-NetworkDeny $program $ordinal; $ordinal += 1 }
    Write-Host 'ONNX_WINDOWS_NETWORK_DENY=firewall-outbound-block-for-cmake-python-protoc-msbuild-cl-link-git'
    Invoke-Checked 'verify isolated ONNX build-driver imports' $pythonExe @('-c', 'import build_args; import util; print(build_args.__file__); print(util.__file__)')
    Invoke-Checked 'build reduced CPU-only ONNX Runtime' $pythonExe @(
        (Join-Path $sourceRoot 'tools/ci_build/build.py'), '--config', 'Release', '--update', '--build', '--skip_tests', '--skip_submodule_sync', '--parallel', '2',
        '--build_shared_lib', '--include_ops_by_config', (Join-Path $sourceRoot 'required-operators.config'), '--disable_contrib_ops', '--disable_ml_ops',
        '--path_to_protoc_exe', $protocExe, '--cmake_path', $cmakeExe, '--cmake_generator', 'Visual Studio 17 2022', '--cmake_deps_mirror_dir', $mirrorRoot,
        '--cmake_extra_defines', 'CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL', 'onnxruntime_USE_TELEMETRY=OFF', 'onnxruntime_BUILD_UNIT_TESTS=OFF'
    )
    $cache = Join-Path $sourceRoot 'build/Windows/Release/CMakeCache.txt'; Require-File $cache 'ONNX Runtime CMake cache'; $cacheText = Get-Content -LiteralPath $cache -Raw
    foreach ($entry in @('onnxruntime_BUILD_SHARED_LIB=ON', 'onnxruntime_DISABLE_CONTRIB_OPS=ON', 'onnxruntime_DISABLE_ML_OPS=ON', 'onnxruntime_USE_TELEMETRY=OFF', 'CMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL')) { if ($cacheText -notmatch "(?m)^$([regex]::Escape($entry.Split('=')[0]))(?::[A-Z_]+)?=$([regex]::Escape($entry.Split('=')[1]))\r?$") { throw "CMake cache does not retain $entry" } }
    $dll = @(Get-ChildItem -LiteralPath (Join-Path $sourceRoot 'build') -Filter onnxruntime.dll -Recurse -File); if ($dll.Count -ne 1) { throw "ONNX build produced $($dll.Count) onnxruntime.dll files; expected exactly one" }
    $outputBin = Join-Path $OutputRoot 'bin'; New-Item -ItemType Directory -Path $outputBin | Out-Null; $outputDll = Join-Path $outputBin 'onnxruntime.dll'; Copy-Item -LiteralPath $dll[0].FullName -Destination $outputDll
    $dumpbin = Join-Path $toolBin 'dumpbin.exe'; Require-File $dumpbin 'dumpbin.exe'; $dependents = (& $dumpbin /dependents $outputDll 2>&1 | Out-String); if ($LASTEXITCODE -ne 0) { throw 'dumpbin /dependents failed' }
    $forbidden = @($dependents -split "`r?`n" | Where-Object { $_ -match '(?i)(cuda|nvcuda|cudart|cublas|cudnn|nvrtc|vulkan|opencl|tensorrt|directml)' }); if ($forbidden.Count -ne 0) { throw "ONNX DLL has forbidden accelerator imports: $($forbidden -join '; ')" }
    $validation = Join-Path $ReportRoot 'onnxruntime-build-validation.log'; @('schema=solstone.onnx-windows-validation.v1', "product_commit=$ExpectedProductCommit", "cargo_lock_sha256=$ExpectedCargoLockSha256", "source_sha256=$(Get-Sha256 $SourceArchive)", "mirror_sha256=$(Get-Sha256 $MirrorArchive)", "cmake_cache_sha256=$(Get-Sha256 $cache)", 'network_access=denied-by-firewall-for-cmake-python-protoc-msbuild-cl-link-git', 'forbidden_accelerator_imports=none', "output_sha256=$(Get-Sha256 $outputDll)") | Set-Content -LiteralPath $validation -Encoding utf8
    $toolchain = "$((& $cmakeExe --version | Select-Object -First 1).Trim()); $((Get-Item -LiteralPath $cl).VersionInfo.ProductVersion)"; $evidence = Join-Path $ReportRoot 'onnxruntime-build-evidence.json'; $receipt = Join-Path $ReportRoot 'onnxruntime-build-receipt.json'
    Invoke-Checked 'persist ONNX controlled-build receipt' $distribution @('onnx-windows', 'record', '--source-archive', $SourceArchive, '--mirror-archive', $MirrorArchive, '--cmake-archive', $CmakeArchive, '--python-archive', $PythonArchive, '--protoc-archive', $ProtocArchive, '--cmake-cache', $cache, '--output-root', $OutputRoot, '--evidence', $evidence, '--receipt', $receipt, '--validation', $validation, '--product-commit', $ExpectedProductCommit, '--cargo-lock-sha256', $ExpectedCargoLockSha256, '--builder-host', $BuilderHost, '--toolchain', $toolchain)
    Invoke-Checked 'rehash persisted ONNX receipt output' $distribution @('onnx-windows', 'verify', '--receipt', $receipt, '--output-root', $OutputRoot)
    Write-Host "ONNX_WINDOWS_BUILD_OK output=$outputDll receipt=$receipt evidence=$evidence validation=$validation"
} finally { Remove-NetworkDenies }
