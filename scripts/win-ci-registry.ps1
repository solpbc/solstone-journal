# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Run a byte-bound copy so Cargo can rebuild the original during registry suites.
[CmdletBinding()]
param([string]$RepositoryRoot=(Split-Path -Parent $PSScriptRoot))
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'

function Invoke-WindowsRegistryCopy {
    [CmdletBinding()]
    param([Parameter(Mandatory=$true)][string]$Source,
          [Parameter(Mandatory=$true)][string]$CopyRoot,
          [Parameter(Mandatory=$true)][string[]]$Arguments)
    $sourceItem=Get-Item -LiteralPath $Source -ErrorAction Stop
    if ($sourceItem.PSIsContainer -or ($sourceItem.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'plain CI runner file required' }
    $parent=Get-Item -LiteralPath $CopyRoot -ErrorAction Stop
    if (!$parent.PSIsContainer -or ($parent.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'plain runner-copy parent required' }
    $before=(Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash.ToLowerInvariant()
    $directory=Join-Path $CopyRoot ('journal-win-registry-'+[Guid]::NewGuid().ToString('N'))
    $copy=Join-Path $directory 'solstone-ci.exe'
    $owned=$false
    $nativeExit=$null
    try {
        New-Item -ItemType Directory -Path $directory -ErrorAction Stop | Out-Null
        $owned=$true
        [IO.File]::Copy($Source,$copy,$false)
        if ((Get-FileHash -LiteralPath $copy -Algorithm SHA256).Hash.ToLowerInvariant() -cne $before -or
            (Get-FileHash -LiteralPath $Source -Algorithm SHA256).Hash.ToLowerInvariant() -cne $before) { throw 'CI runner changed during copy' }
        Write-Output "JOURNAL_WIN_CI_REGISTRY_RUNNER sha256=$before copy=$copy"
        & $copy @Arguments
        $nativeExit=$LASTEXITCODE
        if ($nativeExit -isnot [int]) { throw 'CI runner native exit missing' }
    } finally {
        if ($owned) {
            # Remove exactly our file and now-empty directory; never recurse.
            if (Test-Path -LiteralPath $copy) { [IO.File]::Delete($copy) }
            [IO.Directory]::Delete($directory,$false)
            if (Test-Path -LiteralPath $directory) { throw 'runner copy cleanup unverified' }
        }
    }
    $script:RegistryNativeExit=$nativeExit
}

if ($MyInvocation.InvocationName -ne '.') {
    $code=1
    try {
        Set-Location -LiteralPath $RepositoryRoot
        $script:RegistryNativeExit=$null
        Invoke-WindowsRegistryCopy -Source (Join-Path $RepositoryRoot 'core\target\debug\solstone-ci.exe') `
            -CopyRoot (Join-Path $RepositoryRoot 'core\target') `
            -Arguments @('windows-run','--coverage','core\target\journal-win-suite-coverage.json',
                '--owner-evidence','core\target\journal-win-owner-evidence.json',
                '--owner-nonce',$env:JOURNAL_WIN_CI_PREPARED_OWNER_NONCE,
                '--evidence-vars','core\target\journal-win-ci-evidence-vars.cmd')
        $code=$script:RegistryNativeExit
    } catch { Write-Error -ErrorRecord $_ -ErrorAction Continue }
    exit $code
}
