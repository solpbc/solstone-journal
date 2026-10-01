# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Native Windows file-lock and exit controls; no suite coverage credit.
Set-StrictMode -Version Latest
$ErrorActionPreference='Stop'
. (Join-Path $PSScriptRoot 'win-ci-registry.ps1')
$root=Join-Path ([IO.Path]::GetTempPath()) ('journal-registry-control-'+[Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root -ErrorAction Stop | Out-Null
$source=Join-Path $root 'source.exe'
$count=0
try {
    Add-Type -OutputAssembly $source -OutputType ConsoleApplication -TypeDefinition @'
using System;
using System.IO;
public static class RegistryLockControl {
 public static int Main(string[] args) {
  if(args.Length!=2)return 31;
  try { using(var file=new FileStream(args[0],FileMode.Open,FileAccess.ReadWrite,FileShare.None)) { if(file.Length<1)return 32; } }
  catch(IOException) { Console.WriteLine("ORIGINAL_LOCKED");return 33; }
  Console.WriteLine("ORIGINAL_WRITABLE");return Int32.Parse(args[1]);
 }
}
'@
    $before=(Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
    & $source $source '0'
    if ($LASTEXITCODE -ne 33) { throw 'direct-running-executable lock control did not refuse' }
    $count++
    foreach ($expected in @(0,9)) {
        $script:RegistryNativeExit=$null
        $rows=@(Invoke-WindowsRegistryCopy -Source $source -CopyRoot $root -Arguments @($source,[string]$expected))
        if ($script:RegistryNativeExit -isnot [int] -or $script:RegistryNativeExit -ne $expected -or
            @($rows | Where-Object { $_ -ceq 'ORIGINAL_WRITABLE' }).Count -ne 1) { throw 'copy did not prove original writable and exact native exit' }
        if ((Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash -cne $before -or
            @(Get-ChildItem -LiteralPath $root -Directory).Count -ne 0) { throw 'source changed or copy directory survived' }
        $count++
    }
    Write-Output "JOURNAL_WIN_CI_REGISTRY_COPY_CONTROLS=passed count=$count"
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
