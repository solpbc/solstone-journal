# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Controlled x64-hosted ARM64 CPU/OpenMP/Vulkan producer. Host tools are unshipped.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$SourceArchive,
    [Parameter(Mandatory=$true)][string]$SdkArchive,
    [Parameter(Mandatory=$true)][string]$OpenMpInputs,
    [Parameter(Mandatory=$true)][string]$WorkRoot,
    [Parameter(Mandatory=$true)][string]$CaptureRoot,
    [Parameter(Mandatory=$true)][string]$ReportRoot,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory=$true)][string]$BuilderHost
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$utf8=[Text.UTF8Encoding]::new($false,$true)
$rules=[Collections.Generic.List[string]]::new()
$programs=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
$commands=[Collections.Generic.List[object]]::new()
$ordinal=0
. (Join-Path $PSScriptRoot 'windows-git-programs.ps1')
. (Join-Path $PSScriptRoot 'ffmpeg-windows-arm64-tools.ps1')

function Full-Path([string]$Value) {
    if($Value -notmatch '^[A-Za-z]:[\\/]' -or $Value -match '["%!?&|<>^\r\n]' -or $Value.Contains([char]0) -or $Value -match '(^|[\\/])\.\.([\\/]|$)') {throw "Ordinary absolute build path required: $Value"}
    return [IO.Path]::GetFullPath($Value).TrimEnd('\','/')
}
function Sha([string]$File) {return (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant()}
# ReadAllText drops a BOM. Compiler metadata is admitted as its original UTF-8 bytes.
function Raw-Text([string]$File) {return $utf8.GetString([IO.File]::ReadAllBytes($File))}
function Json-File([string]$File,[object]$Value) {[IO.File]::WriteAllText($File,($Value|ConvertTo-Json -Depth 16),$utf8)}
function New-Root([string]$Directory) {
    if(Test-Path -LiteralPath $Directory){throw "Fresh Llama root required: $Directory"}
    New-Item -ItemType Directory -Path $Directory|Out-Null
}
function Identity([string]$File,[string]$Label) {
    $item=Get-Item -LiteralPath $File
    if($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)){throw "Regular tool/input required: $File"}
    return [pscustomobject]@{label=$Label;size=[long]$item.Length;sha256=(Sha $File)}
}
function Host-Pe([string]$File) {
    $bytes=[IO.File]::ReadAllBytes($File)
    if($bytes.Length -lt 64 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a){throw "Host program is not PE: $File"}
    $at=[BitConverter]::ToInt32($bytes,0x3c)
    if($at -lt 0 -or $at+24 -gt $bytes.Length -or [BitConverter]::ToUInt32($bytes,$at) -ne 0x4550 -or [BitConverter]::ToUInt16($bytes,$at+4) -ne 0x8664){throw "Host program is not classic AMD64: $File"}
}
function Run([string]$Program,[string[]]$Arguments,[string]$Label,[string]$Log='') {
    $null=Identity $Program 'native-program'
    if(-not $Log){$script:ordinal+=1;$Log=Join-Path $ReportRoot ('command-{0:D3}.log' -f $script:ordinal)}
    if(Test-Path -LiteralPath $Log){throw "Original command log already exists: $Log"}
    $global:LASTEXITCODE=$null;$saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    try{& $Program @Arguments *> $Log;$code=$global:LASTEXITCODE}finally{$ErrorActionPreference=$saved}
    if($null -eq $code -or $code -ne 0){throw "$Label exited ${code}; original log: $Log"}
    return $Log
}
function Build-Step([string]$Label,[string]$ProgramLabel,[string[]]$Arguments) {
    Set-Location $WorkRoot
    $log=Run $programs[$ProgramLabel] $Arguments $Label (Join-Path $CaptureRoot ('logs\'+$Label+'.log'))
    $commands.Add([pscustomobject]@{label=$Label;program_label=$ProgramLabel;arguments=$Arguments;cwd=$WorkRoot;exit_code=0;log_sha256=(Sha $log)})
}
function Check-Product {
    $head=& $script:git --no-replace-objects -C $RepositoryRoot rev-parse HEAD
    if($LASTEXITCODE -ne 0 -or $head.Trim() -ne $ExpectedProductCommit){throw 'Llama product source differs'}
    $dirty=& $script:git --no-replace-objects -C $RepositoryRoot status --porcelain=v1 --untracked-files=all
    if($LASTEXITCODE -ne 0 -or $dirty){throw 'Llama product checkout is dirty'}
    if((Sha (Join-Path $RepositoryRoot 'core\Cargo.lock')) -ne $ExpectedCargoLockSha256){throw 'Llama Cargo lock differs'}
}
function Environment-For([string]$Architecture) {
    $batch=Join-Path $WorkRoot ('environment-'+$Architecture+'.cmd')
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$script:vcvars`" $Architecture >nul`r`nif errorlevel 1 exit /b %errorlevel%`r`nset`r`n",[Text.Encoding]::ASCII)
    $log=Run $script:cmd @('/d','/s','/c',("`"$batch`"")) ('capture '+$Architecture)
    $result=@{}
    foreach($line in [IO.File]::ReadAllLines($log)){
        $at=$line.IndexOf('=')
        if($at -gt 0){$key=$line.Substring(0,$at);if(@('PATH','INCLUDE','LIB','LIBPATH','VCToolsInstallDir','VCToolsVersion','WindowsSdkDir','WindowsSDKVersion','VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH') -contains $key){$result[$key]=$line.Substring($at+1)}}
    }
    $target=if($Architecture -eq 'x64'){'x64'}else{'arm64'}
    if($result['VSCMD_ARG_HOST_ARCH'] -ne 'x64' -or $result['VSCMD_ARG_TGT_ARCH'] -ne $target -or $result['VCToolsVersion'].TrimEnd('\') -ne '14.44.35207'){throw 'Pinned compiler environment differs'}
    return $result
}
function Set-Environment([hashtable]$Values) {foreach($key in $Values.Keys){[Environment]::SetEnvironmentVariable($key,$Values[$key],'Process')}}
function Source-Census([string]$Directory) {
    $rows=[Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
    foreach($item in Get-ChildItem -LiteralPath $Directory -Recurse -Force){
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Source tree contains a reparse point'}
        if(-not $item.PSIsContainer){$rows.Add($item.FullName.Substring($Directory.Length+1).Replace('\','/'),@([long]$item.Length,(Sha $item.FullName)))}
    }
    if($rows.Count -eq 0){throw 'Source census is empty'}
    return ,$rows
}
function Sdk-State {
    $state=[ordered]@{}
    foreach($scope in @('Machine','User')){foreach($name in @('PATH','VULKAN_SDK','VK_SDK_PATH')){$state[$scope+'/'+$name]=[Environment]::GetEnvironmentVariable($name,$scope)}}
    foreach($relative in @('System32\vulkan-1.dll','SysWOW64\vulkan-1.dll')){
        $file=Join-Path $env:SystemRoot $relative
        $state[$relative]=if(Test-Path -LiteralPath $file){@{bytes=[long](Get-Item -LiteralPath $file).Length;sha256=(Sha $file)}}else{$null}
    }
    foreach($prefix in @('HKLM:\SOFTWARE\Khronos\Vulkan','HKLM:\SOFTWARE\WOW6432Node\Khronos\Vulkan')){
        foreach($kind in @('Drivers','ExplicitLayers','ImplicitLayers')){
            $key=$prefix+'\'+$kind;$values=[ordered]@{}
            if(Test-Path -LiteralPath $key){$item=Get-Item -LiteralPath $key;foreach($name in @($item.GetValueNames()|Sort-Object)){$values[$name]=$item.GetValue($name)}}
            $state[$key]=$values
        }
    }
    return ($state|ConvertTo-Json -Depth 8 -Compress)
}
function Firewall-Ready {
    $profiles=@(Get-NetFirewallProfile)
    if($profiles.Count -ne 3 -or @($profiles|Where-Object{$_.Enabled -ne 'True'}).Count -ne 0){throw 'All Windows Firewall profiles must be enabled'}
}
function Deny([string]$File) {
    $name='solstone-llama-arm64-'+$PID+'-'+[Guid]::NewGuid().ToString('N')
    # Register cleanup before mutation, including a partially successful provider call.
    $rules.Add($name)
    $null=New-NetFirewallRule -Name $name -DisplayName $name -Enabled True -Direction Outbound -Action Block -Program $File -Profile Any -Protocol Any -LocalAddress Any -RemoteAddress Any -LocalPort Any -RemotePort Any
    return $name
}
function Tcp-Error {
    $client=[Net.Sockets.TcpClient]::new()
    try{
        $pending=$client.BeginConnect('1.1.1.1',443,$null,$null)
        try{if(-not $pending.AsyncWaitHandle.WaitOne(15000)){throw 'Native TCP control timed out'};$client.EndConnect($pending)}finally{$pending.AsyncWaitHandle.Close()}
        return 0
    }catch{
        $errorValue=$_.Exception
        while($null -ne $errorValue){if($errorValue -is [Net.Sockets.SocketException]){return [int]$errorValue.NativeErrorCode};$errorValue=$errorValue.InnerException}
        throw
    }finally{$client.Dispose()}
}
function Add-Program([string]$Label,[string]$File) {$programs.Add($Label,(Full-Path $File));$null=Identity $File $Label}
function One([string]$Directory,[string]$Name) {
    $found=@(Get-ChildItem -LiteralPath $Directory -Recurse -File -Filter $Name)
    if($found.Count -ne 1){throw "Expected exactly one $Name under $Directory"}
    return $found[0].FullName
}

try {
    $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or -not [Environment]::Is64BitProcess -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64'){throw 'Administrative native AMD64 builder required'}
    $RepositoryRoot=Full-Path $RepositoryRoot;$SourceArchive=Full-Path $SourceArchive;$SdkArchive=Full-Path $SdkArchive;$OpenMpInputs=Full-Path $OpenMpInputs
    $WorkRoot=Full-Path $WorkRoot;$CaptureRoot=Full-Path $CaptureRoot;$ReportRoot=Full-Path $ReportRoot
    $roots=@($RepositoryRoot,$WorkRoot,$CaptureRoot,$ReportRoot)
    for($i=0;$i -lt $roots.Count;$i++){for($j=$i+1;$j -lt $roots.Count;$j++){
        if($roots[$i].Equals($roots[$j],[StringComparison]::OrdinalIgnoreCase) -or ($roots[$i]+'\').StartsWith($roots[$j]+'\',[StringComparison]::OrdinalIgnoreCase) -or ($roots[$j]+'\').StartsWith($roots[$i]+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Llama roots must be disjoint'}
    }}
    New-Root $WorkRoot;New-Root $CaptureRoot;New-Root $ReportRoot
    foreach($name in @('logs','report','proof','host','output\bin')){New-Item -ItemType Directory (Join-Path $CaptureRoot $name)|Out-Null}
    $mapIdentity=Identity $OpenMpInputs 'input-map'
    if($mapIdentity.size -gt 4194304){throw 'Bounded OpenMP map required'}
    $inputMapBytes=[IO.File]::ReadAllBytes($OpenMpInputs)
    $OpenMpInputs=Join-Path $WorkRoot 'openmp-inputs.json'
    [IO.File]::WriteAllBytes($OpenMpInputs,$inputMapBytes)
    $inputMapSha=Sha $OpenMpInputs
    $original=(Raw-Text $OpenMpInputs)|ConvertFrom-Json
    $git=(Get-Command git.exe -ErrorAction Stop).Source;$cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    Check-Product
    if([IO.Path]::GetFullPath($PSCommandPath) -ne (Join-Path $RepositoryRoot 'core\distribution\llama-windows-arm64-build.ps1')){throw 'Driver must run from its clean product checkout'}
    $driverSha=Sha $PSCommandPath;$helperSha=Sha (Join-Path $PSScriptRoot 'ffmpeg-windows-arm64-tools.ps1')
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath
    if($LASTEXITCODE -ne 0 -or $vs -ne 'C:\BuildTools'){throw 'Pinned BuildTools installation absent'}
    $vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    $hostEnvironment=Environment-For 'x64';$armEnvironment=Environment-For 'x64_arm64'
    $hostBin=Join-Path $hostEnvironment.VCToolsInstallDir 'bin\Hostx64\x64';$armBin=Join-Path $armEnvironment.VCToolsInstallDir 'bin\Hostx64\arm64'
    $nmake=Join-Path $hostBin 'nmake.exe'
    Set-Location $WorkRoot
    $batch=Join-Path $WorkRoot 'build-recorder.cmd'
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$vcvars`" x64 >nul`r`nif errorlevel 1 exit /b %errorlevel%`r`ncd /d `"$RepositoryRoot`"`r`nset CARGO_BUILD_JOBS=2`r`ncargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --target x86_64-pc-windows-msvc --locked --offline`r`nexit /b %errorlevel%`r`n",[Text.Encoding]::ASCII)
    $null=Run $cmd @('/d','/s','/c',("`"$batch`"")) 'offline native recorder build'
    $recorder=Join-Path $RepositoryRoot 'core\target\x86_64-pc-windows-msvc\debug\solstone-distribution.exe';Host-Pe $recorder
    $inputArgs=@('--source-archive',$SourceArchive,'--sdk-archive',$SdkArchive,'--openmp-inputs',$OpenMpInputs)
    Set-Location $RepositoryRoot
    $raw=Run $recorder (@('llama-windows-arm64','verify-inputs')+$inputArgs+@('--work-root',$WorkRoot)) 'pinned source, SDK and OpenMP admission'
    $plan=[IO.File]::ReadAllText($raw)|ConvertFrom-Json
    $sourceRoot=Join-Path $WorkRoot 'source';$llvmOuter=Join-Path $WorkRoot 'llvm-unpack';$cmakeOuter=Join-Path $WorkRoot 'cmake-unpack'
    $llvmRoot=Join-Path $WorkRoot 'llvm';$cmakeRoot=Join-Path $WorkRoot 'cmake';$pythonRoot=Join-Path $WorkRoot 'python';$openmpRoot=Join-Path $WorkRoot 'openmp'
    foreach($dir in @($sourceRoot,$llvmOuter,$cmakeOuter,$pythonRoot,(Join-Path $openmpRoot 'include'),(Join-Path $openmpRoot 'lib'),(Join-Path $WorkRoot 'inputs'))){New-Item -ItemType Directory $dir|Out-Null}
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.cmake_archive,$cmakeOuter)
    Move-Item (Join-Path $cmakeOuter 'cmake-3.31.12-windows-x86_64') $cmakeRoot
    $cmake=Join-Path $cmakeRoot 'bin\cmake.exe'
    $pin=@($plan.tools|Where-Object{$_.label -eq 'cmake/bin/cmake.exe'});$observed=Identity $cmake 'cmake/bin/cmake.exe'
    if($pin.Count -ne 1 -or $pin[0].size -ne $observed.size -or $pin[0].sha256 -ne $observed.sha256){throw 'Extracted pinned CMake differs'}
    Set-Location $llvmOuter;$null=Run $cmake @('-E','tar','xf',([string]$original.llvm_archive)) 'extract pinned LLVM host tools'
    Move-Item (Join-Path $llvmOuter 'clang+llvm-22.1.6-x86_64-pc-windows-msvc') $llvmRoot
    Set-Location $sourceRoot;$null=Run $cmake @('-E','tar','xzf',$SourceArchive) 'extract pinned Unicode source closure'
    Set-Location $WorkRoot
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.python_archive,$pythonRoot)
    $pythonConfig="python312.zip`n.`n";[IO.File]::WriteAllText((Join-Path $pythonRoot 'python312._pth'),$pythonConfig,$utf8)
    foreach($member in @('include/omp.h','lib/libomp.lib')){
        $to=Join-Path $openmpRoot $member.Replace('/','\');Copy-Item -LiteralPath (Join-Path ([string]$original.development_root) $member.Replace('/','\')) -Destination $to
        $id=Identity $to ('dependencies/openmp/'+$member);$pin=@($plan.openmp_inputs|Where-Object{$_.label -eq $id.label})
        if($pin.Count -ne 1 -or $pin[0].size -ne $id.size -or $pin[0].sha256 -ne $id.sha256){throw 'Copied original OpenMP development input differs'}
    }
    $sdkCopy=Join-Path $WorkRoot 'inputs\vulkan-sdk.exe';Copy-Item -LiteralPath $SdkArchive -Destination $sdkCopy
    $pin=@($plan.tools|Where-Object{$_.label -eq 'inputs/vulkan-sdk.exe'});$observed=Identity $sdkCopy 'inputs/vulkan-sdk.exe'
    if($pin.Count -ne 1 -or $observed.size -ne $pin[0].size -or $observed.sha256 -ne $pin[0].sha256){throw 'Copied pinned SDK differs'}
    Firewall-Ready
    $sdkBefore=Sdk-State;$extractRule=Deny $sdkCopy
    $sdkRoot=Join-Path $WorkRoot 'sdk'
    $null=Run $sdkCopy @('--root',$sdkRoot,'--accept-licenses','--default-answer','--confirm-command','install','copy_only=1') 'fenced SDK copy-only extraction'
    if((Sdk-State) -ne $sdkBefore){throw 'Copy-only SDK changed global state'}
    Remove-NetFirewallRule -Name $extractRule -ErrorAction Stop
    $sdkExpected=@($plan.tools|Where-Object{$_.label.StartsWith('sdk/')})
    $sdkActual=@(Get-ChildItem -LiteralPath $sdkRoot -Recurse -File -Filter '*.exe')
    if($sdkExpected.Count -ne $sdkActual.Count){throw 'SDK native executable census differs'}
    foreach($file in $sdkActual){$label='sdk/'+$file.FullName.Substring($sdkRoot.Length+1).Replace('\','/').ToLowerInvariant();if(@($sdkExpected|Where-Object{$_.label -eq $label}).Count -ne 1){throw "Unpinned SDK program: $label"}}
    $sourceBefore=Source-Census $sourceRoot
    $patch=Join-Path $sourceRoot 'patches\0001-bounded-shader-build.patch'
    $null=Run $git @('-C',(Join-Path $sourceRoot 'llama'),'apply','--check',$patch) 'check original bounded shader patch'
    $null=Run $git @('-C',(Join-Path $sourceRoot 'llama'),'apply',$patch) 'apply original bounded shader patch'
    $sourcePatched=Source-Census $sourceRoot
    $clang=Join-Path $llvmRoot 'bin\clang-cl.exe';$python=Join-Path $pythonRoot 'python.exe'
    Set-Environment $hostEnvironment
    $proxies=Join-Path $WorkRoot 'host-proxies';New-Item -ItemType Directory $proxies|Out-Null
    foreach($row in @(@('host-cc','host-cc',$clang),@('host-link','arm-link',(Join-Path $hostBin 'link.exe')),@('host-lib','arm-lib',(Join-Path $hostBin 'lib.exe')))){
        $name=[string]$row[0];$src=Join-Path $proxies ($name+'.c');$exe=Join-Path $proxies ($name+'.exe')
        [IO.File]::WriteAllText($src,(New-FfmpegWindowsArm64ProxySource -Role ([string]$row[1]) -Program ([string]$row[2]) -Environment $hostEnvironment),$utf8)
        $null=Run (Join-Path $hostBin 'cl.exe') @('/nologo','/O2','/MT',('/Fe'+$exe),('/Fo'+(Join-Path $proxies ($name+'.obj'))),$src) ('compile unshipped '+$name)
        Host-Pe $exe
        Copy-Item -LiteralPath $src -Destination (Join-Path $CaptureRoot ('proof\'+$name+'.c'))
        Copy-Item -LiteralPath $exe -Destination (Join-Path $CaptureRoot ('proof\'+$name+'.exe'))
    }
    $hostToolchain=Join-Path $WorkRoot 'shader-host-toolchain.cmake'
    $hostToolchainText="set(CMAKE_SYSTEM_NAME Windows)`nset(CMAKE_SYSTEM_PROCESSOR AMD64)`n"
    foreach($entry in @(@('CMAKE_C_COMPILER',(Join-Path $proxies 'host-cc.exe')),@('CMAKE_CXX_COMPILER',(Join-Path $proxies 'host-cc.exe')),@('CMAKE_LINKER',(Join-Path $proxies 'host-link.exe')),@('CMAKE_AR',(Join-Path $proxies 'host-lib.exe')),@('CMAKE_MAKE_PROGRAM',$nmake))){$hostToolchainText+='set('+[string]$entry[0]+' "'+([string]$entry[1]).Replace('\','/')+'" CACHE FILEPATH "" FORCE)'+"`n"}
    $hostToolchainText+="set(CMAKE_MSVC_RUNTIME_LIBRARY MultiThreadedDLL)`n";[IO.File]::WriteAllText($hostToolchain,$hostToolchainText,$utf8)
    foreach($tool in $plan.tools){
        $file=Join-Path $WorkRoot $tool.label.Replace('/','\');$id=Identity $file $tool.label
        if($id.size -ne $tool.size -or $id.sha256 -ne $tool.sha256){throw "Extracted pinned tool differs: $($tool.label)"}
        Add-Program ([string]$tool.label) $file
    }
    foreach($file in Get-ChildItem -LiteralPath $armBin -Filter '*.exe' -File){if($file.Name -ne 'nmake.exe'){Add-Program ('msvc/'+$file.Name.ToLowerInvariant()) $file.FullName}}
    foreach($file in Get-ChildItem -LiteralPath $hostBin -Filter '*.exe' -File){Add-Program ('msvc/host-'+$file.Name.ToLowerInvariant()) $file.FullName}
    Add-Program 'msvc/nmake.exe' $nmake
    $windowsSdk=Join-Path $armEnvironment.WindowsSdkDir ('bin\'+$armEnvironment.WindowsSDKVersion.TrimEnd('\')+'\x64')
    foreach($file in Get-ChildItem -LiteralPath $windowsSdk -Filter '*.exe' -File){Add-Program ('sdk/'+$file.Name.ToLowerInvariant()) $file.FullName}
    $msbuildRoot=Join-Path $vs 'MSBuild\Current\Bin\amd64'
    foreach($file in Get-ChildItem -LiteralPath $msbuildRoot -Recurse -File -Filter '*.exe'){
        $relative=$file.FullName.Substring($msbuildRoot.Length+1).Replace('\','-').ToLowerInvariant()
        $label=if($relative -eq 'msbuild.exe'){'msvc/msbuild.exe'}else{'msvc/msbuild-'+$relative}
        Add-Program $label $file.FullName
    }
    Add-Program 'windows/cmd.exe' $cmd;Add-Program 'windows/powershell.exe' (Get-Process -Id $PID).Path
    foreach($tool in @(Get-ControlledGitPrograms $git)){Add-Program ([string]$tool.label) ([string]$tool.file)}
    Add-Program 'windows/recorder.exe' $recorder
    foreach($name in @('host-cc','host-link','host-lib')){Add-Program ('windows/'+$name+'.exe') (Join-Path $proxies ($name+'.exe'))}
    # The native shader generator does not exist yet. Fence its future path before building it.
    $shader=Join-Path $WorkRoot 'engine-build\Release\vulkan-shaders-gen.exe'
    $programs.Add('windows/shader-generator.exe',$shader)
    $labels=@($programs.Keys);[Array]::Sort($labels,[StringComparer]::Ordinal)
    $beforeTools=@();foreach($label in $labels){if($label -ne 'windows/shader-generator.exe'){$beforeTools+=Identity $programs[$label] $label}}
    $positive=Tcp-Error;if($positive -ne 0){throw "Native network positive control failed: $positive"}
    $denies=@()
    foreach($label in $labels){
        $name=Deny $programs[$label];$rule=Get-NetFirewallRule -Name $name
        $app=$rule|Get-NetFirewallApplicationFilter;$address=$rule|Get-NetFirewallAddressFilter;$port=$rule|Get-NetFirewallPortFilter
        $protocol=[string]$port.Protocol;if($protocol -eq '256'){$protocol='Any'}
        $denies+=[pscustomobject]@{label=$label;program=[string]$app.Program;enabled=($rule.Enabled -eq 'True');direction=[string]$rule.Direction;action=[string]$rule.Action;profile=[string]$rule.Profile;protocol=$protocol;local_address=@($address.LocalAddress);remote_address=@($address.RemoteAddress);local_port=@($port.LocalPort);remote_port=@($port.RemotePort)}
    }
    $blockedBefore=Tcp-Error;if($blockedBefore -ne 10013){throw "Native network deny failed: $blockedBefore"}
    Set-Environment $armEnvironment
    foreach($name in @('CL','_CL_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_PLATFORM','CMAKE_GENERATOR_TOOLSET','PYTHONHOME','PYTHONPATH')){Remove-Item ('Env:\'+$name) -ErrorAction SilentlyContinue}
    $env:INCLUDE=(Join-Path $openmpRoot 'include')+';'+$env:INCLUDE;$env:PATH=(Join-Path $llvmRoot 'bin')+';'+$env:PATH;$env:VULKAN_SDK=$sdkRoot
    $forward=$WorkRoot.Replace('\','/')
    Build-Step 'loader-configure' 'cmake/bin/cmake.exe' @($plan.loader_arguments)
    Build-Step 'loader-build' 'cmake/bin/cmake.exe' @('--build',($forward+'/loader-build'),'--config','Release','--target','vulkan','--parallel','1')
    Build-Step 'offset-replay' 'python/python.exe' @(($forward+'/source/loader/scripts/parse_asm_values.py'),($forward+'/offsets-replayed.asm'),($forward+'/loader-build/upstream/loader/Release/asm_offset.asm'),'MARMASM','MSVC','arm64')
    Build-Step 'engine-configure' 'cmake/bin/cmake.exe' @($plan.engine_arguments)
    Build-Step 'engine-build' 'cmake/bin/cmake.exe' @('--build',($forward+'/engine-build'),'--target','llama-server','--parallel','1')
    $blockedAfter=Tcp-Error;if($blockedAfter -ne 10013){throw "Native network deny changed: $blockedAfter"}
    Firewall-Ready
    foreach($id in $beforeTools){$after=Identity $programs[$id.label] $id.label;if($after.size -ne $id.size -or $after.sha256 -ne $id.sha256){throw "Tool changed during build: $($id.label)"}}
    $archiveLabels=@{};foreach($tool in $plan.tools){$archiveLabels.Add([string]$tool.label,$true)}
    $archiveIds=@();$systemIds=@();$paths=[ordered]@{}
    foreach($label in $labels){$paths[$label]=$programs[$label];$id=Identity $programs[$label] $label;if($archiveLabels.ContainsKey($label)){$archiveIds+=$id}else{$systemIds+=$id}}
    Host-Pe $shader
    $development=@();foreach($member in @('include/omp.h','lib/libomp.lib')){$development+=Identity (Join-Path $openmpRoot $member.Replace('/','\')) ('dependencies/openmp/'+$member)}
    $caches=[ordered]@{engine=(Raw-Text (Join-Path $WorkRoot 'engine-build\CMakeCache.txt'));loader=(Raw-Text (Join-Path $WorkRoot 'loader-build\CMakeCache.txt'));shader=(Raw-Text (Join-Path $WorkRoot 'engine-build\ggml\src\ggml-vulkan\vulkan-shaders-gen-prefix\src\vulkan-shaders-gen-build\CMakeCache.txt'))}
    $generated=[ordered]@{};foreach($name in $plan.generated_members){$generated[$name]=Raw-Text (Join-Path $WorkRoot $name.Replace('/','\'))}
    $sourceAfter=Source-Census $sourceRoot;$sdkAfter=Sdk-State
    if($sdkBefore -ne $sdkAfter){throw 'Build changed global Vulkan state'}
    foreach($row in @(@((One (Join-Path $WorkRoot 'engine-build\bin') 'llama-server.exe'),'output\bin\llama-server.exe'),@((Join-Path $WorkRoot 'loader-build\upstream\loader\Release\vulkan-1.dll'),'output\bin\vulkan-1.dll'),@($shader,'host\vulkan-shaders-gen.exe'),@((One (Join-Path $WorkRoot 'loader-build') 'unknown_ext_chain_marmasm.obj'),'proof\unknown_ext_chain_marmasm.obj'),@((One (Join-Path $WorkRoot 'loader-build') 'gen_defines.asm'),'proof\gen_defines.asm'),@((Join-Path $WorkRoot 'offsets-replayed.asm'),'proof\offsets-replayed.asm'))){Copy-Item -LiteralPath ([string]$row[0]) -Destination (Join-Path $CaptureRoot ([string]$row[1]))}
    $validation=[ordered]@{schema=$plan.validation_schema;product=@{commit=$ExpectedProductCommit;lock_sha256=$ExpectedCargoLockSha256};driver_sha256=$driverSha;helper_sha256=$helperSha;recorder_sha256=(Sha $recorder);work_root=$WorkRoot;archive_tools=$archiveIds;system_tools=$systemIds;network_denied_tools=$labels;tool_paths=$paths;network_denies=$denies;tools_before=$beforeTools;source_before=$sourceBefore;source_patched=$sourcePatched;source_after=$sourceAfter;caches=$caches;generated_configuration=$generated;openmp_development=$development;python_path_configuration=$pythonConfig;sdk_state_before=$sdkBefore;sdk_state_after=$sdkAfter;network_positive_error=$positive;network_blocked_before_error=$blockedBefore;network_blocked_after_error=$blockedAfter;commands=@($commands.ToArray())}
    $validationFile=Join-Path $CaptureRoot 'report\validation.json';Json-File $validationFile $validation
    if((Sha $OpenMpInputs) -ne $inputMapSha){throw 'OpenMP input map snapshot changed'}
    Check-Product
    Set-Location $RepositoryRoot
    $receiptFile=Join-Path $ReportRoot 'receipt.json';$evidenceFile=Join-Path $ReportRoot 'evidence.json'
    $recordArgs=@('llama-windows-arm64','record')+$inputArgs+@('--capture-root',$CaptureRoot,'--receipt',$receiptFile,'--evidence',$evidenceFile,'--product-commit',$ExpectedProductCommit,'--cargo-lock-sha256',$ExpectedCargoLockSha256,'--builder-host',$BuilderHost)
    $log=Run $recorder $recordArgs 'record original controlled ARM64 Llama build'
    if([IO.File]::ReadAllText($log) -notmatch 'LLAMA_ARM64_WINDOWS_RECORD_PASS'){throw 'Llama recorder marker absent'}
    $log=Run $recorder (@('llama-windows-arm64','verify')+$inputArgs+@('--capture-root',$CaptureRoot,'--receipt',$receiptFile,'--evidence',$evidenceFile)) 're-admit original controlled ARM64 Llama build'
    if([IO.File]::ReadAllText($log) -notmatch 'LLAMA_ARM64_WINDOWS_VERIFY_PASS'){throw 'Llama re-admission marker absent'}
    Check-Product
    $result=[ordered]@{marker='LLAMA_ARM64_CONTROLLED_BUILD_PASS';recorded_utc=[DateTime]::UtcNow.ToString('o');product_commit=$ExpectedProductCommit;receipt_sha256=(Sha $receiptFile);evidence_sha256=(Sha $evidenceFile);validation_sha256=(Sha $validationFile);engine_sha256=(Sha (Join-Path $CaptureRoot 'output\bin\llama-server.exe'));loader_sha256=(Sha (Join-Path $CaptureRoot 'output\bin\vulkan-1.dll'));native_arm_execution=$false;installed_journal=$false}
} finally {
    foreach($name in $rules){Remove-NetFirewallRule -Name $name -ErrorAction SilentlyContinue}
    $remaining=@(Get-NetFirewallRule -ErrorAction Stop)
    foreach($name in $rules){if(@($remaining|Where-Object{$_.Name -eq $name}).Count -ne 0){throw "Llama firewall cleanup unconfirmed: $name"}}
}
Json-File (Join-Path $ReportRoot 'result.json') $result
$result|ConvertTo-Json -Depth 5 -Compress
