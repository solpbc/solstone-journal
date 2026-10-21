# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Produce CED ARM64 against one original admitted OpenMP development cohort.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$SourceArchive,
    [Parameter(Mandatory=$true)][string]$OpenMpInputs,
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
    if (Test-Path -LiteralPath $Directory) { throw "Fresh OpenMP root required: $Directory" }
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
    if ($programs.ContainsKey($Label)) { throw "Duplicate OpenMP system-tool label: $Label" }
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
    $RepositoryRoot=Full-Path $RepositoryRoot;$SourceArchive=Full-Path $SourceArchive;$OpenMpInputs=Full-Path $OpenMpInputs
    $WorkRoot=Full-Path $WorkRoot;$OutputRoot=Full-Path $OutputRoot;$ReportRoot=Full-Path $ReportRoot
    $roots=@($WorkRoot,$OutputRoot,$ReportRoot,$RepositoryRoot)
    for($i=0;$i -lt $roots.Count;$i++){
        for($j=$i+1;$j -lt $roots.Count;$j++){
            if($roots[$i].Equals($roots[$j],[StringComparison]::OrdinalIgnoreCase) -or ($roots[$i]+'\').StartsWith($roots[$j]+'\',[StringComparison]::OrdinalIgnoreCase) -or ($roots[$j]+'\').StartsWith($roots[$i]+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'CED ARM roots must be disjoint'}
        }
    }
    New-Root $WorkRoot;New-Root $OutputRoot;New-Root $ReportRoot
    [IO.File]::WriteAllText((Join-Path $ReportRoot 'build.log'),'',$utf8)
    $mapItem=Get-Item -LiteralPath $OpenMpInputs
    if($mapItem.PSIsContainer -or ($mapItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -or $mapItem.Length -gt 4194304){throw 'Bounded regular OpenMP input map required'}
    $inputMapBytes=[IO.File]::ReadAllBytes($OpenMpInputs)
    $OpenMpInputs=Join-Path $WorkRoot 'openmp-inputs.json'
    $mapStream=[IO.File]::Open($OpenMpInputs,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{$mapStream.Write($inputMapBytes,0,$inputMapBytes.Length);$mapStream.Flush($true)}finally{$mapStream.Dispose()}
    $original=[IO.File]::ReadAllText($OpenMpInputs)|ConvertFrom-Json
    $inputMapSha=Sha $OpenMpInputs
    $git=(Get-Command git.exe -ErrorAction Stop).Source;$cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    $head=(& $git --no-replace-objects -C $RepositoryRoot rev-parse HEAD).Trim()
    if($LASTEXITCODE -ne 0 -or $head -ne $ExpectedProductCommit){throw 'CED ARM product source differs'}
    $dirty=& $git --no-replace-objects -C $RepositoryRoot status --porcelain=v1 --untracked-files=all
    if($LASTEXITCODE -ne 0 -or $dirty){throw 'CED ARM product checkout is dirty'}
    if((Sha (Join-Path $RepositoryRoot 'core\Cargo.lock')) -ne $ExpectedCargoLockSha256){throw 'CED ARM Cargo lock differs'}
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
    $inputArgs=@('--source-archive',$SourceArchive,'--openmp-inputs',$OpenMpInputs)
    $raw=Run-Checked $recorder (@('ced-windows-arm64','verify-inputs')+$inputArgs) 'pinned CED/OpenMP input admission'
    $plan=[IO.File]::ReadAllText($raw)|ConvertFrom-Json
    if((Get-Item $SourceArchive).Length -ne $plan.source.size -or (Sha $SourceArchive) -ne $plan.source.sha256){throw 'CED source bytes changed after admission'}
    if((Sha $OpenMpInputs) -ne $inputMapSha){throw 'CED OpenMP input-map snapshot changed'}
    $sourceRoot=Join-Path $WorkRoot 'source';$llvmOuter=Join-Path $WorkRoot 'llvm-unpack';$cmakeOuter=Join-Path $WorkRoot 'cmake-unpack'
    $pythonRoot=Join-Path $WorkRoot 'python';$llvmRoot=Join-Path $WorkRoot 'llvm';$cmakeRoot=Join-Path $WorkRoot 'cmake';$openmpRoot=Join-Path $WorkRoot 'openmp'
    New-Item -ItemType Directory $sourceRoot,$llvmOuter,$cmakeOuter,$pythonRoot,(Join-Path $openmpRoot 'include'),(Join-Path $openmpRoot 'lib')|Out-Null
    $tar=Join-Path $env:SystemRoot 'System32\tar.exe'
    $null=Run-Checked $tar @('-xzf',$SourceArchive,'-C',$sourceRoot) 'extract verified CED/ggml source'
    $null=Run-Checked $tar @('-xf',([string]$original.llvm_archive),'-C',$llvmOuter) 'extract pinned LLVM host tools'
    Move-Item (Join-Path $llvmOuter 'clang+llvm-22.1.6-x86_64-pc-windows-msvc') $llvmRoot
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.cmake_archive,$cmakeOuter)
    Move-Item (Join-Path $cmakeOuter 'cmake-3.31.12-windows-x86_64') $cmakeRoot
    [IO.Compression.ZipFile]::ExtractToDirectory([string]$original.python_archive,$pythonRoot)
    Import-Arm-Environment $vcvars $cmd
    foreach($name in @('CL','_CL_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_PLATFORM','CMAKE_GENERATOR_TOOLSET','PYTHONHOME','PYTHONPATH')){Remove-Item ('Env:\'+$name) -ErrorAction SilentlyContinue}
    $pythonConfig="python312.zip`n.`n"
    [IO.File]::WriteAllText((Join-Path $pythonRoot 'python312._pth'),$pythonConfig,$utf8)
    foreach($member in @('include/omp.h','lib/libomp.lib')){
        $from=Join-Path ([string]$original.development_root) $member.Replace('/','\')
        $to=Join-Path $openmpRoot $member.Replace('/','\')
        $pin=@($plan.openmp_inputs|Where-Object{$_.label -eq ('dependencies/openmp/'+$member)})
        if($pin.Count -ne 1){throw 'OpenMP development pin missing'}
        $null=File-Identity $from ([string]$pin[0].label)
        Copy-Item -LiteralPath $from -Destination $to
        if((Get-Item $to).Length -ne $pin[0].size -or (Sha $to) -ne $pin[0].sha256){throw 'Copied OpenMP development input differs'}
    }
    $env:INCLUDE=(Join-Path $openmpRoot 'include')+';'+$env:INCLUDE
    $archiveIds=[Collections.Generic.List[object]]::new()
    foreach($tool in $plan.tools){
        $file=Join-Path $WorkRoot $tool.label.Replace('/','\');$observed=File-Identity $file $tool.label
        if($observed.size -ne $tool.size -or $observed.sha256 -ne $tool.sha256){throw 'Extracted CED host tool differs'}
        Native-Host-Program $file;$archiveIds.Add($observed);$programs.Add([string]$tool.label,$file)
    }
    $msvcArm=Join-Path $env:VCToolsInstallDir 'bin\Hostx64\arm64';$msvcHost=Join-Path $env:VCToolsInstallDir 'bin\Hostx64\x64'
    foreach($file in Get-ChildItem $msvcArm -Filter '*.exe' -File){if($file.Name -ne 'nmake.exe'){Add-System-Tool ('msvc/'+$file.Name.ToLowerInvariant()) $file.FullName}}
    $nmake=Join-Path $msvcHost 'nmake.exe';Add-System-Tool 'msvc/nmake.exe' $nmake
    $sdk=Join-Path $env:WindowsSdkDir ('bin\'+$env:WindowsSDKVersion.TrimEnd('\')+'\x64')
    foreach($file in Get-ChildItem $sdk -Filter '*.exe' -File){Add-System-Tool ('sdk/'+$file.Name.ToLowerInvariant()) $file.FullName}
    Add-System-Tool 'windows/cmd.exe' $cmd
    Add-System-Tool 'windows/powershell.exe' (Get-Process -Id $PID).Path
    Add-System-Tool 'windows/git.exe' $git
    Add-System-Tool 'windows/recorder.exe' $recorder
    foreach($label in @('msvc/link.exe','msvc/lib.exe','msvc/nmake.exe','sdk/rc.exe','sdk/mt.exe','windows/cmd.exe','windows/powershell.exe','windows/git.exe','windows/recorder.exe')){Native-Host-Program $programs[$label]}
    $labels=@($programs.Keys);[Array]::Sort($labels,[StringComparer]::Ordinal)
    if(@(Get-NetFirewallProfile|Where-Object{$_.Enabled -ne 'True'}).Count -ne 0){throw 'Windows Firewall profiles must be enabled'}
    $denies=[Collections.Generic.List[object]]::new()
    foreach($label in $labels){
        $name='solstone-ced-arm64-'+$PID+'-'+[Guid]::NewGuid().ToString('N')
        $null=New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $programs[$label] -Profile Any -Protocol Any -LocalAddress Any -RemoteAddress Any -LocalPort Any -RemotePort Any
        $rules.Add($name);$rule=Get-NetFirewallRule -DisplayName $name
        $app=$rule|Get-NetFirewallApplicationFilter;$address=$rule|Get-NetFirewallAddressFilter;$port=$rule|Get-NetFirewallPortFilter
        $protocol=[string]$port.Protocol;if($protocol -eq '256'){$protocol='Any'}
        $denies.Add([pscustomobject]@{label=$label;program=[string]$app.Program;enabled=($rule.Enabled -eq 'True');direction=[string]$rule.Direction;action=[string]$rule.Action;profile=[string]$rule.Profile;protocol=$protocol;local_address=@($address.LocalAddress);remote_address=@($address.RemoteAddress);local_port=@($port.LocalPort);remote_port=@($port.RemotePort)})
    }
    $clang=Join-Path $llvmRoot 'bin\clang-cl.exe';$cmake=Join-Path $cmakeRoot 'bin\cmake.exe';$python=Join-Path $pythonRoot 'python.exe'
    $buildRoot=Join-Path $WorkRoot 'build';$prefix=Join-Path $WorkRoot 'install'
    $cmakeArgs=@('-S',$sourceRoot,'-B',$buildRoot,'-G','NMake Makefiles')
    foreach($property in $plan.configuration.PSObject.Properties){if($property.Name -ne 'CMAKE_GENERATOR'){$cmakeArgs+=('-D'+$property.Name+'='+[string]$property.Value)}}
    $cmakeArgs+=@("-DCMAKE_C_COMPILER=$clang","-DCMAKE_CXX_COMPILER=$clang","-DCMAKE_ASM_COMPILER=$clang","-DPython3_EXECUTABLE=$python","-DCMAKE_MAKE_PROGRAM=$nmake","-DCMAKE_INSTALL_PREFIX=$prefix","-DOpenMP_libomp_LIBRARY=$openmpRoot\lib\libomp.lib","-DOpenMP_C_INCLUDE_DIR=$openmpRoot\include","-DOpenMP_CXX_INCLUDE_DIR=$openmpRoot\include")
    $null=Run-Checked $cmake $cmakeArgs 'configure ARM64 CED CPU library'
    $null=Run-Checked $cmake @('--build',$buildRoot,'--target','ced') 'serial NMake CED build'
    New-Item -ItemType Directory (Join-Path $OutputRoot 'bin')|Out-Null
    Copy-Item (Join-Path $buildRoot 'ced.dll') (Join-Path $OutputRoot 'bin\ced.dll')
    foreach($tool in $archiveIds){if((Sha $programs[$tool.label]) -ne $tool.sha256){throw 'Pinned CED host tool changed during compilation'}}
    foreach($tool in $systemIds.Values){if((Sha $programs[$tool.label]) -ne $tool.sha256){throw 'Unowned CED host tool changed during compilation'}}
    $development=@()
    foreach($member in @('include/omp.h','lib/libomp.lib')){
        $label='dependencies/openmp/'+$member;$file=Join-Path $openmpRoot $member.Replace('/','\')
        $observed=File-Identity $file $label;$pin=@($plan.openmp_inputs|Where-Object{$_.label -eq $label})
        if($pin.Count -ne 1 -or $observed.size -ne $pin[0].size -or $observed.sha256 -ne $pin[0].sha256){throw 'OpenMP development bytes changed during CED compilation'}
        $development+=$observed
    }
    if(@(Get-NetFirewallProfile|Where-Object{$_.Enabled -ne 'True'}).Count -ne 0){throw 'Windows Firewall was disabled during CED compilation'}
    [IO.File]::AppendAllText((Join-Path $ReportRoot 'build.log'),"CED_ARM64_WINDOWS_BUILD_EXIT=0`n",$utf8)
    $cacheText=[IO.File]::ReadAllText((Join-Path $buildRoot 'CMakeCache.txt'))
    Copy-Item (Join-Path $buildRoot 'CMakeCache.txt') (Join-Path $ReportRoot 'CMakeCache.txt')
    $cache=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
    foreach($line in [IO.File]::ReadAllLines((Join-Path $buildRoot 'CMakeCache.txt'))){
        if($line.StartsWith('#') -or $line.StartsWith('/') -or [string]::IsNullOrEmpty($line)){continue}
        $at=$line.IndexOf('=');if($at -lt 0){continue};$key=($line.Substring(0,$at) -split ':',2)[0]
        if($cache.ContainsKey($key)){throw "Duplicate CED CMake cache key: $key"};$cache.Add($key,$line.Substring($at+1))
    }
    $configuration=[ordered]@{}
    foreach($property in $plan.configuration.PSObject.Properties){if(-not $cache.ContainsKey($property.Name)){throw "CED cache key missing: $($property.Name)"};$configuration[$property.Name]=$cache[$property.Name]}
    $systemRows=@();$toolPaths=[ordered]@{}
    foreach($label in $labels){$toolPaths[$label]=$programs[$label];if($systemIds.ContainsKey($label)){$systemRows+=$systemIds[$label]}}
    $capture=[ordered]@{schema=$plan.capture_schema;archive_tools=@($archiveIds.ToArray());system_tools=$systemRows;network_denied_tools=$labels;configuration=$configuration;development=$development}
    $validation=[ordered]@{schema=$plan.validation_schema;work_root=$WorkRoot;tool_paths=$toolPaths;network_denies=@($denies.ToArray());cache=$cacheText;build_log=[IO.File]::ReadAllText((Join-Path $ReportRoot 'build.log'));install_manifest='';python_path_configuration=$pythonConfig}
    $captureFile=Join-Path $ReportRoot 'capture.json';$validationFile=Join-Path $ReportRoot 'validation.log'
    Json-File $captureFile $capture;Json-File $validationFile $validation
    if((Sha $OpenMpInputs) -ne $inputMapSha){throw 'CED OpenMP input-map snapshot changed during compilation'}
    $aggregateLog='record.log';[IO.File]::WriteAllText((Join-Path $ReportRoot $aggregateLog),'',$utf8)
    $recordArgs=@('ced-windows-arm64','record')+$inputArgs+@('--capture',$captureFile,'--validation',$validationFile,'--output-root',$OutputRoot,'--receipt',(Join-Path $ReportRoot 'receipt.json'),'--evidence',(Join-Path $ReportRoot 'evidence.json'),'--product-commit',$ExpectedProductCommit,'--cargo-lock-sha256',$ExpectedCargoLockSha256,'--builder-host',$BuilderHost)
    $recordLog=Run-Checked $recorder $recordArgs 'record original controlled ARM64 CED build'
    if([IO.File]::ReadAllText($recordLog) -notmatch 'CED_ARM64_WINDOWS_RECORD_PASS'){throw 'CED ARM recorder returned no marker'}
    $result=[pscustomobject]@{marker='CED_ARM64_CONTROLLED_BUILD_RECORDED';recorded_utc=[DateTime]::UtcNow.ToString('o');product_commit=$ExpectedProductCommit;output_sha256=(Sha (Join-Path $OutputRoot 'bin\ced.dll'));receipt_sha256=(Sha (Join-Path $ReportRoot 'receipt.json'));evidence_sha256=(Sha (Join-Path $ReportRoot 'evidence.json'));validation_sha256=(Sha $validationFile);native_execution=$false;installed_journal=$false}
}
finally {
    foreach($name in $rules){Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue}
    $remaining=@(Get-NetFirewallRule -ErrorAction Stop)
    foreach($name in $rules){if(@($remaining|Where-Object{$_.DisplayName -eq $name}).Count -ne 0){throw "CED firewall cleanup unconfirmed: $name"}}
}
$result|ConvertTo-Json -Depth 5 -Compress

