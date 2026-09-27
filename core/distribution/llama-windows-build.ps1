# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Source-bound native capture, preceding receipt admission. Build tools are Unowned.
# Incomplete captures retain their fence/rules; root exit does not prove descendant cleanup.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][ValidateRange(30,14400)][int]$OverallSeconds,
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$SourceArchive,
    [Parameter(Mandatory=$true)][string]$SdkArchive,
    [Parameter(Mandatory=$true)][string]$CmakeArchive,
    [Parameter(Mandatory=$true)][string]$RunRoot,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [string]$GitRoot='C:\Program Files\Git'
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$invocationClock=[Diagnostics.Stopwatch]::StartNew()
$RepositoryRoot=(Resolve-Path -LiteralPath $RepositoryRoot).ProviderPath
if (-not [IO.Path]::GetFullPath($PSScriptRoot).Equals(
    [IO.Path]::GetFullPath((Join-Path $RepositoryRoot 'core\distribution')), [StringComparison]::OrdinalIgnoreCase)) {
    throw 'execute the driver from the exact transferred product checkout'
}
$captureHelper=Join-Path $PSScriptRoot 'rfdetr-windows-capture.ps1'
if ((Get-FileHash -LiteralPath $captureHelper -Algorithm SHA256).Hash.ToLowerInvariant() -cne
    'a4959a5aca1b346a915e87c4019e64169a153f27af2a724012eb01dca13fe605') { throw 'capture helper identity changed' }
. $captureHelper
$utf8 = [Text.UTF8Encoding]::new($false, $true)
$fence = 'C:\ProgramData\solstone\journal-win-bootstrap.lock'
$token = [Guid]::NewGuid().ToString('N')
$fenceOwned = $false
$pending = $false
$exitCode = 1
$failures = [Collections.Generic.List[string]]::new()
$rules = [Collections.Generic.List[string]]::new()
$captures = [Collections.Generic.List[object]]::new()
$records = [Collections.Generic.List[object]]::new()
$environmentChanges = @{}
$nativeNames = @('cl','link','MSBuild','rustc','cargo','ninja','cmake','ctest','vpk','signtool',
    'llama-server','glslc','vulkan-shaders-gen','asm_offset','git','bash','sh')

function Write-NewText([string]$Path, [string]$Text) {
    $bytes = $utf8.GetBytes($Text)
    $file = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
}
function Write-NewJson([string]$Path, $Value) {
    Write-NewText $Path (ConvertTo-Json -InputObject $Value -Depth 15)
}
function Digest([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "required file absent: $Path" }
    if ((Get-Item -LiteralPath $Path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse file refused: $Path" }
}
function Require-CmdPath([string]$Path) {
    if ($Path -match '["%!&|<>^\r\n]') { throw "path cannot be passed to the fixed cmd batch: $Path" }
}
function Set-BuildEnvironment([string]$Name, [string]$Value) {
    if (-not $environmentChanges.ContainsKey($Name)) {
        $environmentChanges[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
    }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}
function Invoke-Native {
    param([string]$Label, [string]$Command, [string[]]$Arguments, [string]$Cwd,
        [int]$Seconds = 120, [hashtable]$Environment = @{}, [string]$CmdArgumentLine)
    if ($Label -notmatch '^[a-z0-9-]+$') { throw 'invalid native evidence label' }
    $remaining = [int64]$OverallSeconds * 1000 - $invocationClock.ElapsedMilliseconds
    if ($remaining -le 0) { throw 'total llama invocation budget exhausted before native launch' }
    $waitMilliseconds = [int][Math]::Min([int64]$Seconds * 1000, $remaining)
    $stdout = Join-Path $logRoot "$Label.stdout"
    $stderr = Join-Path $logRoot "$Label.stderr"
    $capture = Invoke-RfdetrCapture -Command $Command -Arguments $Arguments -Cwd $Cwd `
        -StdoutPath $stdout -StderrPath $stderr -TimeoutMilliseconds $waitMilliseconds `
        -Environment $Environment -CmdArgumentLine $CmdArgumentLine
    $captures.Add($capture)
    # Preserve ownership before any evidence write can itself fail.
    if ($capture.launch_attempted -and -not $capture.completed) { $script:pending = $true }
    if ($Label -ceq 'host-fence-acquire' -and $capture.completed -and $capture.exit_code -eq 0) {
        $script:fenceOwned = $true
    }
    $detail = [ordered]@{label=$Label; argv=@($Command)+$Arguments; cwd=$Cwd;
        launch_attempted=$capture.launch_attempted; started=$capture.started; pid=$capture.pid;
        exit_code=$capture.exit_code; completed=$capture.completed; elapsed_ms=$capture.elapsed_ms;
        wait_budget_ms=$waitMilliseconds; invocation_elapsed_ms=$invocationClock.ElapsedMilliseconds;
        error=$capture.error; stdout_path="logs/$Label.stdout"; stderr_path="logs/$Label.stderr"}
    Write-NewJson (Join-Path $logRoot "$Label.execution.json") $detail
    if (-not $capture.completed) {
        throw "$Label capture incomplete: $($capture.error)"
    }
    $records.Add([ordered]@{label=$Label; argv=@($Command)+$Arguments; cwd=$Cwd;
        exit_code=$capture.exit_code; stdout_path="logs/$Label.stdout"; stderr_path="logs/$Label.stderr"})
    if ($capture.exit_code -ne 0) { throw "$Label exited $($capture.exit_code)" }
    if ((Get-Item -LiteralPath $stdout).Length -gt 67108864 -or (Get-Item -LiteralPath $stderr).Length -gt 67108864) {
        throw "$Label exceeds the recorder stream admission limit; original bytes retained"
    }
}
function Log-Text([string]$Label) { [IO.File]::ReadAllText((Join-Path $logRoot "$Label.stdout"), $utf8) }
function Assert-NoNative([string]$Label) {
    # A finite, read-only census of the release/build tools that serialize this
    # host. This is not a process-tree owner and does not reopen/kill these PIDs.
    $all = @(Get-Process -ErrorAction Stop)
    if (@($all | Where-Object { $_.Id -eq $PID }).Count -ne 1) { throw 'process census did not find its own driver' }
    $active = @($all | Where-Object { $nativeNames -contains $_.ProcessName })
    Write-NewJson (Join-Path $reportRoot "$Label.json") @($active | ForEach-Object {
        [ordered]@{name=$_.ProcessName; pid=$_.Id}
    })
    if ($active.Count -ne 0) { throw "native/release tool activity at $Label; yield the host" }
}
function Add-Deny([string]$Program, [bool]$Future = $false) {
    if (-not $Future) { Require-File $Program }
    $name = "solstone-llama-build-$token-$($rules.Count)"
    # Register intent before creation so an uncertain creation result also has
    # a durable cleanup name. Only these request-owned names may be removed.
    $rules.Add($name)
    Write-NewJson (Join-Path $reportRoot "$name.json") ([ordered]@{name=$name;program=$Program})
    New-NetFirewallRule -Name $name -DisplayName $name -Direction Outbound -Action Block `
        -Program $Program -Profile Any -Enabled True -ErrorAction Stop | Out-Null
    $rule = Get-NetFirewallRule -Name $name -PolicyStore ActiveStore -ErrorAction Stop
    if ($rule.Enabled -ne 'True' -or $rule.Direction -ne 'Outbound' -or $rule.Action -ne 'Block') {
        throw "network deny is not active: $name"
    }
}
function Remove-Denies {
    foreach ($name in $rules) {
        $existing = @(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name })
        if ($existing.Count -ne 0) { Remove-NetFirewallRule -Name $name -ErrorAction Stop }
        if (@(Get-NetFirewallRule -ErrorAction Stop | Where-Object { $_.Name -ceq $name }).Count -ne 0) {
            throw "network deny removal unconfirmed: $name"
        }
    }
    $rules.Clear()
}
function Check-Product([string]$Phase) {
    Invoke-Native "product-head-$Phase" $git @('rev-parse','HEAD') $RepositoryRoot
    if ((Log-Text "product-head-$Phase").Trim() -cne $ExpectedProductCommit) { throw 'product source commit mismatch' }
    Invoke-Native "product-status-$Phase" $git @('status','--porcelain=v1','--untracked-files=all','--ignore-submodules=none') $RepositoryRoot
    if ((Log-Text "product-status-$Phase").Length -ne 0) { throw 'product checkout is dirty' }
    if ((Digest (Join-Path $RepositoryRoot 'core\Cargo.lock')) -cne $ExpectedCargoLockSha256) { throw 'product lockfile mismatch' }
}


function Observe-Denies([string]$Phase) {
    $profiles=@(Get-NetFirewallProfile -PolicyStore ActiveStore -ErrorAction Stop)
    if ($profiles.Count -ne 3 -or @($profiles | Where-Object { -not $_.Enabled }).Count) {
        throw 'all firewall profiles must already be enabled; this driver does not change host policy'
    }
    $observations=@(foreach ($name in $rules) {
        $rule=Get-NetFirewallRule -Name $name -PolicyStore ActiveStore -ErrorAction Stop
        $app=@($rule | Get-NetFirewallApplicationFilter -ErrorAction Stop)
        $intent=Get-Content -LiteralPath (Join-Path $reportRoot "$name.json") -Raw | ConvertFrom-Json
        if ($rule.Enabled -ne 'True' -or $rule.Direction -ne 'Outbound' -or $rule.Action -ne 'Block' -or
            $rule.Profile.ToString() -ne 'Any' -or $app.Count -ne 1 -or
            -not $app[0].Program.Equals($intent.program,[StringComparison]::OrdinalIgnoreCase)) {
            throw "effective program deny disagrees with intent: $name"
        }
        [ordered]@{name=$name;program=$app[0].Program;enabled=$rule.Enabled.ToString();
            direction=$rule.Direction.ToString();action=$rule.Action.ToString();profile=$rule.Profile.ToString()}
    })
    Write-NewJson (Join-Path $reportRoot "denial-$Phase.json") ([ordered]@{
        utc=[DateTimeOffset]::UtcNow.ToString('o');
        profiles=@($profiles | ForEach-Object {[ordered]@{name=$_.Name;enabled=$_.Enabled}});
        rules=$observations})
}
function Source-Census([string]$Phase) {
    $rows=@(Get-ChildItem -LiteralPath $source -Recurse -File | Sort-Object FullName | ForEach-Object {
        Require-File $_.FullName
        [ordered]@{path=$_.FullName.Substring($source.Length+1).Replace('\','/');size=$_.Length;sha256=(Digest $_.FullName)}
    })
    Write-NewJson (Join-Path $reportRoot "source-$Phase.json") $rows
}
function Sdk-State {
    $values=[ordered]@{}
    foreach ($scope in @('Machine','User')) {
        foreach ($name in @('PATH','VULKAN_SDK','VK_SDK_PATH')) {
            $values["$scope/$name"]=[Environment]::GetEnvironmentVariable($name,$scope)
        }
    }
    foreach ($path in @('C:\Windows\System32\vulkan-1.dll','C:\Windows\SysWOW64\vulkan-1.dll')) {
        $values[$path]=if(Test-Path -LiteralPath $path){Digest $path}else{'absent'}
    }
    return $values
}
$RunRoot=[IO.Path]::GetFullPath($RunRoot)
if (Test-Path -LiteralPath $RunRoot) { throw 'run root exists; use a fresh isolated path' }
if ($RunRoot.StartsWith($RepositoryRoot.TrimEnd('\')+'\',[StringComparison]::OrdinalIgnoreCase)) {
    throw 'run root must be outside product checkout'
}
New-Item -ItemType Directory -Path $RunRoot -ErrorAction Stop | Out-Null
$reportRoot=Join-Path $RunRoot 'report'
$logRoot=Join-Path $reportRoot 'logs'
New-Item -ItemType Directory -Path $reportRoot,$logRoot -ErrorAction Stop | Out-Null
try {
    foreach ($name in @('CL','_CL_','LINK','_LINK_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS',
        'CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_INSTANCE','CMAKE_GENERATOR_PLATFORM',
        'CMAKE_GENERATOR_TOOLSET','CMAKE_PREFIX_PATH')) {
        if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name))) {
            throw "inherited build override refused: $name"
        }
    }
    Assert-NoNative 'host-before-fence'
    $vms=@(Get-VM -ErrorAction Stop)
    Write-NewJson (Join-Path $reportRoot 'vms-before.json') @($vms | ForEach-Object {
        [ordered]@{name=$_.Name;state=$_.State.ToString();memory_assigned=$_.MemoryAssigned}
    })
    if (@($vms | Where-Object {$_.State.ToString() -ne 'Off'}).Count) { throw 'wait for VM handback' }
    $cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    Require-CmdPath $fence
    Invoke-Native 'host-fence-acquire' $cmd @('/d','/s','/c',"mkdir `"$fence`"") $RunRoot 30 @{} `
        ('/d /s /c "mkdir "{0}""' -f $fence)
    Write-NewText (Join-Path $fence 'owner.token') $token
    Write-NewText (Join-Path $fence 'held.marker') ([DateTimeOffset]::UtcNow.ToUnixTimeSeconds().ToString())
    Assert-NoNative 'host-after-fence'
    $git=Join-Path $GitRoot 'cmd\git.exe'
    foreach ($path in @($SourceArchive,$SdkArchive,$CmakeArchive,$git)) {Require-File $path}
    if ((Digest $SourceArchive) -cne 'ea613b46d078609bdac8dc05f99959bd38e965e7a5abadf58eab023c83203828') {throw 'source archive digest mismatch'}
    if ((Digest $SdkArchive) -cne '81f474711e9042f4cd22b31b2f7a8870db2e428b21586fb43dd80150be97310d') {throw 'SDK archive digest mismatch'}
    if ((Digest $CmakeArchive) -cne '0c4baa40f28b3f8225eb3fdf6946c987b4fe901403b4eaf2fbbd9378100aaa0c') {throw 'CMake archive digest mismatch'}
    foreach ($entry in @(Get-ChildItem Env:)) {
        if ($entry.Name.StartsWith('GIT_',[StringComparison]::OrdinalIgnoreCase)) {Set-BuildEnvironment $entry.Name $null}
    }
    Set-BuildEnvironment 'GIT_CONFIG_NOSYSTEM' '1'
    Set-BuildEnvironment 'GIT_CONFIG_GLOBAL' 'NUL'
    Set-BuildEnvironment 'GIT_NO_REPLACE_OBJECTS' '1'
    Set-BuildEnvironment 'GIT_TERMINAL_PROMPT' '0'
    Set-BuildEnvironment 'MSBUILDDISABLENODEREUSE' '1'
    Set-BuildEnvironment 'CMAKE_BUILD_PARALLEL_LEVEL' '1'
    Check-Product 'before'
    $cmakeRoot=Join-Path $RunRoot 'cmake'
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive,$cmakeRoot)
    $cmake=Join-Path $cmakeRoot 'cmake-3.31.12-windows-x86_64\bin\cmake.exe'
    Require-File $cmake
    $source=Join-Path $RunRoot 'source'
    New-Item -ItemType Directory -Path $source | Out-Null
    # The pinned CMake extractor preserves upstream Unicode filenames; the
    # ambient Windows bsdtar can reject them under the SSH process locale.
    Invoke-Native 'extract-source' $cmake @('-E','tar','xzf',$SourceArchive) $source 120
    Source-Census 'original'
    $manifest=Get-Content -LiteralPath (Join-Path $source 'source-manifest.json') -Raw | ConvertFrom-Json
    $actual=@(Get-ChildItem -LiteralPath $source -Recurse -File)
    if ($actual.Count -ne @($manifest.members).Count+1) {throw 'source member census mismatch'}
    foreach ($member in $manifest.members) {
        $path=Join-Path $source $member.path
        Require-File $path
        if ((Get-Item -LiteralPath $path).Length -ne $member.bytes -or (Digest $path) -cne $member.sha256) {throw "source member mismatch: $($member.path)"}
    }
    $llama=Join-Path $source 'llama'
    $patch=Join-Path $source 'patches\0001-bounded-shader-build.patch'
    Invoke-Native 'check-shader-patch' $git @('apply','--check',$patch) $llama
    Invoke-Native 'apply-shader-patch' $git @('apply',$patch) $llama
    Source-Census 'patched'
    # A positive transfer control precedes network-denied build work. Its negative
    # counterpart uses the same binary, URL and pinned bytes after effective-rule readback.
    $control=Join-Path $RunRoot 'network-control.cmake'
    Write-NewText $control @'
file(DOWNLOAD "https://raw.githubusercontent.com/ggml-org/llama.cpp/571d0d540df04f25298d0e159e520d9fc62ed121/CMakeLists.txt" "${OUTPUT}" TIMEOUT 20 TLS_VERIFY ON STATUS result)
list(GET result 0 code)
message(STATUS "transfer-status=${result}")
if(EXPECT_DENIED)
  if(code EQUAL 0)
    message(FATAL_ERROR "denied transfer unexpectedly succeeded")
  endif()
else()
  if(NOT code EQUAL 0)
    message(FATAL_ERROR "positive transfer control failed")
  endif()
  file(SHA256 "${OUTPUT}" digest)
  if(NOT digest STREQUAL "272b66f1e71b0be5bb8a59b2b92dc5f97f1292aba2a72f86c9cb3673362374cc")
    message(FATAL_ERROR "positive transfer bytes differ")
  endif()
endif()
'@
    Invoke-Native 'network-positive' $cmake @('-DEXPECT_DENIED=OFF',"-DOUTPUT=$RunRoot/network-positive.txt",'-P',$control) $RunRoot 40
    Add-Deny $cmake
    Add-Deny $SdkArchive
    Observe-Denies 'sdk-before'
    $sdk=Join-Path $RunRoot 'sdk'
    Write-NewJson (Join-Path $reportRoot 'sdk-state-before.json') (Sdk-State)
    Invoke-Native 'sdk-copy-only' $SdkArchive @('--root',$sdk,'--accept-licenses','--default-answer','--confirm-command','install','copy_only=1') $RunRoot 600
    Write-NewJson (Join-Path $reportRoot 'sdk-state-after.json') (Sdk-State)
    if ((Digest (Join-Path $reportRoot 'sdk-state-before.json')) -cne (Digest (Join-Path $reportRoot 'sdk-state-after.json'))) {throw 'SDK changed monitored machine state'}
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    Invoke-Native 'vswhere' $vswhere @('-latest','-products','*','-requires','Microsoft.VisualStudio.Component.VC.Tools.x86.x64','-property','installationPath') $RunRoot
    $vs=(Log-Text 'vswhere').Trim()
    if (-not $vs -or $vs.Contains("`n")) {throw 'VS discovery did not select one installation'}
    $vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    Require-CmdPath $vcvars
    $batch=Join-Path $RunRoot 'build-environment.cmd'
    Require-CmdPath $batch
    Write-NewText $batch ("@echo off`r`ncall `"$vcvars`" x64`r`nif errorlevel 1 exit /b %errorlevel%`r`nset PATH`r`nset INCLUDE`r`nset LIB`r`nset VCTools`r`nset WindowsSDK`r`nset VSINSTALLDIR`r`nset VCINSTALLDIR`r`nexit /b 0`r`n")
    Invoke-Native 'build-environment' $cmd @('/d','/s','/c',"`"$batch`"") $RunRoot 60 @{} ('/d /s /c ""{0}""' -f $batch)
    foreach ($line in (Log-Text 'build-environment') -split "`r?`n") {
        if ($line -match '^(PATH|PATHEXT|INCLUDE|LIB|LIBPATH|VCToolsInstallDir|VCToolsVersion|VCToolsRedistDir|WindowsSdkDir|WindowsSDKVersion|WindowsSDKLibVersion|WindowsSdkBinPath|WindowsSdkVerBinPath|VSINSTALLDIR|VCINSTALLDIR|VSCMD_ARG_TGT_ARCH)=(.*)$') {
            Set-BuildEnvironment $Matches[1] $Matches[2]
        }
    }
    Set-BuildEnvironment 'VULKAN_SDK' $sdk
    $header=Join-Path $sdk 'Include\vulkan\vulkan_core.h'
    if ([IO.File]::ReadAllText($header) -notmatch '(?m)^#define VK_HEADER_VERSION 357\r?$') {throw 'SDK headers version mismatch'}
    $spirv=@(Get-ChildItem -LiteralPath $sdk -Recurse -File -Filter SPIRV-HeadersConfig.cmake)
    if ($spirv.Count -ne 1) {throw 'SPIRV header package census mismatch'}
    $cl=(Get-Command cl.exe -CommandType Application).Source
    $link=(Get-Command link.exe -CommandType Application).Source
    $bin=Split-Path -Parent $cl
    $build=Join-Path $RunRoot 'engine-build'
    $loaderBuild=Join-Path $RunRoot 'loader-build'
    foreach ($program in @($cl,$link,(Join-Path $bin 'ml64.exe'),$git,
        (Join-Path $vs 'MSBuild\Current\Bin\MSBuild.exe'),(Join-Path $vs 'MSBuild\Current\Bin\amd64\MSBuild.exe'),
        (Join-Path $sdk 'Bin\glslc.exe'),(Join-Path $GitRoot 'mingw64\bin\git.exe'))) {Add-Deny $program}
    Add-Deny (Join-Path $build 'Release\vulkan-shaders-gen.exe') $true
    Add-Deny (Join-Path $loaderBuild 'upstream\loader\Release\asm_offset.exe') $true
    Observe-Denies 'before'
    Invoke-Native 'network-negative-before' $cmake @('-DEXPECT_DENIED=ON',"-DOUTPUT=$RunRoot/network-negative-before.txt",'-P',$control) $RunRoot 40
    $toolset='host=x64,version='+$env:VCToolsVersion.TrimEnd('\')
    $common=@('-G','Visual Studio 17 2022','-A','x64','-T',$toolset,"-DCMAKE_GENERATOR_INSTANCE=$vs")
    $engineArgs=@('-S',$llama,'-B',$build)+$common+@(
        '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreadedDLL','-DBUILD_SHARED_LIBS=OFF','-DGGML_BACKEND_DL=OFF',
        '-DGGML_CPU_ALL_VARIANTS=OFF','-DGGML_CPU=ON','-DGGML_VULKAN=ON','-DGGML_NATIVE=OFF','-DGGML_CCACHE=OFF','-DGGML_LLAMAFILE=OFF',
        '-DGGML_CUDA=OFF','-DGGML_HIP=OFF','-DGGML_METAL=OFF','-DGGML_OPENCL=OFF','-DGGML_SYCL=OFF','-DGGML_RPC=OFF','-DGGML_BLAS=OFF',
        '-DGGML_AVX=ON','-DGGML_AVX2=ON','-DGGML_AVX512=OFF','-DGGML_FMA=ON','-DGGML_F16C=ON','-DGGML_BMI2=ON',
        '-DLLAMA_BUILD_COMMON=ON','-DLLAMA_BUILD_TOOLS=ON','-DLLAMA_BUILD_SERVER=ON','-DLLAMA_BUILD_APP=OFF','-DLLAMA_BUILD_TESTS=OFF','-DLLAMA_BUILD_EXAMPLES=OFF',
        '-DLLAMA_BUILD_UI=OFF','-DLLAMA_USE_PREBUILT_UI=OFF','-DLLAMA_OPENSSL=OFF','-DLLAMA_BUILD_BORINGSSL=OFF','-DLLAMA_BUILD_LIBRESSL=OFF','-DLLAMA_LLGUIDANCE=OFF',
        "-DVulkan_GLSLC_EXECUTABLE=$sdk/Bin/glslc.exe","-DVulkan_INCLUDE_DIR=$sdk/Include","-DVulkan_LIBRARY=$sdk/Lib/vulkan-1.lib","-DSPIRV-Headers_DIR=$($spirv[0].DirectoryName)")
    Invoke-Native 'engine-configure' $cmake $engineArgs $source 600
    Invoke-Native 'engine-build' $cmake @('--build',$build,'--config','Release','--target','llama-server','--parallel','1') $source 7200
    $loaderArgs=@('-S',(Join-Path $source 'loader-wrapper'),'-B',$loaderBuild)+$common+@(
        "-DVulkanHeaders_DIR=$source/vulkan-headers",'-DUPDATE_DEPS=OFF','-DBUILD_TESTS=OFF','-DLOADER_CODEGEN=OFF',
        '-DCODE_COVERAGE=OFF','-DUSE_MASM=ON','-DUSE_GAS=OFF','-DENABLE_WIN10_ONECORE=OFF','-DLOADER_USE_UNSAFE_FILE_SEARCH=OFF')
    Invoke-Native 'loader-configure' $cmake $loaderArgs $source 600
    Invoke-Native 'loader-build' $cmake @('--build',$loaderBuild,'--config','Release','--target','vulkan','--parallel','1') $source 1800
    Observe-Denies 'after'
    Invoke-Native 'network-negative-after' $cmake @('-DEXPECT_DENIED=ON',"-DOUTPUT=$RunRoot/network-negative-after.txt",'-P',$control) $RunRoot 40
    Source-Census 'after'
    if ((Digest (Join-Path $reportRoot 'source-patched.json')) -cne (Digest (Join-Path $reportRoot 'source-after.json'))) {throw 'source changed during build'}
    Check-Product 'after'
    $output=Join-Path $RunRoot 'output\bin'
    New-Item -ItemType Directory -Path $output | Out-Null
    [IO.File]::Copy((Join-Path $build 'bin\Release\llama-server.exe'),(Join-Path $output 'llama-server.exe'),$false)
    [IO.File]::Copy((Join-Path $loaderBuild 'upstream\loader\Release\vulkan-1.dll'),(Join-Path $output 'vulkan-1.dll'),$false)
    $dumpbin=Join-Path $bin 'dumpbin.exe'
    Invoke-Native 'engine-imports' $dumpbin @('/imports',(Join-Path $output 'llama-server.exe')) $output
    Invoke-Native 'loader-imports' $dumpbin @('/imports',(Join-Path $output 'vulkan-1.dll')) $output
    Write-NewJson (Join-Path $reportRoot 'outputs.json') @(Get-ChildItem -LiteralPath $output -File | ForEach-Object {
        [ordered]@{path=('bin/'+$_.Name);bytes=$_.Length;sha256=(Digest $_.FullName)}
    })
    Write-NewJson (Join-Path $reportRoot 'subprocess-evidence.json') @($records.ToArray())
    Write-NewJson (Join-Path $reportRoot 'inputs.json') ([ordered]@{
        product_commit=$ExpectedProductCommit;cargo_lock_sha256=$ExpectedCargoLockSha256;
        driver_sha256=(Digest $PSCommandPath);capture_helper_sha256=(Digest $captureHelper);
        source_sha256=(Digest $SourceArchive);sdk_sha256=(Digest $SdkArchive);cmake_sha256=(Digest $CmakeArchive);
        tools=@(@($cmake,$cl,$link,(Join-Path $bin 'ml64.exe'),(Join-Path $sdk 'Bin\glslc.exe')) | ForEach-Object {
            [ordered]@{path=$_;sha256=(Digest $_)}
        });kind='native capture; receipt admission pending'})
    $exitCode=0
} catch {
    $failures.Add($_.ToString()); $failures.Add($_.ScriptStackTrace)
} finally {
    if ($fenceOwned -and -not $pending) {
        try {Assert-NoNative 'host-terminal'} catch {$pending=$true;$failures.Add($_.ToString())}
    }
    if ($fenceOwned -and -not $pending) {
        try {
            Remove-Denies
            if ([IO.File]::ReadAllText((Join-Path $fence 'owner.token')) -cne $token) {throw 'fence ownership changed'}
            $unexpected=@(Get-ChildItem -LiteralPath $fence -Force -ErrorAction Stop | Where-Object {$_.Name -notin @('owner.token','held.marker')})
            if ($unexpected.Count) {throw 'unexpected fence contents; retained'}
            Remove-Item -LiteralPath (Join-Path $fence 'owner.token'),(Join-Path $fence 'held.marker') -ErrorAction Stop
            Remove-Item -LiteralPath $fence -ErrorAction Stop
            if(Test-Path -LiteralPath $fence){throw 'fence removal unconfirmed'}
            $fenceOwned=$false
        } catch {$pending=$true;$failures.Add($_.ToString())}
    }
    if($pending -or $failures.Count){$exitCode=1}
    foreach($name in $environmentChanges.Keys){[Environment]::SetEnvironmentVariable($name,$environmentChanges[$name],'Process')}
    Write-NewJson (Join-Path $reportRoot 'execution.json') ([ordered]@{
        exit_code=$exitCode;pending_reconciliation=$pending;elapsed_ms=$invocationClock.ElapsedMilliseconds;
        fence_retained=$fenceOwned;owner_token=$token;firewall_rules_retained=@($rules.ToArray());
        failures=@($failures.ToArray());process_ownership='Unowned';whole_tree_quiescence_proven=$false;
        incomplete_logs_are_original_prefixes=$pending;kind='native capture; not an admitted receipt'})
    Write-NewText (Join-Path $reportRoot 'exit.txt') $exitCode.ToString()
}
Write-Output "LLAMA_WINDOWS_CAPTURE_TERMINAL exit=$exitCode pending=$pending report=$reportRoot"
exit $exitCode
