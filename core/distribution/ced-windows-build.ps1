# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# The native half of the controlled CED Windows build. The POSIX driver has
# already materialized source and acquired CMake by digest before transferring
# them here. This script refuses inherited work/output roots, verifies those
# bytes before extraction, then blocks outbound traffic for every executable
# the CMake/MSVC build invokes. It builds and records a pre-signing DLL only;
# it does not stage, sign, package, load, or advertise CED capability.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$RepositoryRoot,
    [Parameter(Mandatory = $true)]
    [string]$SourceArchive,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$SourceSha256,
    [Parameter(Mandatory = $true)]
    [ValidateRange(1, [Int64]::MaxValue)]
    [Int64]$SourceSize,
    [Parameter(Mandatory = $true)]
    [string]$CmakeArchive,
    [Parameter(Mandatory = $true)]
    [string]$WorkRoot,
    [Parameter(Mandatory = $true)]
    [string]$OutputRoot,
    [Parameter(Mandatory = $true)]
    [string]$ReportRoot,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{40}$')]
    [string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9a-f]{64}$')]
    [string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)]
    [string]$BuilderHost
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$firewallRules = [System.Collections.Generic.List[string]]::new()

function Require-File([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "$Label is not a regular file: $Path"
    }
}

function Require-NewRoot([string]$Path, [string]$Label) {
    if (Test-Path -LiteralPath $Path) {
        throw "$Label already exists and will not be reused: $Path"
    }
    New-Item -ItemType Directory -Path $Path -Force | Out-Null
}

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-Checked([string]$Label, [string]$Command, [string[]]$Arguments) {
    Write-Host "=== $Label ==="
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

function Add-NetworkDeny([string]$Program, [int]$Ordinal) {
    Require-File $Program "network-deny program"
    $name = "solstone-ced-build-$PID-$Ordinal-$([Guid]::NewGuid().ToString('N'))"
    New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $Program -Profile Any | Out-Null
    $firewallRules.Add($name)
}

function Remove-NetworkDenies {
    foreach ($name in $firewallRules) {
        Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue
    }
}

try {
    Require-File $SourceArchive 'CED source archive'
    Require-File $CmakeArchive 'CMake archive'
    Require-NewRoot $WorkRoot 'CED work root'
    Require-NewRoot $OutputRoot 'CED output root'
    Require-NewRoot $ReportRoot 'CED report root'

    if ((Get-Item -LiteralPath $SourceArchive).Length -ne $SourceSize) {
        throw "CED source archive size mismatch"
    }
    if ((Get-Sha256 $SourceArchive) -ne $SourceSha256) {
        throw "CED source archive SHA-256 mismatch"
    }

    Set-Location -LiteralPath $RepositoryRoot
    $observedCommit = (& git rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $observedCommit -ne $ExpectedProductCommit) {
        throw "repository commit does not match the source-bound driver value"
    }
    $dirty = & git status --porcelain=v1 --untracked-files=all --ignore-submodules=none
    if ($LASTEXITCODE -ne 0 -or $null -ne $dirty -and $dirty.Count -ne 0) {
        throw "repository is not a clean transferred checkout"
    }
    $observedLock = Get-Sha256 (Join-Path $RepositoryRoot 'core/Cargo.lock')
    if ($observedLock -ne $ExpectedCargoLockSha256) {
        throw "repository Cargo.lock does not match the source-bound driver value"
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    Require-File $vswhere 'vswhere'
    $vsInstall = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if ([string]::IsNullOrWhiteSpace($vsInstall)) {
        throw 'Visual Studio Build Tools with VC.Tools.x86.x64 are required'
    }
    $vcvars = Join-Path $vsInstall 'VC/Auxiliary/Build/vcvarsall.bat'
    Require-File $vcvars 'vcvarsall'
    $msvcRoot = Get-ChildItem -LiteralPath (Join-Path $vsInstall 'VC/Tools/MSVC') -Directory |
        Sort-Object Name -Descending |
        Select-Object -First 1
    if ($null -eq $msvcRoot) {
        throw 'MSVC tools directory is absent'
    }
    $toolBin = Join-Path $msvcRoot.FullName 'bin/Hostx64/x64'
    $cl = Join-Path $toolBin 'cl.exe'
    $link = Join-Path $toolBin 'link.exe'
    $msbuild = Join-Path $vsInstall 'MSBuild/Current/Bin/MSBuild.exe'
    $git = (Get-Command git.exe -ErrorAction Stop).Source
    Require-File $cl 'MSVC cl.exe'
    Require-File $link 'MSVC link.exe'
    Require-File $msbuild 'MSBuild.exe'

    $cargoBuild = 'call "{0}" x64 >nul && cargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked --offline' -f $vcvars
    Invoke-Checked 'offline build of the CED receipt recorder' 'cmd.exe' @('/d', '/s', '/c', $cargoBuild)
    $distribution = Join-Path $RepositoryRoot 'core/target/debug/solstone-distribution.exe'
    Require-File $distribution 'CED receipt recorder'

    Invoke-Checked 'verify transferred CED and CMake inputs' $distribution @(
        'ced-windows', 'verify-inputs',
        '--source-archive', $SourceArchive,
        '--cmake-archive', $CmakeArchive
    )

    $sourceRoot = Join-Path $WorkRoot 'source'
    $cmakeRoot = Join-Path $WorkRoot 'cmake'
    $buildRoot = Join-Path $WorkRoot 'build'
    New-Item -ItemType Directory -Path $sourceRoot, $cmakeRoot | Out-Null
    Invoke-Checked 'extract verified CED source archive' 'tar.exe' @('-xzf', $SourceArchive, '-C', $sourceRoot)
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive, $cmakeRoot)
    $cmakeCandidates = @(Get-ChildItem -LiteralPath $cmakeRoot -Filter cmake.exe -Recurse -File)
    if ($cmakeCandidates.Count -ne 1) {
        throw "CMake archive produced $($cmakeCandidates.Count) cmake.exe files; expected exactly one"
    }
    $cmake = $cmakeCandidates[0].FullName

    $ordinal = 1
    foreach ($program in @($cmake, $msbuild, $cl, $link, $git) | Select-Object -Unique) {
        Add-NetworkDeny $program $ordinal
        $ordinal += 1
    }
    Write-Host 'CED_WINDOWS_NETWORK_DENY=firewall-outbound-block-for-cmake-msbuild-cl-link-git'

    $cmakeArguments = @(
        '-S', $sourceRoot,
        '-B', $buildRoot,
        '-G', 'Visual Studio 17 2022',
        '-A', 'x64',
        '-DCED_SHARED=ON',
        '-DCED_BUILD_CLI=OFF',
        '-DCED_BUILD_TESTS=OFF',
        '-DCED_GGML_CUDA=OFF',
        '-DCED_GGML_METAL=OFF',
        '-DCED_GGML_VULKAN=OFF',
        '-DCED_GGML_HIP=OFF',
        '-DBUILD_SHARED_LIBS=OFF',
        '-DGGML_NATIVE=OFF',
        '-DGGML_LLAMAFILE=OFF',
        '-DGGML_OPENCL=OFF',
        '-DGGML_BACKEND_DL=OFF',
        '-DGGML_OPENMP=ON'
    )
    Invoke-Checked 'configure CED Windows Release CMake build' $cmake $cmakeArguments
    Invoke-Checked 'build CED Windows Release DLL' $cmake @('--build', $buildRoot, '--config', 'Release', '--target', 'ced', '--parallel', '2')

    $cache = Join-Path $buildRoot 'CMakeCache.txt'
    Require-File $cache 'CED CMake cache'
    $cacheText = Get-Content -LiteralPath $cache -Raw
    $requiredCache = @{
        'CED_SHARED' = 'ON'; 'CED_BUILD_CLI' = 'OFF'; 'CED_BUILD_TESTS' = 'OFF';
        'CED_GGML_CUDA' = 'OFF'; 'CED_GGML_METAL' = 'OFF'; 'CED_GGML_VULKAN' = 'OFF';
        'CED_GGML_HIP' = 'OFF'; 'BUILD_SHARED_LIBS' = 'OFF'; 'GGML_NATIVE' = 'OFF';
        'GGML_LLAMAFILE' = 'OFF'; 'GGML_OPENCL' = 'OFF'; 'GGML_BACKEND_DL' = 'OFF';
        'GGML_OPENMP' = 'ON'
    }
    foreach ($name in $requiredCache.Keys) {
        $cachePattern = "(?m)^$([regex]::Escape($name))(?::[A-Z_]+)?=$([regex]::Escape($requiredCache[$name]))\r?$"
        if ($cacheText -notmatch $cachePattern) {
            throw "CMake cache does not retain $name=$($requiredCache[$name])"
        }
    }

    $dllCandidates = @(Get-ChildItem -LiteralPath $buildRoot -Filter ced.dll -Recurse -File)
    if ($dllCandidates.Count -ne 1) {
        throw "CED build produced $($dllCandidates.Count) ced.dll files; expected exactly one"
    }
    $outputBin = Join-Path $OutputRoot 'bin'
    New-Item -ItemType Directory -Path $outputBin | Out-Null
    $outputDll = Join-Path $outputBin 'ced.dll'
    Copy-Item -LiteralPath $dllCandidates[0].FullName -Destination $outputDll

    $dumpbin = Join-Path $toolBin 'dumpbin.exe'
    Require-File $dumpbin 'dumpbin.exe'
    $expectedExports = @(
        'ced_capi_abi_version', 'ced_capi_load', 'ced_capi_free',
        'ced_capi_last_error', 'ced_capi_classify_pcm_json', 'ced_capi_free_string'
    ) | Sort-Object
    $exportText = (& $dumpbin /exports $outputDll 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw 'dumpbin /exports failed'
    }
    $actualExports = [regex]::Matches($exportText, '(?m)^\s*\d+\s+[0-9A-F]+\s+[0-9A-F]+\s+([A-Za-z_][A-Za-z0-9_]*)\s*$') |
        ForEach-Object { $_.Groups[1].Value } |
        Sort-Object -Unique
    if (@(Compare-Object -ReferenceObject $expectedExports -DifferenceObject $actualExports).Count -ne 0) {
        throw "CED DLL exports are not the reviewed six-symbol ABI: $($actualExports -join ', ')"
    }
    $dependentsText = (& $dumpbin /dependents $outputDll 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw 'dumpbin /dependents failed'
    }
    $forbiddenImports = @($dependentsText -split "`r?`n" | Where-Object {
        $_ -match '(?i)(cuda|nvcuda|cudart|cublas|cudnn|nvrtc|vulkan|opencl)'
    })
    if ($forbiddenImports.Count -ne 0) {
        throw "CED DLL has forbidden GPU imports: $($forbiddenImports -join '; ')"
    }

    $validation = Join-Path $ReportRoot 'ced-build-validation.log'
    @(
        'schema=solstone.ced-windows-validation.v1',
        "product_commit=$ExpectedProductCommit",
        "cargo_lock_sha256=$ExpectedCargoLockSha256",
        "source_sha256=$SourceSha256",
        "source_size=$SourceSize",
        "cmake_sha256=$(Get-Sha256 $CmakeArchive)",
        "cmake_cache_sha256=$(Get-Sha256 $cache)",
        'network_access=denied-by-firewall-for-cmake-msbuild-cl-link-git',
        "exports=$($actualExports -join ',')",
        'forbidden_gpu_imports=none',
        "output_sha256=$(Get-Sha256 $outputDll)"
    ) | Set-Content -LiteralPath $validation -Encoding utf8

    $cmakeVersion = (& $cmake --version | Select-Object -First 1).Trim()
    $clVersion = (& $cl 2>&1 | Select-Object -First 1).ToString().Trim()
    $toolchain = "$cmakeVersion; $clVersion"
    $evidence = Join-Path $ReportRoot 'ced-build-evidence.json'
    $receipt = Join-Path $ReportRoot 'ced-build-receipt.json'
    Invoke-Checked 'persist CED controlled-build receipt' $distribution @(
        'ced-windows', 'record',
        '--source-archive', $SourceArchive,
        '--cmake-archive', $CmakeArchive,
        '--cmake-cache', $cache,
        '--output-root', $OutputRoot,
        '--evidence', $evidence,
        '--receipt', $receipt,
        '--validation', $validation,
        '--product-commit', $ExpectedProductCommit,
        '--cargo-lock-sha256', $ExpectedCargoLockSha256,
        '--builder-host', $BuilderHost,
        '--toolchain', $toolchain
    )
    Invoke-Checked 'rehash and re-census persisted CED receipt output' $distribution @(
        'ced-windows', 'verify',
        '--receipt', $receipt,
        '--output-root', $OutputRoot
    )
    Write-Host "CED_WINDOWS_BUILD_OK output=$outputDll receipt=$receipt evidence=$evidence validation=$validation"
}
finally {
    Remove-NetworkDenies
}
