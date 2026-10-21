# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Build one native ARM64 OpenMP runtime and retain its original development pair.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$SourceArchive,
    [Parameter(Mandatory=$true)][string]$LlvmArchive,
    [Parameter(Mandatory=$true)][string]$CmakeArchive,
    [Parameter(Mandatory=$true)][string]$PythonArchive,
    [Parameter(Mandatory=$true)][string]$WorkRoot,
    [Parameter(Mandatory=$true)][string]$OutputRoot,
    [Parameter(Mandatory=$true)][string]$DevelopmentRoot,
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
    if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator) -or -not [Environment]::Is64BitProcess) { throw 'Administrative AMD64 builder required' }
    $RepositoryRoot=Full-Path $RepositoryRoot
    $SourceArchive=Full-Path $SourceArchive;$LlvmArchive=Full-Path $LlvmArchive
    $CmakeArchive=Full-Path $CmakeArchive;$PythonArchive=Full-Path $PythonArchive
    $WorkRoot=Full-Path $WorkRoot;$OutputRoot=Full-Path $OutputRoot
    $DevelopmentRoot=Full-Path $DevelopmentRoot;$ReportRoot=Full-Path $ReportRoot
    $roots=@($WorkRoot,$OutputRoot,$DevelopmentRoot,$ReportRoot,$RepositoryRoot)
    for ($i=0;$i -lt $roots.Count;$i++) {
        for ($j=$i+1;$j -lt $roots.Count;$j++) {
            if ($roots[$i].Equals($roots[$j],[StringComparison]::OrdinalIgnoreCase) -or ($roots[$i]+'\').StartsWith($roots[$j]+'\',[StringComparison]::OrdinalIgnoreCase) -or ($roots[$j]+'\').StartsWith($roots[$i]+'\',[StringComparison]::OrdinalIgnoreCase)) { throw 'OpenMP roots must be disjoint' }
        }
    }
    New-Root $WorkRoot;New-Root $OutputRoot;New-Root $DevelopmentRoot;New-Root $ReportRoot
    [IO.File]::WriteAllText((Join-Path $ReportRoot 'build.log'),'',$utf8)
    $git=(Get-Command git.exe -ErrorAction Stop).Source
    $cmd=Join-Path $env:SystemRoot 'System32\cmd.exe'
    $head=(& $git --no-replace-objects -C $RepositoryRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $head -ne $ExpectedProductCommit) { throw 'OpenMP product source commit differs' }
    $dirty=& $git --no-replace-objects -C $RepositoryRoot status --porcelain=v1 --untracked-files=all
    if ($LASTEXITCODE -ne 0 -or $dirty) { throw 'OpenMP product checkout is dirty' }
    if ((Sha (Join-Path $RepositoryRoot 'core\Cargo.lock')) -ne $ExpectedCargoLockSha256) { throw 'OpenMP Cargo lock differs' }
    Set-Location -LiteralPath $RepositoryRoot
    $vswhere=Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs=(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.ARM64 -property installationPath)
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($vs)) { throw 'MSVC ARM64 tools are absent' }
    $vcvars=Join-Path $vs 'VC\Auxiliary\Build\vcvarsall.bat'
    # Native profiles must run the source-pinned toolchain top-up before this
    # driver. Build the owned recorder for the builder, before the C++ fence.
    $batch=Join-Path $WorkRoot 'build-recorder.cmd'
    [IO.File]::WriteAllText($batch,"@echo off`r`ncall `"$vcvars`" x64 >nul`r`nif errorlevel 1 exit /b %errorlevel%`r`ncd /d `"$RepositoryRoot`"`r`nset CARGO_BUILD_JOBS=2`r`ncargo build --manifest-path core\Cargo.toml -p solstone-core-distribution --bin solstone-distribution --target x86_64-pc-windows-msvc --locked --offline`r`nexit /b %errorlevel%`r`n",[Text.Encoding]::ASCII)
    $null=Run-Checked $cmd @('/d','/s','/c',("`"{0}`"" -f $batch)) 'offline AMD64 recorder build'
    $recorder=Join-Path $RepositoryRoot 'core\target\x86_64-pc-windows-msvc\debug\solstone-distribution.exe'
    Native-Host-Program $recorder
    $inputArgs=@('--source-archive',$SourceArchive,'--llvm-archive',$LlvmArchive,'--cmake-archive',$CmakeArchive,'--python-archive',$PythonArchive)
    $raw=Run-Checked $recorder (@('openmp-windows','verify-inputs')+$inputArgs) 'pinned OpenMP input preflight'
    $plan=[IO.File]::ReadAllText($raw)|ConvertFrom-Json

    $sourceRoot=Join-Path $WorkRoot 'source'
    $llvmOuter=Join-Path $WorkRoot 'llvm-unpack';$cmakeOuter=Join-Path $WorkRoot 'cmake-unpack'
    $pythonRoot=Join-Path $WorkRoot 'python';$llvmRoot=Join-Path $WorkRoot 'llvm';$cmakeRoot=Join-Path $WorkRoot 'cmake'
    New-Item -ItemType Directory $sourceRoot,$llvmOuter,$cmakeOuter,$pythonRoot | Out-Null
    $tar=Join-Path $env:SystemRoot 'System32\tar.exe'
    $null=Run-Checked $tar @('-xf',$SourceArchive,'-C',$sourceRoot,'llvm-project-22.1.6.src/openmp','llvm-project-22.1.6.src/cmake','llvm-project-22.1.6.src/llvm/cmake','llvm-project-22.1.6.src/llvm/CMakeLists.txt') 'extract pinned OpenMP source subset'
    $null=Run-Checked $tar @('-xf',$LlvmArchive,'-C',$llvmOuter) 'extract pinned LLVM host tools'
    Move-Item -LiteralPath (Join-Path $llvmOuter 'clang+llvm-22.1.6-x86_64-pc-windows-msvc') -Destination $llvmRoot
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [IO.Compression.ZipFile]::ExtractToDirectory($CmakeArchive,$cmakeOuter)
    Move-Item -LiteralPath (Join-Path $cmakeOuter 'cmake-3.31.12-windows-x86_64') -Destination $cmakeRoot
    [IO.Compression.ZipFile]::ExtractToDirectory($PythonArchive,$pythonRoot)
    Import-Arm-Environment $vcvars $cmd
    foreach ($name in @('CL','_CL_','CFLAGS','CXXFLAGS','CPPFLAGS','LDFLAGS','CMAKE_TOOLCHAIN_FILE','CMAKE_GENERATOR','CMAKE_GENERATOR_PLATFORM','CMAKE_GENERATOR_TOOLSET','PYTHONHOME','PYTHONPATH')) { Remove-Item ('Env:\'+$name) -ErrorAction SilentlyContinue }
    $openmpSource=Join-Path $sourceRoot 'llvm-project-22.1.6.src\openmp'
    $pythonConfig="python312.zip`n.`n"+($openmpSource.Replace('\','/')+'/runtime/tools')+"`n"
    [IO.File]::WriteAllText((Join-Path $pythonRoot 'python312._pth'),$pythonConfig,$utf8)
    $archiveIds=[Collections.Generic.List[object]]::new()
    foreach ($tool in $plan.tools) {
        $file=Join-Path $WorkRoot $tool.label.Replace('/','\')
        $observed=File-Identity $file $tool.label
        if ($observed.size -ne $tool.size -or $observed.sha256 -ne $tool.sha256) { throw "Extracted host tool differs: $($tool.label)" }
        Native-Host-Program $file
        $archiveIds.Add($observed)
        $programs.Add([string]$tool.label,$file)
    }
    $msvcArm=Join-Path $env:VCToolsInstallDir 'bin\Hostx64\arm64'
    $msvcHost=Join-Path $env:VCToolsInstallDir 'bin\Hostx64\x64'
    foreach ($file in Get-ChildItem -LiteralPath $msvcArm -Filter '*.exe' -File) {
        if ($file.Name -ne 'nmake.exe') { Add-System-Tool ('msvc/'+$file.Name.ToLowerInvariant()) $file.FullName }
    }
    $nmake=Join-Path $msvcHost 'nmake.exe';Add-System-Tool 'msvc/nmake.exe' $nmake
    $sdk=Join-Path $env:WindowsSdkDir ('bin\'+$env:WindowsSDKVersion.TrimEnd('\')+'\x64')
    foreach ($file in Get-ChildItem -LiteralPath $sdk -Filter '*.exe' -File) { Add-System-Tool ('sdk/'+$file.Name.ToLowerInvariant()) $file.FullName }
    Add-System-Tool 'windows/cmd.exe' $cmd
    Add-System-Tool 'windows/powershell.exe' (Get-Process -Id $PID).Path
    Add-System-Tool 'windows/git.exe' $git
    Add-System-Tool 'windows/recorder.exe' $recorder
    # SDK tool directories also contain unowned I386 executables. Observe and
    # fence those candidates; only the selected build programs require AMD64.
    foreach ($label in @('msvc/link.exe','msvc/lib.exe','msvc/nmake.exe','sdk/rc.exe','sdk/mt.exe','windows/cmd.exe','windows/powershell.exe','windows/git.exe','windows/recorder.exe')) {
        Native-Host-Program $programs[$label]
    }
    $labels=@($programs.Keys);[Array]::Sort($labels,[StringComparer]::Ordinal)
    if (@(Get-NetFirewallProfile | Where-Object { $_.Enabled -ne 'True' }).Count -ne 0) { throw 'Windows Firewall profiles must be enabled' }
    $denies=[Collections.Generic.List[object]]::new()
    foreach ($label in $labels) {
        $name='solstone-openmp-'+$PID+'-'+[Guid]::NewGuid().ToString('N')
        $null=New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $programs[$label] -Profile Any -Protocol Any -LocalAddress Any -RemoteAddress Any -LocalPort Any -RemotePort Any
        $rules.Add($name)
        $rule=Get-NetFirewallRule -DisplayName $name
        $app=$rule|Get-NetFirewallApplicationFilter
        $address=$rule|Get-NetFirewallAddressFilter
        $port=$rule|Get-NetFirewallPortFilter
        $protocol=[string]$port.Protocol;if ($protocol -eq '256') {$protocol='Any'}
        $denies.Add([pscustomobject]@{label=$label;program=[string]$app.Program;enabled=($rule.Enabled -eq 'True');direction=[string]$rule.Direction;action=[string]$rule.Action;profile=[string]$rule.Profile;protocol=$protocol;local_address=@($address.LocalAddress);remote_address=@($address.RemoteAddress);local_port=@($port.LocalPort);remote_port=@($port.RemotePort)})
    }
    $clang=Join-Path $llvmRoot 'bin\clang-cl.exe'
    $cmake=Join-Path $cmakeRoot 'bin\cmake.exe'
    $python=Join-Path $pythonRoot 'python.exe'
    $buildRoot=Join-Path $WorkRoot 'build';$prefix=Join-Path $WorkRoot 'install'
    $cmakeArgs=@('-S',$openmpSource,'-B',$buildRoot,'-G','NMake Makefiles')
    foreach ($property in $plan.configuration.PSObject.Properties) {
        if ($property.Name -ne 'CMAKE_GENERATOR') { $cmakeArgs+=('-D'+$property.Name+'='+[string]$property.Value) }
    }
    $cmakeArgs+=@("-DCMAKE_C_COMPILER=$clang","-DCMAKE_CXX_COMPILER=$clang","-DCMAKE_ASM_COMPILER=$clang","-DPython3_EXECUTABLE=$python","-DCMAKE_MAKE_PROGRAM=$nmake","-DCMAKE_INSTALL_PREFIX=$prefix")
    $null=Run-Checked $cmake $cmakeArgs 'configure ARM64 OpenMP CPU runtime'
    $null=Run-Checked $cmake @('--build',$buildRoot,'--target','install') 'serial NMake OpenMP install'
    Copy-Item -LiteralPath (Join-Path $prefix 'bin\libomp.dll') -Destination (Join-Path $OutputRoot 'libomp.dll')
    Copy-Item -LiteralPath (Join-Path $openmpSource 'LICENSE.TXT') -Destination (Join-Path $OutputRoot 'LICENSE.txt')
    New-Item -ItemType Directory (Join-Path $DevelopmentRoot 'include'),(Join-Path $DevelopmentRoot 'lib') | Out-Null
    Copy-Item -LiteralPath (Join-Path $prefix 'include\omp.h') -Destination (Join-Path $DevelopmentRoot 'include\omp.h')
    Copy-Item -LiteralPath (Join-Path $prefix 'lib\libomp.lib') -Destination (Join-Path $DevelopmentRoot 'lib\libomp.lib')
    foreach ($tool in $archiveIds) {
        if ((Sha $programs[$tool.label]) -ne $tool.sha256) { throw 'Pinned host tool changed during compilation' }
    }
    foreach ($tool in $systemIds.Values) {
        if ((Sha $programs[$tool.label]) -ne $tool.sha256) { throw 'Unowned host tool changed during compilation' }
    }
    if (@(Get-NetFirewallProfile | Where-Object { $_.Enabled -ne 'True' }).Count -ne 0) { throw 'Windows Firewall was disabled during compilation' }
    [IO.File]::AppendAllText((Join-Path $ReportRoot 'build.log'),"OPENMP_WINDOWS_BUILD_EXIT=0`n",$utf8)
    $cacheText=[IO.File]::ReadAllText((Join-Path $buildRoot 'CMakeCache.txt'))
    $installText=[IO.File]::ReadAllText((Join-Path $buildRoot 'install_manifest.txt'))
    $buildText=[IO.File]::ReadAllText((Join-Path $ReportRoot 'build.log'))
    Copy-Item -LiteralPath (Join-Path $buildRoot 'CMakeCache.txt') -Destination (Join-Path $ReportRoot 'CMakeCache.txt')
    Copy-Item -LiteralPath (Join-Path $buildRoot 'install_manifest.txt') -Destination (Join-Path $ReportRoot 'install_manifest.txt')
    Copy-Item -LiteralPath (Join-Path $pythonRoot 'python312._pth') -Destination (Join-Path $ReportRoot 'python312._pth')
    $cache=[Collections.Generic.Dictionary[string,string]]::new([StringComparer]::Ordinal)
    foreach ($line in [IO.File]::ReadAllLines((Join-Path $buildRoot 'CMakeCache.txt'))) {
        if ($line.StartsWith('#') -or $line.StartsWith('/') -or [string]::IsNullOrEmpty($line)) { continue }
        $at=$line.IndexOf('=');if ($at -lt 0) { continue }
        $key=($line.Substring(0,$at) -split ':',2)[0]
        if ($cache.ContainsKey($key)) { throw "Duplicate CMake cache key: $key" }
        $cache.Add($key,$line.Substring($at+1))
    }
    $configuration=[ordered]@{}
    foreach ($property in $plan.configuration.PSObject.Properties) {
        if (-not $cache.ContainsKey($property.Name)) { throw "CMake cache key missing: $($property.Name)" }
        $configuration[$property.Name]=$cache[$property.Name]
    }
    $development=@()
    foreach ($label in @('include/omp.h','lib/libomp.lib')) {
        $development+=File-Identity (Join-Path $DevelopmentRoot $label.Replace('/','\')) $label
    }
    $systemRows=@();$toolPaths=[ordered]@{}
    foreach ($label in $labels) {
        $toolPaths[$label]=$programs[$label]
        if ($systemIds.ContainsKey($label)) { $systemRows+=$systemIds[$label] }
    }
    $capture=[ordered]@{
        schema='solstone.openmp-windows-build-evidence.v1';inputs=@($plan.inputs)
        archive_tools=@($archiveIds.ToArray());system_tools=$systemRows
        network_denied_tools=$labels;configuration=$configuration;development=$development
        cache_sha256=(Text-Sha $cacheText);build_log_sha256=(Text-Sha $buildText)
        install_manifest_sha256=(Text-Sha $installText)
    }
    $validation=[ordered]@{
        schema='solstone.openmp-windows-validation.v1';work_root=$WorkRoot
        tool_paths=$toolPaths;network_denies=@($denies.ToArray());cache=$cacheText
        build_log=$buildText;install_manifest=$installText;python_path_configuration=$pythonConfig
    }
    $captureFile=Join-Path $ReportRoot 'capture.json';$validationFile=Join-Path $ReportRoot 'validation.log'
    Json-File $captureFile $capture;Json-File $validationFile $validation
    # Freeze the exact compiler log already bound by capture/validation.
    $aggregateLog='record.log'
    [IO.File]::WriteAllText((Join-Path $ReportRoot $aggregateLog),'',$utf8)
    $recordArgs=@('openmp-windows','record')+$inputArgs+@('--capture',$captureFile,'--validation',$validationFile,'--output-root',$OutputRoot,'--development-root',$DevelopmentRoot,'--receipt',(Join-Path $ReportRoot 'receipt.json'),'--evidence',(Join-Path $ReportRoot 'evidence.json'),'--product-commit',$ExpectedProductCommit,'--cargo-lock-sha256',$ExpectedCargoLockSha256,'--builder-host',$BuilderHost)
    $recordLog=Run-Checked $recorder $recordArgs 'record original ARM64 OpenMP build'
    if ([IO.File]::ReadAllText($recordLog) -notmatch 'OPENMP_WINDOWS_RECORD_PASS') { throw 'OpenMP recorder returned no result marker' }
    $result=[pscustomobject]@{
        marker='OPENMP_CONTROLLED_BUILD_RECORDED';recorded_utc=[DateTime]::UtcNow.ToString('o')
        product_commit=$ExpectedProductCommit;output_sha256=(Sha (Join-Path $OutputRoot 'libomp.dll'))
        header_sha256=(Sha (Join-Path $DevelopmentRoot 'include\omp.h'))
        import_library_sha256=(Sha (Join-Path $DevelopmentRoot 'lib\libomp.lib'))
        receipt_sha256=(Sha (Join-Path $ReportRoot 'receipt.json'))
        evidence_sha256=(Sha (Join-Path $ReportRoot 'evidence.json'))
        validation_sha256=(Sha $validationFile);native_execution=$false;installed_journal=$false
    }
}
finally {
    foreach ($name in $rules) { Remove-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue }
    foreach ($name in $rules) {
        if (@(Get-NetFirewallRule -DisplayName $name -ErrorAction SilentlyContinue).Count -ne 0) { throw "OpenMP firewall cleanup unconfirmed: $name" }
    }
}
$result | ConvertTo-Json -Depth 5 -Compress
