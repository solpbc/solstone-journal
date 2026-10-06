# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$RepositoryRoot,
    [Parameter(Mandatory = $true)][string]$SourceArchive,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$SourceSha256,
    [Parameter(Mandatory = $true)][int64]$SourceSize,
    [Parameter(Mandatory = $true)][string]$BundleArchive,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$BundleSha256,
    [Parameter(Mandatory = $true)][int64]$BundleSize,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ManifestSha256,
    [Parameter(Mandatory = $true)][string]$WorkRoot,
    [Parameter(Mandatory = $true)][string]$OutputRoot,
    [Parameter(Mandatory = $true)][string]$ReportRoot,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{40}$')][string]$ExpectedProductCommit,
    [Parameter(Mandatory = $true)][ValidatePattern('^[0-9a-f]{64}$')][string]$ExpectedCargoLockSha256,
    [Parameter(Mandatory = $true)][string]$BuilderHost,
    [Parameter(Mandatory = $true)][string]$TransportPeer
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$utf8 = [Text.UTF8Encoding]::new($false, $true)
$rules = [Collections.Generic.List[string]]::new()
$token = [Guid]::NewGuid().ToString('N')
$environmentChanges = @{}
$reachedCopy = $false

function Require-NewRoot([string]$Path) {
    if (Test-Path -LiteralPath $Path) { throw "root path already exists: $Path" }
}

function Require-File([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "required file absent: $Path" }
    if ((Get-Item -LiteralPath $Path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "reparse file refused: $Path" }
}

function Digest([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Write-NewText([string]$Path, [string]$Text) {
    $bytes = $utf8.GetBytes($Text)
    $file = [IO.FileStream]::new($Path, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::Read)
    try { $file.Write($bytes, 0, $bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
}

function Write-NewJson([string]$Path, $Value) {
    Write-NewText $Path (ConvertTo-Json -InputObject $Value -Depth 15)
}

function Set-BuildEnvironment([string]$Name, [string]$Value) {
    if (-not $environmentChanges.ContainsKey($Name)) {
        $environmentChanges[$Name] = [Environment]::GetEnvironmentVariable($Name, 'Process')
    }
    [Environment]::SetEnvironmentVariable($Name, $Value, 'Process')
}

function ConvertTo-Argument([string]$Value) {
    if ($Value.IndexOf([char]0) -ge 0) { throw 'NUL in subprocess argument' }
    $text = [Text.StringBuilder]::new()
    [void]$text.Append('"')
    $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') { $slashes++; continue }
        if ($character -eq '"') {
            [void]$text.Append(('\' * (2 * $slashes + 1)))
        } else {
            [void]$text.Append(('\' * $slashes))
        }
        [void]$text.Append($character)
        $slashes = 0
    }
    [void]$text.Append(('\' * (2 * $slashes)))
    [void]$text.Append('"')
    return $text.ToString()
}

function Invoke-Native {
    param([string]$Label, [string]$Command, [string[]]$Arguments, [string]$Cwd, [int]$Seconds = 120)
    $logDir = Join-Path $WorkRoot 'driver-logs'
    if (-not (Test-Path -LiteralPath $logDir)) {
        New-Item -ItemType Directory -Path $logDir -Force | Out-Null
    }
    $stdout = Join-Path $logDir "$Label.stdout"
    $stderr = Join-Path $logDir "$Label.stderr"
    $psi = [Diagnostics.ProcessStartInfo]::new()
    $psi.FileName = $Command
    $encoded = foreach ($arg in $Arguments) { ConvertTo-Argument $arg }
    $psi.Arguments = $encoded -join ' '
    $psi.WorkingDirectory = $Cwd
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $proc = [Diagnostics.Process]::Start($psi)
    $stdoutTask = $proc.StandardOutput.ReadToEndAsync()
    $stderrTask = $proc.StandardError.ReadToEndAsync()
    if (-not $proc.WaitForExit($Seconds * 1000)) {
        try { $proc.Kill() } catch {}
        throw "$Label timed out after $Seconds seconds"
    }
    $stdoutText = $stdoutTask.GetAwaiter().GetResult()
    $stderrText = $stderrTask.GetAwaiter().GetResult()
    [IO.File]::WriteAllText($stdout, $stdoutText, $utf8)
    [IO.File]::WriteAllText($stderr, $stderrText, $utf8)
    if ($proc.ExitCode -ne 0) {
        throw "$Label exited $($proc.ExitCode): $stderrText"
    }
}

function Log-Text([string]$Label) {
    $logDir = Join-Path $WorkRoot 'driver-logs'
    [IO.File]::ReadAllText((Join-Path $logDir "$Label.stdout"), $utf8)
}

function Add-Deny([string]$Program) {
    Require-File $Program
    $ruleName = "solstone-nvattest-deny-$token-$($rules.Count)"
    $rules.Add($ruleName)
    netsh advfirewall firewall add rule name="$ruleName" dir=out action=block program="$Program" profile=any enable=yes | Out-Null
}

function Test-TcpProbe([string]$HostName, [int]$Port, [int]$TimeoutMs = 3000) {
    $client = [Net.Sockets.TcpClient]::new()
    try {
        $ar = $client.BeginConnect($HostName, $Port, $null, $null)
        if (-not $ar.AsyncWaitHandle.WaitOne($TimeoutMs, $false)) {
            return $false
        }
        $client.EndConnect($ar)
        return $true
    } catch {
        return $false
    } finally {
        $client.Dispose()
    }
}

function Decrement-AddressBytes([byte[]]$bytes) {
    $copy = [byte[]]$bytes.Clone()
    for ($i = $copy.Length - 1; $i -ge 0; $i--) {
        if ($copy[$i] -gt 0) {
            $copy[$i]--
            break
        }
        $copy[$i] = 255
    }
    return $copy
}

function Increment-AddressBytes([byte[]]$bytes) {
    $copy = [byte[]]$bytes.Clone()
    for ($i = $copy.Length - 1; $i -ge 0; $i--) {
        if ($copy[$i] -lt 255) {
            $copy[$i]++
            break
        }
        $copy[$i] = 0
    }
    return $copy
}

function Format-Address([byte[]]$bytes) {
    return ([Net.IPAddress]::new($bytes)).ToString()
}

function Is-AllZeros([byte[]]$bytes) {
    foreach ($b in $bytes) { if ($b -ne 0) { return $false } }
    return $true
}

function Is-AllOnes([byte[]]$bytes) {
    foreach ($b in $bytes) { if ($b -ne 255) { return $false } }
    return $true
}

try {
    # 1. Require-NewRoot and pre-extract validation
    Require-NewRoot $WorkRoot
    Require-NewRoot $OutputRoot
    Require-NewRoot $ReportRoot

    Require-File $SourceArchive
    Require-File $BundleArchive

    $actualSourceSize = (Get-Item -LiteralPath $SourceArchive).Length
    $actualSourceSha = Digest $SourceArchive
    if ($actualSourceSize -ne $SourceSize -or $actualSourceSha -ne $SourceSha256) {
        throw "source archive size or sha256 mismatch: expected $SourceSha256 ($SourceSize bytes), got $actualSourceSha ($actualSourceSize bytes)"
    }
    $actualBundleSize = (Get-Item -LiteralPath $BundleArchive).Length
    $actualBundleSha = Digest $BundleArchive
    if ($actualBundleSize -ne $BundleSize -or $actualBundleSha -ne $BundleSha256) {
        throw "bundle archive size or sha256 mismatch: expected $BundleSha256 ($BundleSize bytes), got $actualBundleSha ($actualBundleSize bytes)"
    }

    if ($ManifestSha256 -ne '6fe151b377b80c894135b4b42e65d4bdbfdcd4e170fea9c197b16fdaa833921f') {
        throw 'ManifestSha256 does not equal pinned 6fe151b377b80c894135b4b42e65d4bdbfdcd4e170fea9c197b16fdaa833921f'
    }

    New-Item -ItemType Directory -Path $WorkRoot, $OutputRoot, $ReportRoot -Force | Out-Null

    $gitStatus = (& git -C $RepositoryRoot status --porcelain=v1 --untracked-files=all --ignore-submodules=none)
    if ($LASTEXITCODE -ne 0 -or (-not [string]::IsNullOrWhiteSpace($gitStatus))) {
        throw "git repository is dirty: $gitStatus"
    }
    $gitCommit = (& git -C $RepositoryRoot rev-parse HEAD).Trim()
    if ($LASTEXITCODE -ne 0 -or $gitCommit -ne $ExpectedProductCommit) {
        throw "git commit mismatch: expected $ExpectedProductCommit, got $gitCommit"
    }
    $cargoLock = Join-Path $RepositoryRoot 'core\Cargo.lock'
    Require-File $cargoLock
    $actualLockSha = Digest $cargoLock
    if ($actualLockSha -ne $ExpectedCargoLockSha256) {
        throw "Cargo.lock sha256 mismatch: expected $ExpectedCargoLockSha256, got $actualLockSha"
    }

    # 2. Clear process environment to allowlist and set Path
    $allowedEnv = @(
        'SystemRoot','SystemDrive','windir','ComSpec','PATHEXT',
        'ProgramFiles','ProgramFiles(x86)','ProgramW6432',
        'USERPROFILE','HOMEDRIVE','HOMEPATH','USERNAME',
        'TEMP','TMP','PROCESSOR_ARCHITECTURE','NUMBER_OF_PROCESSORS',
        'APPDATA','LOCALAPPDATA'
    )
    $currentVars = [Environment]::GetEnvironmentVariables('Process')
    foreach ($key in $currentVars.Keys) {
        if ($allowedEnv -notcontains $key) {
            [Environment]::SetEnvironmentVariable($key, $null, 'Process')
        }
    }
    $systemRoot = $env:SystemRoot
    if ([string]::IsNullOrEmpty($systemRoot)) { $systemRoot = 'C:\Windows' }
    $env:PATH = "$env:USERPROFILE\.cargo\bin;$systemRoot\System32;$systemRoot;$systemRoot\System32\Wbem"
    $env:NVAT_SOURCE_COMMIT = '8fdbb0f8c10594a5f88f77fdec4766803b4e6d59'

    # 3. Select Rust toolchain
    . (Join-Path $RepositoryRoot 'core\distribution\windows-rust-toolchain.ps1')
    Select-WindowsRustToolchain $RepositoryRoot $ReportRoot

    # 4. Build solstone-distribution and verify inputs
    Invoke-Native 'cargo-build-distribution' 'cargo.exe' @(
        'build', '--manifest-path', (Join-Path $RepositoryRoot 'core\Cargo.toml'),
        '-p', 'solstone-core-distribution', '--bin', 'solstone-distribution',
        '--locked', '--offline'
    ) $RepositoryRoot 300

    $distribution = Join-Path $RepositoryRoot 'core\target\debug\solstone-distribution.exe'
    Require-File $distribution

    Invoke-Native 'verify-inputs' $distribution @(
        'nvattest-windows', 'verify-inputs',
        '--source-archive', $SourceArchive,
        '--bundle-archive', $BundleArchive
    ) $RepositoryRoot 60

    # 5. Environment and Tool Census via vswhere and vcvars64.bat
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    Require-File $vswhere
    $vsInstall = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath).Trim()
    if ([string]::IsNullOrWhiteSpace($vsInstall)) { throw 'Visual Studio with VC.Tools.x86.x64 is required' }
    $vcvars64 = Join-Path $vsInstall 'VC\Auxiliary\Build\vcvars64.bat'
    Require-File $vcvars64
    $vcvarsLines = cmd.exe /c "`"$vcvars64`" >nul && set"
    if ($LASTEXITCODE -ne 0) { throw "vcvars64.bat execution failed with exit code $LASTEXITCODE" }
    $vcEnv = @{}
    foreach ($line in $vcvarsLines) {
        if ($line -match '^(.*?)=(.*)$') {
            $k = $matches[1]
            $v = $matches[2]
            $vcEnv[$k] = $v
            [Environment]::SetEnvironmentVariable($k, $v, 'Process')
        }
    }

    $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

    $rustcPath = (Get-Command rustc.exe -CommandType Application -ErrorAction Stop).Source
    $cargoPath = (Get-Command cargo.exe -CommandType Application -ErrorAction Stop).Source
    Require-File $rustcPath
    Require-File $cargoPath
    $rustcSha = Digest $rustcPath
    $cargoSha = Digest $cargoPath

    $rustcVer = (& $rustcPath -V).Trim()
    $cargoVer = (& $cargoPath -V).Trim()

    $vcToolsVersion = $vcEnv['VCToolsVersion']
    if ([string]::IsNullOrEmpty($vcToolsVersion)) { throw 'VCToolsVersion is missing from vcvars environment' }

    $windowsSdkVersion = $vcEnv['WindowsSDKVersion']
    if ([string]::IsNullOrEmpty($windowsSdkVersion)) { throw 'WindowsSDKVersion is missing from vcvars environment' }
    if (-not $windowsSdkVersion.EndsWith('\') -or $windowsSdkVersion.EndsWith('\\')) {
        throw "WindowsSDKVersion must end with exactly one trailing backslash: $windowsSdkVersion"
    }

    # Extract cmake from bundle member cmake-3.31.12-windows-x86_64.zip
    $cmakeExtractRoot = Join-Path $WorkRoot 'tools-cmake'
    New-Item -ItemType Directory -Path $cmakeExtractRoot -Force | Out-Null
    tar.exe -xf $BundleArchive -C $cmakeExtractRoot cmake-3.31.12-windows-x86_64.zip
    $cmakeZip = Join-Path $cmakeExtractRoot 'cmake-3.31.12-windows-x86_64.zip'
    Require-File $cmakeZip
    tar.exe -xf $cmakeZip -C $cmakeExtractRoot
    $cmakeExe = Join-Path $cmakeExtractRoot 'cmake-3.31.12-windows-x86_64\bin\cmake.exe'
    Require-File $cmakeExe
    $cmakeSha = Digest $cmakeExe
    $cmakeVerOutput = (& $cmakeExe --version)
    $cmakeVer = $cmakeVerOutput[0].Trim()

    $censusObj = [ordered]@{
        schema = 'solstone.nvattest-windows-tool-census.v1'
        rustc = [ordered]@{
            version = $rustcVer
            path = $rustcPath
            sha256 = $rustcSha
        }
        cargo = [ordered]@{
            version = $cargoVer
            path = $cargoPath
            sha256 = $cargoSha
        }
        cmake = [ordered]@{
            version = $cmakeVer
            path = $cmakeExe
            sha256 = $cmakeSha
        }
        msvc = [ordered]@{
            vc_tools_version = $vcToolsVersion
        }
        windows_sdk = [ordered]@{
            version = $windowsSdkVersion
        }
        vs = [ordered]@{
            product_version = $vcEnv['VisualStudioVersion']
        }
    }
    $censusPath = Join-Path $ReportRoot 'tool-census.json'
    Write-NewJson $censusPath $censusObj

    # 6. Network controls
    if ($TransportPeer -eq '1.1.1.1') { throw 'TransportPeer cannot be 1.1.1.1' }
    if (-not (Test-TcpProbe '1.1.1.1' 443 5000)) {
        throw 'positive network control failed: could not connect to 1.1.1.1:443 before firewall rules'
    }
    $networkPositive = 'connected'

    # Install outbound firewall complement rules
    $isIpv6 = $TransportPeer.Contains(':')
    if (-not $isIpv6) {
        $peerBytes = [Net.IPAddress]::Parse($TransportPeer).GetAddressBytes()
        if (-not (Is-AllZeros $peerBytes)) {
            $lowerEnd = Format-Address (Decrement-AddressBytes $peerBytes)
            $r1 = "0.0.0.0-${lowerEnd}"
            $rn1 = "solstone-nvattest-net-$token-1"
            $rules.Add($rn1)
            netsh advfirewall firewall add rule name="$rn1" dir=out action=block remoteip="$r1" profile=any enable=yes | Out-Null
        }
        if (-not (Is-AllOnes $peerBytes)) {
            $upperStart = Format-Address (Increment-AddressBytes $peerBytes)
            $r2 = "${upperStart}-255.255.255.255"
            $rn2 = "solstone-nvattest-net-$token-2"
            $rules.Add($rn2)
            netsh advfirewall firewall add rule name="$rn2" dir=out action=block remoteip="$r2" profile=any enable=yes | Out-Null
        }
        $rn6 = "solstone-nvattest-net-$token-v6"
        $rules.Add($rn6)
        netsh advfirewall firewall add rule name="$rn6" dir=out action=block remoteip="::/0" profile=any enable=yes | Out-Null
    } else {
        # Block all IPv4
        $rn4 = "solstone-nvattest-net-$token-v4"
        $rules.Add($rn4)
        netsh advfirewall firewall add rule name="$rn4" dir=out action=block remoteip="0.0.0.0-255.255.255.255" profile=any enable=yes | Out-Null

        # Block IPv6 complement
        $peerBytes = [Net.IPAddress]::Parse($TransportPeer).GetAddressBytes()
        if (-not (Is-AllZeros $peerBytes)) {
            $lowerEnd = Format-Address (Decrement-AddressBytes $peerBytes)
            $r1 = "::-${lowerEnd}"
            $rn6_1 = "solstone-nvattest-net-$token-v6-1"
            $rules.Add($rn6_1)
            netsh advfirewall firewall add rule name="$rn6_1" dir=out action=block remoteip="$r1" profile=any enable=yes | Out-Null
        }
        if (-not (Is-AllOnes $peerBytes)) {
            $upperStart = Format-Address (Increment-AddressBytes $peerBytes)
            $r2 = "${upperStart}-ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"
            $rn6_2 = "solstone-nvattest-net-$token-v6-2"
            $rules.Add($rn6_2)
            netsh advfirewall firewall add rule name="$rn6_2" dir=out action=block remoteip="$r2" profile=any enable=yes | Out-Null
        }
    }

    if (Test-TcpProbe '1.1.1.1' 443 3000) {
        throw 'negative network control failed: probe connected while firewall rules were active'
    }
    $networkNegative = 'refused'

    # 7. Extract source and run SDK refusal controls
    $sourceExtract = Join-Path $WorkRoot 'source'
    New-Item -ItemType Directory -Path $sourceExtract -Force | Out-Null
    tar.exe -xf $SourceArchive -C $sourceExtract
    $sdkBuildScript = Join-Path $sourceExtract 'sol\windows\build.ps1'
    Require-File $sdkBuildScript

    # Refusal 1: wrong -OfflineManifestSha256
    $rootRefusal1 = Join-Path $WorkRoot 'build-refusal-manifest'
    $proc1 = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sdkBuildScript,
        '-Root', $rootRefusal1, '-OfflineBundle', $BundleArchive,
        '-OfflineManifestSha256', '0000000000000000000000000000000000000000000000000000000000000000'
    ) -Wait -PassThru -NoNewWindow -RedirectStandardError (Join-Path $WorkRoot 'refusal1.stderr')
    $refusal1Stderr = [IO.File]::ReadAllText((Join-Path $WorkRoot 'refusal1.stderr'), $utf8)
    if ($proc1.ExitCode -eq 0 -or -not $refusal1Stderr.Contains('offline manifest does not match the caller-bound digest')) {
        throw "refusal 1 failed: exit code $($proc1.ExitCode), stderr: $refusal1Stderr"
    }
    $refusalManifestExit = $proc1.ExitCode
    $refusalManifestBoundary = 'offline manifest does not match the caller-bound digest'

    # Refusal 2: -ReuseDependencies with -OfflineBundle
    $rootRefusal2 = Join-Path $WorkRoot 'build-refusal-reuse'
    $proc2 = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sdkBuildScript,
        '-Root', $rootRefusal2, '-ReuseDependencies', '-OfflineBundle', $BundleArchive,
        '-OfflineManifestSha256', $ManifestSha256
    ) -Wait -PassThru -NoNewWindow -RedirectStandardError (Join-Path $WorkRoot 'refusal2.stderr')
    $refusal2Stderr = [IO.File]::ReadAllText((Join-Path $WorkRoot 'refusal2.stderr'), $utf8)
    if ($proc2.ExitCode -eq 0 -or -not $refusal2Stderr.Contains('offline builds cannot reuse dependencies')) {
        throw "refusal 2 failed: exit code $($proc2.ExitCode), stderr: $refusal2Stderr"
    }
    $refusalReuseExit = $proc2.ExitCode
    $refusalReuseBoundary = 'offline builds cannot reuse dependencies'

    # Refusal 3: corrupt bundle copy
    $corruptTree = Join-Path $WorkRoot 'corrupt-bundle-tree'
    New-Item -ItemType Directory -Path $corruptTree -Force | Out-Null
    tar.exe -xf $BundleArchive -C $corruptTree
    $manifestJsonPath = Join-Path $corruptTree 'offline-manifest.json'
    Require-File $manifestJsonPath
    $manifestObj = Get-Content -LiteralPath $manifestJsonPath -Raw | ConvertFrom-Json
    $targetEntry = $null
    foreach ($f in $manifestObj.files) {
        if ($f.path -ne 'offline-manifest.json') {
            $targetEntry = $f
            break
        }
    }
    if ($null -eq $targetEntry) { throw 'no file in manifest to corrupt' }
    $targetFilePath = Join-Path $corruptTree $targetEntry.path
    Require-File $targetFilePath
    $origTargetBytes = [IO.File]::ReadAllBytes($targetFilePath)
    $corruptTargetBytes = [byte[]]$origTargetBytes.Clone()
    $corruptTargetBytes[0] = $corruptTargetBytes[0] -bxor 0xFF
    [IO.File]::WriteAllBytes($targetFilePath, $corruptTargetBytes)

    $corruptBundleTar = Join-Path $WorkRoot 'corrupt-bundle.tar'
    Push-Location $corruptTree
    try {
        tar.exe -cf $corruptBundleTar *
    } finally {
        Pop-Location
    }

    $rootRefusal3 = Join-Path $WorkRoot 'build-refusal-corrupt'
    $proc3 = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sdkBuildScript,
        '-Root', $rootRefusal3, '-OfflineBundle', $corruptBundleTar,
        '-OfflineManifestSha256', $ManifestSha256
    ) -Wait -PassThru -NoNewWindow -RedirectStandardError (Join-Path $WorkRoot 'refusal3.stderr')
    $refusal3Stderr = [IO.File]::ReadAllText((Join-Path $WorkRoot 'refusal3.stderr'), $utf8)

    [IO.File]::WriteAllBytes($targetFilePath, $origTargetBytes)
    $restoredSha = Digest $targetFilePath
    if ($restoredSha -ne $targetEntry.sha256) {
        throw "restored file sha256 mismatch for $($targetEntry.path): expected $($targetEntry.sha256), got $restoredSha"
    }
    Remove-Item -LiteralPath $corruptTree -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $corruptBundleTar -Force -ErrorAction SilentlyContinue

    if ($proc3.ExitCode -eq 0 -or -not $refusal3Stderr.Contains('missing or changed offline input')) {
        throw "refusal 3 failed: exit code $($proc3.ExitCode), stderr: $refusal3Stderr"
    }
    $refusalCorruptExit = $proc3.ExitCode
    $refusalCorruptBoundary = 'missing or changed offline input'

    # 8. One real build
    $realBuildRoot = Join-Path $WorkRoot 'build-real'
    $validationLog = Join-Path $ReportRoot 'validation.log'
    $realStdout = Join-Path $WorkRoot 'real-build.stdout'
    $realStderr = Join-Path $WorkRoot 'real-build.stderr'
    $procReal = Start-Process -FilePath 'powershell.exe' -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $sdkBuildScript,
        '-Root', $realBuildRoot, '-OfflineBundle', $BundleArchive,
        '-OfflineManifestSha256', $ManifestSha256
    ) -Wait -PassThru -NoNewWindow -RedirectStandardOutput $realStdout -RedirectStandardError $realStderr
    if ($procReal.ExitCode -ne 0) {
        $errOut = [IO.File]::ReadAllText($realStderr, $utf8)
        throw "real SDK build failed with exit code $($procReal.ExitCode): $errOut"
    }

    $transcript = [IO.File]::ReadAllText($realStdout, $utf8)
    [IO.File]::WriteAllText($validationLog, $transcript, $utf8)

    $builtExe = Join-Path $realBuildRoot 'bin\nvattest.exe'
    $builtMsvcp = Join-Path $realBuildRoot 'bin\msvcp140.dll'
    $builtVcruntime = Join-Path $realBuildRoot 'bin\vcruntime140.dll'
    $builtVcruntime1 = Join-Path $realBuildRoot 'bin\vcruntime140_1.dll'
    $builtLicense = Join-Path $realBuildRoot 'LICENSE'

    Require-File $builtExe
    Require-File $builtMsvcp
    Require-File $builtVcruntime
    Require-File $builtVcruntime1
    Require-File $builtLicense

    $dumpbinPath = (Get-Command dumpbin.exe -CommandType Application -ErrorAction Stop).Source
    Require-File $dumpbinPath
    $dumpbinOutput = (& $dumpbinPath /dependents $builtExe)
    [IO.File]::AppendAllText($validationLog, "`n`n=== dumpbin /dependents nvattest.exe ===`n" + ($dumpbinOutput -join "`n"), $utf8)

    foreach ($dll in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
        $dllPath = Join-Path $realBuildRoot "bin\$dll"
        $dOut = (& $dumpbinPath /dependents $dllPath)
        [IO.File]::AppendAllText($validationLog, "`n`n=== dumpbin /dependents $dll ===`n" + ($dOut -join "`n"), $utf8)
    }

    # 9. Copy into OutputRoot
    $outBin = Join-Path $OutputRoot 'bin'
    New-Item -ItemType Directory -Path $outBin -Force | Out-Null
    Copy-Item -LiteralPath $builtExe -Destination (Join-Path $outBin 'nvattest.exe')
    Copy-Item -LiteralPath $builtMsvcp -Destination (Join-Path $outBin 'msvcp140.dll')
    Copy-Item -LiteralPath $builtVcruntime -Destination (Join-Path $outBin 'vcruntime140.dll')
    Copy-Item -LiteralPath $builtVcruntime1 -Destination (Join-Path $outBin 'vcruntime140_1.dll')
    Copy-Item -LiteralPath $builtLicense -Destination (Join-Path $OutputRoot 'LICENSE')

    $reachedCopy = $true
}
finally {
    # 10. Clean up only rules added by this script
    foreach ($rule in $rules) {
        netsh advfirewall firewall delete rule name="$rule" | Out-Null
    }
    $rulesRemaining = 0
    foreach ($rule in $rules) {
        $check = netsh advfirewall firewall show rule name="$rule"
        if ($LASTEXITCODE -eq 0 -and ($check -join ' ').Contains($rule)) {
            $rulesRemaining++
        }
    }

    if ($reachedCopy) {
        if ($rulesRemaining -ne 0) {
            throw 'network-rules: firewall rules cleanup incomplete'
        }

        $realReport = Join-Path $realBuildRoot 'build-report.json'
        $evidenceOut = Join-Path $ReportRoot 'nvattest-build-evidence.json'
        $receiptOut = Join-Path $ReportRoot 'receipt.json'

        & $distribution nvattest-windows record `
            --repo $RepositoryRoot `
            --source-archive $SourceArchive `
            --bundle-archive $BundleArchive `
            --output-root $OutputRoot `
            --report $realReport `
            --census $censusPath `
            --validation $validationLog `
            --evidence $evidenceOut `
            --receipt $receiptOut `
            --product-commit $ExpectedProductCommit `
            --cargo-lock-sha256 $ExpectedCargoLockSha256 `
            --builder-host $BuilderHost `
            --toolchain "MSVC $vcToolsVersion" `
            --bundle-path $BundleArchive `
            --manifest-sha256 $ManifestSha256 `
            --refusal-manifest-exit $refusalManifestExit `
            --refusal-manifest-boundary $refusalManifestBoundary `
            --refusal-reuse-exit $refusalReuseExit `
            --refusal-reuse-boundary $refusalReuseBoundary `
            --refusal-corrupt-exit $refusalCorruptExit `
            --refusal-corrupt-boundary $refusalCorruptBoundary `
            --network-positive $networkPositive `
            --network-negative $networkNegative `
            --network-rules-remaining $rulesRemaining
        if ($LASTEXITCODE -ne 0) { throw "record failed with exit code $LASTEXITCODE" }

        & $distribution nvattest-windows verify `
            --receipt $receiptOut `
            --output-root $OutputRoot
        if ($LASTEXITCODE -ne 0) { throw "verify failed with exit code $LASTEXITCODE" }
    }
}
