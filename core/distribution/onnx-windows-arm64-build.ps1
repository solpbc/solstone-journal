# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Produce reduced CPU ONNX Runtime for native Windows ARM64 with offline inputs.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$Inputs,
    [Parameter(Mandatory=$true)][string]$WorkRoot,
    [Parameter(Mandatory=$true)][string]$OutputRoot,
    [Parameter(Mandatory=$true)][string]$ReportRoot,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory=$true)][string]$BuilderHost
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$utf8=[Text.UTF8Encoding]::new($false)
. (Join-Path $PSScriptRoot 'windows-git-programs.ps1')
$rules=[Collections.Generic.List[string]]::new()
$programs=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
$systemIds=[Collections.Generic.Dictionary[string,object]]::new([StringComparer]::Ordinal)
$commandOrdinal=0
$aggregateLog='build.log'

function Full-Path([string]$Value) {
    if ($Value -notmatch '^[A-Za-z]:[\\/]' -or $Value -match '["%!?&|<>^\r\n]' -or $Value -match '(^|[\\/])\.\.([\\/]|$)') {
        throw "Ordinary absolute build path required: $Value"
    }
    return [IO.Path]::GetFullPath($Value).TrimEnd('\','/')
}
function Sha([string]$File) { return (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant() }
function Text-Sha([string]$Value) {
    $algorithm=[Security.Cryptography.SHA256]::Create()
    try { return [BitConverter]::ToString($algorithm.ComputeHash($utf8.GetBytes($Value))).Replace('-','').ToLowerInvariant() }
    finally { $algorithm.Dispose() }
}
function New-Root([string]$Directory) {
    if (Test-Path -LiteralPath $Directory) { throw "Fresh ONNX root required: $Directory" }
    New-Item -ItemType Directory -Path $Directory | Out-Null
}
function Json-File([string]$File,[object]$Value) {
    [IO.File]::WriteAllText($File,($Value|ConvertTo-Json -Depth 12),$utf8)
}
function Run-Checked([string]$Program,[string[]]$Arguments,[string]$Label) {
    if (-not [IO.File]::Exists($Program)) { throw "Native program missing: $Program" }
    $script:commandOrdinal+=1
    $raw=Join-Path $ReportRoot ("command-{0:D3}.log" -f $script:commandOrdinal)
    $saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    $global:LASTEXITCODE=$null
    try { & $Program @Arguments *> $raw; $code=$global:LASTEXITCODE }
    finally { $ErrorActionPreference=$saved }
    $text=[IO.File]::ReadAllText($raw)
    [IO.File]::AppendAllText((Join-Path $ReportRoot $script:aggregateLog),("COMMAND $Label`n$text`nEXIT=$code`n"),$utf8)
    if ($null -eq $code -or $code -ne 0) { throw "$Label exited ${code}: $text" }
    return $raw
}
function File-Identity([string]$File,[string]$Label) {
    $item=Get-Item -LiteralPath $File
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Regular input required: $File" }
    return [pscustomobject]@{label=$Label;size=[long]$item.Length;sha256=(Sha $File)}
}
function Native-Host-Program([string]$File) {
    $bytes=[IO.File]::ReadAllBytes($File)
    if ($bytes.Length -lt 64 -or $bytes[0] -ne 0x4d -or $bytes[1] -ne 0x5a) { throw "Host program is not PE: $File" }
    $offset=[BitConverter]::ToInt32($bytes,0x3c)
    if ($offset -lt 0 -or $offset+6 -gt $bytes.Length -or [BitConverter]::ToUInt16($bytes,$offset+4) -ne 0x8664) { throw "Host program is not AMD64: $File" }
}
function Add-System-Tool([string]$Label,[string]$File) {
    if ($programs.ContainsKey($Label)) { throw "Duplicate ONNX system-tool label: $Label" }
    $File=Full-Path $File
    $systemIds.Add($Label,(File-Identity $File $Label))
    $programs.Add($Label,$File)
}
function Import-Arm-Environment([string]$Vcvars,[string]$Cmd) {
    $batch=Join-Path $WorkRoot 'arm-environment.cmd'
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$Vcvars`" x64_arm64 >nul`r`nif errorlevel 1 exit /b %errorlevel%`r`nset`r`n",[Text.Encoding]::ASCII)
    $raw=Run-Checked $Cmd @('/d','/s','/c',("`"{0}`"" -f $batch)) 'ARM toolchain environment'
    $wanted=@('PATH','INCLUDE','LIB','LIBPATH','VCToolsInstallDir','VCINSTALLDIR','VSINSTALLDIR','WindowsSdkDir','WindowsSDKVersion','WindowsSDKLibVersion','UniversalCRTSdkDir','UCRTVersion','VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH','VCToolsVersion')
    foreach ($line in [IO.File]::ReadAllLines($raw)) {
        $at=$line.IndexOf('=')
        if ($at -gt 0) {
            $key=$line.Substring(0,$at)
            if ($wanted -contains $key) { [Environment]::SetEnvironmentVariable($key,$line.Substring($at+1),'Process') }
        }
    }
    if ($env:VSCMD_ARG_HOST_ARCH -ne 'x64' -or $env:VSCMD_ARG_TGT_ARCH -ne 'arm64') { throw 'ARM64 vcvars environment was not established' }
}

try {
    $principal=[Security.Principal.WindowsPrincipal]::new([Security.Principal.WindowsIdentity]::GetCurrent())
    if(-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or -not [Environment]::Is64BitProcess){throw 'Administrative AMD64 builder required'}
    $RepositoryRoot=Full-Path $RepositoryRoot;$Inputs=Full-Path $Inputs
    $WorkRoot=Full-Path $WorkRoot;$OutputRoot=Full-Path $OutputRoot;$ReportRoot=Full-Path $ReportRoot
    $roots=@($WorkRoot,$OutputRoot,$ReportRoot,$RepositoryRoot)
    for($i=0;$i -lt $roots.Count;$i++){
        for($j=$i+1;$j -lt $roots.Count;$j++){
            if($roots[$i].Equals($roots[$j],[StringComparison]::OrdinalIgnoreCase) -or ($roots[$i]+'\').StartsWith($roots[$j]+'\',[StringComparison]::OrdinalIgnoreCase) -or ($roots[$j]+'\').StartsWith($roots[$i]+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'ONNX ARM roots must be disjoint'}
        }
    }
    New-Root $WorkRoot;New-Root $OutputRoot;New-Root $ReportRoot
    [IO.File]::WriteAllText((Join-Path $ReportRoot 'build.log'),'',$utf8)
    $mapItem=Get-Item -LiteralPath $Inputs
    if($mapItem.PSIsContainer -or ($mapItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $mapItem.Length -gt 4194304){throw 'Bounded regular ONNX input map required'}
    $inputMapBytes=[IO.File]::ReadAllBytes($Inputs);$Inputs=Join-Path $WorkRoot 'inputs.json'
    $mapStream=[IO.File]::Open($Inputs,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{$mapStream.Write($inputMapBytes,0,$inputMapBytes.Length);$mapStream.Flush($true)}finally{$mapStream.Dispose()}
    $original=[IO.File]::ReadAllText($Inputs)|ConvertFrom-Json
    $inputMapSha=Sha $Inputs
    $git=(Get-Command git.exe -ErrorAction Stop).Source;$cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    $head=(& $git --no-replace-objects -C $RepositoryRoot rev-parse HEAD).Trim()
    if($LASTEXITCODE -ne 0 -or $head -ne $ExpectedProductCommit){throw 'ONNX ARM product source differs'}
    $dirty=& $git --no-replace-objects -C $RepositoryRoot status --porcelain=v1 --untracked-files=all
    if($LASTEXITCODE -ne 0 -or $dirty){throw 'ONNX ARM product checkout is dirty'}
    if((Sha (Join-Path $RepositoryRoot 'core\Cargo.lock')) -ne $ExpectedCargoLockSha256){throw 'ONNX ARM Cargo lock differs'}
    Set-Location $RepositoryRoot
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs=& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath
    if($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($vs)){throw 'MSVC ARM64 tools are absent'}
    $vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    $batch=Join-Path $WorkRoot 'build-recorder.cmd'
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$vcvars`" x64 >nul`r`nif errorlevel 1 exit /b %errorlevel%`r`ncd /d `"$RepositoryRoot`"`r`nset CARGO_BUILD_JOBS=2`r`ncargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --target x86_64-pc-windows-msvc --locked --offline`r`nexit /b %errorlevel%`r`n",[Text.Encoding]::ASCII)
    $null=Run-Checked $cmd @('/d','/s','/c',("`"{0}`"" -f $batch)) 'offline AMD64 recorder build'
    $recorder=Join-Path $RepositoryRoot 'core\target\x86_64-pc-windows-msvc\debug\solstone-distribution.exe'
    Native-Host-Program $recorder
    $raw=Run-Checked $recorder @('onnx-windows-arm64','verify-inputs','--inputs',$Inputs) 'pinned ONNX ARM input admission'
    $plan=[IO.File]::ReadAllText($raw)|ConvertFrom-Json
    $sourceRoot=Join-Path $WorkRoot 'source';$mirrorRoot=Join-Path $WorkRoot 'mirror';$cmakeOuter=Join-Path $WorkRoot 'cmake-unpack'
    $pythonRoot=Join-Path $WorkRoot 'python';$cmakeRoot=Join-Path $WorkRoot 'cmake';$protocRoot=Join-Path $WorkRoot 'protoc';$flatbuffersRoot=Join-Path $WorkRoot 'flatbuffers'
    New-Item -ItemType Directory $sourceRoot,$mirrorRoot,$cmakeOuter,$pythonRoot,$protocRoot,$flatbuffersRoot|Out-Null
    foreach($pair in @(@('source_archive',0),@('mirror_archive',1),@('kleidiai_archive',2),@('cmake_archive',3),@('python_archive',4),@('protoc_archive',5))){
        $file=Full-Path ([string]$original.($pair[0]));$pin=$plan.inputs[$pair[1]]
        $id=File-Identity $file ([string]$pin.label)
        if($id.size -ne $pin.size -or $id.sha256 -ne $pin.sha256){throw 'ONNX input changed after admission'}
    }
    $tar=Join-Path $env:SystemRoot 'System32\tar.exe'
    $null=Run-Checked $tar @('-xzf',([string]$original.source_archive),'-C',$sourceRoot) 'extract original ONNX source'
    $null=Run-Checked $tar @('-xzf',([string]$original.mirror_archive),'-C',$mirrorRoot) 'extract original CPU CMake mirror'
    $kleidi=Join-Path $mirrorRoot ([string]$plan.kleidiai_mirror).Replace('/','\')
    if(Test-Path -LiteralPath $kleidi){throw 'KleidiAI mirror member already exists'}
    New-Item -ItemType Directory -Path (Split-Path $kleidi -Parent) -Force|Out-Null
    Copy-Item -LiteralPath ([string]$original.kleidiai_archive) -Destination $kleidi
    if((Sha $kleidi) -ne $plan.inputs[2].sha256){throw 'Copied KleidiAI mirror differs'}
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.cmake_archive,$cmakeOuter)
    Move-Item (Join-Path $cmakeOuter 'cmake-3.31.12-windows-x86_64') $cmakeRoot
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.python_archive,$pythonRoot)
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.protoc_archive,$protocRoot)
    [IO.Compression.ZipFile]::ExtractToDirectory((Join-Path $mirrorRoot 'github.com/google/flatbuffers/archive/refs/tags/v23.5.26.zip'),$flatbuffersRoot)
    Import-Arm-Environment $vcvars $cmd
    foreach($name in @('CL','_CL_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_PLATFORM','CMAKE_GENERATOR_TOOLSET','PYTHONHOME','PYTHONPATH')){Remove-Item ('Env:\'+$name) -ErrorAction SilentlyContinue}
    $pythonConfig="python312.zip`n.`n$(Join-Path $sourceRoot 'tools/ci_build')`n$(Join-Path $sourceRoot 'tools/python')`n$(Join-Path $flatbuffersRoot 'flatbuffers-23.5.26/python')`n"
    [IO.File]::WriteAllText((Join-Path $pythonRoot 'python312._pth'),$pythonConfig,$utf8)
    $archiveIds=[Collections.Generic.List[object]]::new()
    foreach($tool in $plan.tools){
        $file=Join-Path $WorkRoot $tool.label.Replace('/','\');$observed=File-Identity $file $tool.label
        if($observed.size -ne $tool.size -or $observed.sha256 -ne $tool.sha256){throw 'Extracted ONNX host tool differs'}
        Native-Host-Program $file;$archiveIds.Add($observed);$programs.Add([string]$tool.label,$file)
    }
    $msvcArm=Join-Path $env:VCToolsInstallDir 'bin\Hostx64\arm64'
    foreach($file in Get-ChildItem $msvcArm -Filter '*.exe' -File){Add-System-Tool ('msvc/'+$file.Name.ToLowerInvariant()) $file.FullName}
    $msbuildRoot=Join-Path $vs 'MSBuild\Current\Bin\amd64'
    $msbuild=Join-Path $msbuildRoot 'MSBuild.exe';Add-System-Tool 'msbuild/MSBuild.exe' $msbuild
    $ordinal=0
    foreach($file in Get-ChildItem (Split-Path $msbuildRoot -Parent) -Filter '*.exe' -Recurse -File){
        if($file.FullName -ne $msbuild){$ordinal+=1;Add-System-Tool ('msbuild/child-'+$ordinal+'.exe') $file.FullName}
    }
    $sdk=Join-Path $env:WindowsSdkDir ('bin\'+$env:WindowsSDKVersion.TrimEnd('\')+'\x64')
    foreach($file in Get-ChildItem $sdk -Filter '*.exe' -File){Add-System-Tool ('sdk/'+$file.Name.ToLowerInvariant()) $file.FullName}
    Add-System-Tool 'windows/cmd.exe' $cmd
    Add-System-Tool 'windows/powershell.exe' (Get-Process -Id $PID).Path
    foreach($tool in @(Get-ControlledGitPrograms $git)){Add-System-Tool ([string]$tool.label) ([string]$tool.file)}
    $gitBackend=$programs['windows/git-backend.exe']
    $gitPrefix=Split-Path (Split-Path $gitBackend -Parent) -Parent;$gitRoot=Split-Path $gitPrefix -Parent
    $patch=Join-Path $gitRoot 'usr\bin\patch.exe';Add-System-Tool 'windows/patch.exe' $patch
    $support=[Collections.Generic.List[object]]::new()
    foreach($name in @('msys-2.0.dll','msys-iconv-2.dll','msys-intl-8.dll')){
        $file=Join-Path $gitRoot ('usr\bin\'+$name)
        if(Test-Path -LiteralPath $file){$support.Add((File-Identity $file ('windows/'+$name)))}
    }
    $env:PATH=$msvcArm+';'+(Join-Path $gitRoot 'usr\bin')+';'+(Join-Path $cmakeRoot 'bin')+';'+$env:PATH
    Add-System-Tool 'windows/recorder.exe' $recorder
    foreach($label in @('msvc/cl.exe','msvc/link.exe','msvc/lib.exe','msvc/armasm64.exe','msbuild/MSBuild.exe','sdk/rc.exe','sdk/mt.exe','windows/cmd.exe','windows/powershell.exe','windows/recorder.exe')){Native-Host-Program $programs[$label]}
    $labels=@($programs.Keys);[Array]::Sort($labels,[StringComparer]::Ordinal)
    if($labels.Count -gt 4096){throw 'ONNX tool census exceeds bound'}
    if(@(Get-NetFirewallProfile|Where-Object{$_.Enabled -ne 'True'}).Count -ne 0){throw 'Windows Firewall profiles must be enabled'}
    $denies=[Collections.Generic.List[object]]::new()
    foreach($label in $labels){
        $name='solstone-onnx-arm64-'+$PID+'-'+[Guid]::NewGuid().ToString('N')
        $null=New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $programs[$label] -Profile Any -Protocol Any -LocalAddress Any -RemoteAddress Any -LocalPort Any -RemotePort Any
        $rules.Add($name);$rule=Get-NetFirewallRule -DisplayName $name
        $app=$rule|Get-NetFirewallApplicationFilter;$address=$rule|Get-NetFirewallAddressFilter;$port=$rule|Get-NetFirewallPortFilter
        $protocol=[string]$port.Protocol;if($protocol -eq '256'){$protocol='Any'}
        $denies.Add([pscustomobject]@{label=$label;program=[string]$app.Program;enabled=($rule.Enabled -eq 'True');direction=[string]$rule.Direction;action=[string]$rule.Action;profile=[string]$rule.Profile;protocol=$protocol;local_address=@($address.LocalAddress);remote_address=@($address.RemoteAddress);local_port=@($port.LocalPort);remote_port=@($port.RemotePort)})
    }
    $cmake=Join-Path $cmakeRoot 'bin\cmake.exe';$ctest=Join-Path $cmakeRoot 'bin\ctest.exe';$python=Join-Path $pythonRoot 'python.exe';$protoc=Join-Path $protocRoot 'bin\protoc.exe'
    $buildArguments=@((Join-Path $sourceRoot 'tools/ci_build/build.py'),'--build_dir',(Join-Path $sourceRoot 'build/Windows'),'--config','Release','--update','--build','--skip_tests','--skip_submodule_sync','--parallel','2','--arm64','--build_shared_lib','--include_ops_by_config',(Join-Path $sourceRoot 'required-operators.config'),'--disable_contrib_ops','--disable_ml_ops','--path_to_protoc_exe',$protoc,'--cmake_path',$cmake,'--ctest_path',$ctest,'--cmake_generator','Visual Studio 17 2022','--cmake_deps_mirror_dir',$mirrorRoot,'--cmake_extra_defines')
    $configKeys=@($plan.configuration.PSObject.Properties.Name);[Array]::Sort($configKeys,[StringComparer]::Ordinal)
    foreach($key in $configKeys){if($key -notin @('CMAKE_GENERATOR','CMAKE_GENERATOR_PLATFORM','onnxruntime_REDUCED_OPS_BUILD')){$buildArguments+=($key+'='+[string]$plan.configuration.$key)}}
    $buildArguments+=@(('Python3_EXECUTABLE='+$python),('GIT_EXECUTABLE='+$gitBackend),('Patch_EXECUTABLE='+$patch),('CMAKE_MAKE_PROGRAM='+$msbuild))
    $sourcePatch=Join-Path $RepositoryRoot ([string]$plan.source_patch)
    $patchIdentity=File-Identity $sourcePatch 'patches/onnxruntime-arm64-fp16-activation.patch'
    if($patchIdentity.size -ne $plan.inputs[6].size -or $patchIdentity.sha256 -ne $plan.inputs[6].sha256){throw 'ONNX ARM source patch differs'}
    $providerCmake=Join-Path $sourceRoot 'cmake\onnxruntime_providers_cpu.cmake'
    if((Sha $providerCmake) -ne '692191f92bfa9ad356875780c59c664de53aebce90e449fba619ec1ae71c44dc'){throw 'ONNX ARM source patch preimage differs'}
    $null=Run-Checked $patch @('--batch','--forward','--fuzz=0','-p1','-d',$sourceRoot.Replace('\','/'),'--input',$sourcePatch.Replace('\','/')) 'source-pinned ARM FP16 Conv activation helper link fix'
    if((Sha $providerCmake) -ne $plan.patched_provider_cmake_sha256){throw 'ONNX ARM source patch result differs'}
    $null=Run-Checked $python @('-c','import build_args; import flatbuffers; import util; print(build_args.__file__); print(flatbuffers.__file__); print(util.__file__)') 'isolated source-owned Python import closure'
    $null=Run-Checked $python $buildArguments 'reduced CPU ARM64 ONNX Runtime build'
    [IO.File]::AppendAllText((Join-Path $ReportRoot 'build.log'),"ONNX_ARM64_WINDOWS_BUILD_EXIT=0`n",$utf8)
    $buildRoot=Join-Path $sourceRoot 'build\Windows\Release'
    $dll=@(Get-ChildItem -LiteralPath $buildRoot -Recurse -File -Filter 'onnxruntime.dll')
    if($dll.Count -ne 1){throw 'ONNX ARM expected exactly one output DLL'}
    New-Item -ItemType Directory (Join-Path $OutputRoot 'bin')|Out-Null
    Copy-Item -LiteralPath $dll[0].FullName -Destination (Join-Path $OutputRoot 'bin\onnxruntime.dll')
    $projects=[ordered]@{}
    foreach($name in @('kleidiai','onnxruntime','onnxruntime_mlas','onnxruntime_providers')){
        $files=@(Get-ChildItem -LiteralPath $buildRoot -Recurse -File -Filter ($name+'.vcxproj'))
        if($files.Count -ne 1){throw "ONNX ARM expected exactly one generated $name project"}
        $projects[$name]=[IO.File]::ReadAllText($files[0].FullName)
    }
    $systemLabels=@($systemIds.Keys);[Array]::Sort($systemLabels,[StringComparer]::Ordinal)
    $systemRows=@($systemLabels|ForEach-Object{$systemIds[$_]})
    $capture=[ordered]@{schema=$plan.capture_schema;archive_tools=@($archiveIds.ToArray());system_tools=$systemRows;supporting_inputs=@($support.ToArray());network_denied_tools=$labels}
    $validation=[ordered]@{schema=$plan.validation_schema;work_root=$WorkRoot;tool_paths=$programs;network_denies=@($denies.ToArray());cache=[IO.File]::ReadAllText((Join-Path $buildRoot 'CMakeCache.txt'));build_log=[IO.File]::ReadAllText((Join-Path $ReportRoot 'build.log'));python_path_configuration=$pythonConfig;projects=$projects;build_arguments=$buildArguments;provider_sources_cmake=[IO.File]::ReadAllText($providerCmake)}
    $captureFile=Join-Path $ReportRoot 'capture.json';$validationFile=Join-Path $ReportRoot 'validation.log'
    Json-File $captureFile $capture;Json-File $validationFile $validation
    if((Sha $Inputs) -ne $inputMapSha){throw 'ONNX input-map snapshot changed'}
    $null=Run-Checked $recorder @('onnx-windows-arm64','record','--inputs',$Inputs,'--capture',$captureFile,'--validation',$validationFile,'--output-root',$OutputRoot,'--receipt',(Join-Path $ReportRoot 'receipt.json'),'--evidence',(Join-Path $ReportRoot 'evidence.json'),'--product-commit',$ExpectedProductCommit,'--cargo-lock-sha256',$ExpectedCargoLockSha256,'--builder-host',$BuilderHost) 'record original ONNX ARM controlled-build receipt'
    $null=Run-Checked $recorder @('onnx-windows-arm64','verify','--inputs',$Inputs,'--validation',$validationFile,'--output-root',$OutputRoot,'--receipt',(Join-Path $ReportRoot 'receipt.json'),'--evidence',(Join-Path $ReportRoot 'evidence.json')) 'admit original ONNX ARM packet'
    Write-Output 'ONNX_ARM64_WINDOWS_BUILD_OK'
} finally {
    foreach($name in $rules){Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue}
    $remaining=@(Get-NetFirewallRule -DisplayName ('solstone-onnx-arm64-'+$PID+'-*') -ErrorAction SilentlyContinue)
    if($remaining.Count -ne 0){throw 'ONNX ARM firewall cleanup incomplete'}
    Write-Output 'ONNX_ARM64_WINDOWS_FIREWALL_REMAINING=0'
}
