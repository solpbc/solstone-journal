# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Cross-build the pinned ordinary-backup restic command. Target execution is a
# separate native ARM64 gate. No signing or publication occurs here.
[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)][string]$RepositoryRoot,
    [Parameter(Mandatory=$true)][string]$SourceArchive,
    [Parameter(Mandatory=$true)][string]$GoArchive,
    [Parameter(Mandatory=$true)][string]$ModulesArchive,
    [Parameter(Mandatory=$true)][string]$WorkRoot,
    [Parameter(Mandatory=$true)][string]$OutputRoot,
    [Parameter(Mandatory=$true)][string]$ReportRoot,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory=$true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256
)
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
$ProgressPreference='SilentlyContinue'
$rules=[Collections.Generic.List[string]]::new()
function Sha([string]$file){(Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()}
function TextSha([string]$text){
    $hash=[Security.Cryptography.SHA256]::Create()
    try { return ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($text)))).Replace('-','').ToLowerInvariant() }
    finally { $hash.Dispose() }
}
function NewRoot([string]$path){
    if (-not [IO.Path]::IsPathRooted($path) -or (Test-Path -LiteralPath $path)){throw "Fresh absolute build root required: $path"}
    New-Item -ItemType Directory -Path $path | Out-Null
}
function Bound([string]$file,[long]$size,[string]$digest){
    if (-not (Test-Path -LiteralPath $file -PathType Leaf) -or (Get-Item -LiteralPath $file).Length -ne $size -or (Sha $file) -ne $digest){throw "Restic input differs from its pin: $file"}
}
function Checked([string]$label,[string]$program,[string[]]$arguments){
    $saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    try { & $program @arguments; $code=$LASTEXITCODE } finally {$ErrorActionPreference=$saved}
    if($code -ne 0){throw "$label failed with exit $code"}
}
$git=(Get-Command git.exe -ErrorAction Stop).Source
function SourceBinding {
    $commit=& $git --no-replace-objects -C $RepositoryRoot rev-parse HEAD
    if($LASTEXITCODE -ne 0 -or [string]$commit -ne $ExpectedProductCommit){throw 'Restic driver source commit differs'}
    $dirty=& $git --no-replace-objects -C $RepositoryRoot status --porcelain=v1 --untracked-files=all --ignore-submodules=none
    if($LASTEXITCODE -ne 0 -or $dirty){throw 'Restic driver requires a clean product checkout'}
    if((Sha (Join-Path $RepositoryRoot 'core\Cargo.lock')) -ne $ExpectedCargoLockSha256){throw 'Restic driver product lock differs'}
    if((Sha $PSCommandPath) -ne (Sha (Join-Path $RepositoryRoot 'core\distribution\restic-windows-build.ps1'))){throw 'Restic driver bytes differ from product source'}
}
SourceBinding
$inputs=@(
    [pscustomobject]@{label='sources/restic-0.19.0.tar.gz';size=24158333;sha256='800779b6c4c2396971c0567b09ccdd435e03155e1a0ec94e8bbf3d98641a8bc2'},
    [pscustomobject]@{label='tools/go1.26.4.windows-amd64.zip';size=74910419;sha256='3ca8fb4630b07c419cbdd51f754e31363cfcfb83b3a5354d9e895c90be2cc345'},
    [pscustomobject]@{label='sources/restic-0.19.0-offline-modules.tar.gz';size=129997737;sha256='5fa7af01cd61dbecaa97bf6aa942cfcd1b109c3ae5a5c20946210c9c8428e163'}
)
$archives=@($SourceArchive,$GoArchive,$ModulesArchive)
for($i=0;$i -lt $archives.Count;$i++){Bound $archives[$i] $inputs[$i].size $inputs[$i].sha256}
NewRoot $WorkRoot;NewRoot $OutputRoot;NewRoot $ReportRoot
$sourceRoot=Join-Path $WorkRoot 'source';$toolRoot=Join-Path $WorkRoot 'tools';$moduleRoot=Join-Path $WorkRoot 'modules'
$cacheRoot=Join-Path $WorkRoot 'build-cache';$tmpRoot=Join-Path $WorkRoot 'tmp'
New-Item -ItemType Directory $sourceRoot,$toolRoot,$moduleRoot,$cacheRoot,$tmpRoot | Out-Null
Checked 'extract restic source' 'tar.exe' @('-xf',$SourceArchive,'-C',$sourceRoot)
Checked 'extract offline modules' 'tar.exe' @('-xf',$ModulesArchive,'-C',$moduleRoot)
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::ExtractToDirectory($GoArchive,$toolRoot)
$source=Join-Path $sourceRoot 'restic-0.19.0'
$goMod='a0c67833f2ca1485f1272e1214d22cac65c56641c24a7fad1491cde8d7c21d9a'
$goSum='94840327111dc755890d04c23208d5b17f549846494d4225445cc70ba8ed0b2b'
if((Sha "$source\go.mod") -ne $goMod -or (Sha "$source\go.sum") -ne $goSum){throw 'Restic source module metadata differs'}
$manifestPath=Join-Path $moduleRoot 'offline-modules.json'
$manifestSha='61ae6c281a83df96aed10d1527f5f87d7420894da1f4a6f413affe0275fbbdb6'
if((Sha $manifestPath) -ne $manifestSha){throw 'Restic offline module manifest differs'}
$manifest=[IO.File]::ReadAllText($manifestPath)|ConvertFrom-Json
if($manifest.schema -ne 'solstone.restic-offline-modules.v1' -or $manifest.restic_version -ne '0.19.0' -or $manifest.go_mod_sha256 -ne $goMod -or $manifest.go_sum_sha256 -ne $goSum){throw 'Restic module manifest source differs'}
$seen=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
foreach($item in $manifest.files){
    if($item.path -notmatch '^cache/download/[A-Za-z0-9_+.!@~/-]+$' -or $item.path.Split('/') -contains '..' -or -not $seen.Add($item.path)){throw 'Unsafe or duplicate restic module path'}
    Bound (Join-Path $moduleRoot $item.path) $item.bytes $item.sha256
}
foreach($item in Get-ChildItem $moduleRoot -Recurse -Force){
    if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Restic modules contain a reparse point'}
    if(-not $item.PSIsContainer){$relative=$item.FullName.Substring($moduleRoot.Length+1).Replace('\','/');if($relative -ne 'offline-modules.json' -and -not $seen.Contains($relative)){throw 'Undeclared offline module member'}}
}
$goRoot=Join-Path $toolRoot 'go';$go=Join-Path $goRoot 'bin\go.exe'
$toolFiles=@(Get-ChildItem "$goRoot\bin","$goRoot\pkg\tool\windows_amd64" -Filter '*.exe' -File)
$byLabel=@{}
foreach($item in $toolFiles){
    $label=$item.FullName.Substring($toolRoot.Length+1).Replace('\','/')
    $bytes=[IO.File]::ReadAllBytes($item.FullName)
    if([BitConverter]::ToUInt16($bytes,[BitConverter]::ToInt32($bytes,0x3c)+4) -ne 0x8664){throw 'Foreign Go host tool'}
    $byLabel[$label]=[pscustomobject]@{label=$label;size=$item.Length;sha256=(Sha $item.FullName)}
}
$labels=[string[]]@($byLabel.Keys);[Array]::Sort($labels,[StringComparer]::Ordinal)
$tools=@($labels|ForEach-Object {$byLabel[$_]})
foreach($name in @('go/bin/go.exe','go/pkg/tool/windows_amd64/asm.exe','go/pkg/tool/windows_amd64/compile.exe','go/pkg/tool/windows_amd64/link.exe')){if(-not $byLabel.ContainsKey($name)){throw "Go host tool missing: $name"}}
# No inherited Go flags, alternate toolchain, remote cache program or workspace.
Get-ChildItem Env: | Where-Object {$_.Name.StartsWith('GO',[StringComparison]::OrdinalIgnoreCase)} | ForEach-Object {Remove-Item ('Env:\'+$_.Name)}
$environment=[ordered]@{GOOS='windows';GOARCH='arm64';GOARM64='v8.0';CGO_ENABLED='0';GOTOOLCHAIN='local';GOENV='off';GOWORK='off';GOFLAGS='-mod=readonly';GOPROXY='off';GOSUMDB='off'}
foreach($name in $environment.Keys){Set-Item ('Env:\'+$name) $environment[$name]}
$env:GOROOT=$goRoot;$env:GOMODCACHE=$moduleRoot;$env:GOCACHE=$cacheRoot;$env:GOPATH=Join-Path $WorkRoot 'gopath'
$env:TEMP=$tmpRoot;$env:TMP=$tmpRoot;$env:PATH="$goRoot\bin;$env:SystemRoot\System32;$env:SystemRoot"
$goVersion=& $go version
if($LASTEXITCODE -ne 0 -or [string]$goVersion -ne 'go version go1.26.4 windows/amd64'){throw 'Go host version differs'}
$logicalArguments=@('build','-p=2','-trimpath','-buildvcs=false','-tags=selfupdate,disable_grpc_modules','-ldflags=-s -w -X main.version=0.19.0','-o','restic.exe','./cmd/restic')
$output=Join-Path $OutputRoot 'restic.exe'
$arguments=@('build','-p=2','-trimpath','-buildvcs=false','-tags=selfupdate,disable_grpc_modules','-ldflags=-s -w -X main.version=0.19.0','-o',$output,'./cmd/restic')
$buildLog=Join-Path $ReportRoot 'go-build.log';$buildInfo=Join-Path $ReportRoot 'go-build-info.txt'
try {
    foreach($label in $labels){
        $program=Join-Path $toolRoot $label
        $name='solstone-restic-'+$PID+'-'+[Guid]::NewGuid().ToString('N')
        New-NetFirewallRule -DisplayName $name -Direction Outbound -Action Block -Program $program -Profile Any | Out-Null
        $rules.Add($name)
        $rule=Get-NetFirewallRule -DisplayName $name
        $filter=$rule|Get-NetFirewallApplicationFilter
        if([string]$rule.Enabled -ne 'True' -or [string]$rule.Direction -ne 'Outbound' -or [string]$rule.Action -ne 'Block' -or $filter.Program -ne $program){throw 'Restic network denial was not installed'}
    }
    Push-Location $source
    $saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    try {& $go @arguments *> $buildLog;$code=$LASTEXITCODE} finally {$ErrorActionPreference=$saved;Pop-Location}
    if($code -ne 0){throw "Restic cross build failed with exit $code; see $buildLog"}
    $logText=[IO.File]::ReadAllText($buildLog)+"`r`nRESTIC_WINDOWS_BUILD_EXIT=0`r`n"
    $saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    try {& $go version -m $output *> $buildInfo;$code=$LASTEXITCODE} finally {$ErrorActionPreference=$saved}
    if($code -ne 0){throw 'Go build-info inspection failed'}
    $infoText=[IO.File]::ReadAllText($buildInfo)
    foreach($tool in $tools){Bound (Join-Path $toolRoot $tool.label) $tool.size $tool.sha256}
    if((Sha "$source\go.mod") -ne $goMod -or (Sha "$source\go.sum") -ne $goSum){throw 'Go changed pinned source module metadata'}
    $bytes=[IO.File]::ReadAllBytes($output)
    if([BitConverter]::ToUInt16($bytes,[BitConverter]::ToInt32($bytes,0x3c)+4) -ne 0xAA64){throw 'Restic output is not native ARM64'}
    SourceBinding
    $validation=[ordered]@{schema='solstone.restic-windows-validation.v1';build_log=$logText;go_build_info=$infoText;network_denied_tools=@($labels)}
    $capture=[ordered]@{schema='solstone.restic-windows-build-evidence.v1';inputs=$inputs;module_manifest_sha256=$manifestSha;go_mod_sha256=$goMod;go_sum_sha256=$goSum;go_version=[string]$goVersion;tools=$tools;network_denied_tools=@($labels);arguments=$logicalArguments;environment=$environment;build_log_sha256=(TextSha $logText);go_build_info_sha256=(TextSha $infoText)}
    [IO.File]::WriteAllText((Join-Path $ReportRoot 'validation.log'),($validation|ConvertTo-Json -Depth 6 -Compress),[Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $ReportRoot 'capture.json'),($capture|ConvertTo-Json -Depth 6 -Compress),[Text.UTF8Encoding]::new($false))
}
finally {foreach($name in $rules){Remove-NetFirewallRule -DisplayName $name -ErrorAction Stop}}
Write-Output "RESTIC_WINDOWS_CROSS_BUILD_READY_FOR_ADMISSION $output"
