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
  bool cli=args.Length==9 && args[0]=="windows-run";
  if(!cli && args.Length!=2)return 31;
  string source=cli ? Environment.GetEnvironmentVariable("JOURNAL_REGISTRY_CONTROL_SOURCE") : args[0];
  try { using(var file=new FileStream(source,FileMode.Open,FileAccess.ReadWrite,FileShare.None)) { if(file.Length<1)return 32; } }
  catch(IOException) { Console.WriteLine("ORIGINAL_LOCKED");return 33; }
  Console.WriteLine("ORIGINAL_WRITABLE");
  return Int32.Parse(cli ? Environment.GetEnvironmentVariable("JOURNAL_REGISTRY_CONTROL_EXIT") : args[1]);
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
    # Measure the shipped -File entry point in its own Windows PowerShell
    # process. Dot-sourcing alone cannot catch parameter-initialization errors.
    $repo=Join-Path $root 'repo'
    $scripts=Join-Path $repo 'scripts'
    $target=Join-Path $repo 'core\target'
    $debug=Join-Path $target 'x86_64-pc-windows-msvc\debug'
    New-Item -ItemType Directory -Path $scripts,$debug -ErrorAction Stop | Out-Null
    $runner=Join-Path $scripts 'win-ci-registry.ps1'
    [IO.File]::Copy((Join-Path $PSScriptRoot 'win-ci-registry.ps1'),$runner,$false)
    $original=Join-Path $debug 'solstone-ci.exe'
    [IO.File]::Copy($source,$original,$false)
    foreach ($expected in @(0,9)) {
        $info=[Diagnostics.ProcessStartInfo]::new()
        $info.FileName=Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
        $info.Arguments='-NoProfile -ExecutionPolicy Bypass -File "'+$runner+'"'
        $info.WorkingDirectory=$root
        $info.UseShellExecute=$false
        $info.RedirectStandardOutput=$true
        $info.RedirectStandardError=$true
        $info.EnvironmentVariables['JOURNAL_REGISTRY_CONTROL_SOURCE']=$original
        $info.EnvironmentVariables['JOURNAL_REGISTRY_CONTROL_EXIT']=[string]$expected
        $info.EnvironmentVariables['JOURNAL_WIN_CI_PREPARED_OWNER_NONCE']='registry-control-only'
        $process=[Diagnostics.Process]::Start($info)
        try {
            $stdout=$process.StandardOutput.ReadToEndAsync()
            $stderr=$process.StandardError.ReadToEndAsync()
            if (!$process.WaitForExit(30000)) { $process.Kill(); $null=$process.WaitForExit(5000); throw 'registry CLI control timed out' }
            if (!$stdout.Wait(5000) -or !$stderr.Wait(5000)) { throw 'registry CLI control streams incomplete' }
            if ($process.ExitCode -ne $expected -or $stderr.Result.Length -ne 0 -or
                $stdout.Result -notmatch 'JOURNAL_WIN_CI_REGISTRY_RUNNER sha256=' -or
                $stdout.Result -notmatch 'ORIGINAL_WRITABLE') { throw ('registry CLI default root or native exit control failed: '+$process.ExitCode+' '+$stderr.Result) }
            if ((Get-FileHash -LiteralPath $original -Algorithm SHA256).Hash -cne $before -or
                @(Get-ChildItem -LiteralPath $target -Directory | Where-Object { $_.Name -like 'journal-win-registry-*' }).Count -ne 0) { throw 'registry CLI changed its original or leaked a copy directory' }
        } finally { $process.Dispose() }
        $count++
    }
} finally { Remove-Item -LiteralPath $root -Recurse -Force }
if (Test-Path -LiteralPath $root) { throw 'registry control cleanup unverified' }
Write-Output "JOURNAL_WIN_CI_REGISTRY_COPY_CONTROLS=passed count=$count"
exit 0
