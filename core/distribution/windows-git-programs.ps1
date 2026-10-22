# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Resolve the implementation behind Git for Windows' stable cmd launcher.
# Return every installed Git executable so the caller can hash and fence it.
function Get-ControlledGitPrograms([string]$Git) {
    $saved=$ErrorActionPreference;$ErrorActionPreference='Continue'
    $global:LASTEXITCODE=$null
    try { $exec=(& $Git --exec-path);$code=$global:LASTEXITCODE }
    finally { $ErrorActionPreference=$saved }
    if($null -eq $code -or $code -ne 0 -or @($exec).Count -ne 1){throw 'Git execution-directory census failed'}
    $exec=[IO.Path]::GetFullPath(([string]$exec).Trim()).TrimEnd('\','/')
    if($exec.Replace('\','/') -notmatch '(?i)^[a-z]:/.+/libexec/git-core$'){throw 'Unsupported Git execution directory'}
    $prefix=Split-Path (Split-Path $exec -Parent) -Parent
    $root=Split-Path $prefix -Parent
    $aliases=[ordered]@{
        'windows/git.exe'=[IO.Path]::GetFullPath($Git)
        'windows/git-backend.exe'=(Join-Path $prefix 'bin\git.exe')
        'windows/git-core.exe'=(Join-Path $exec 'git.exe')
        'windows/git-http.exe'=(Join-Path $exec 'git-remote-https.exe')
    }
    $known=[Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    $rows=[Collections.Generic.List[object]]::new()
    foreach($label in $aliases.Keys){
        $file=[IO.Path]::GetFullPath([string]$aliases[$label])
        if(-not $file.StartsWith($root+'\',[StringComparison]::OrdinalIgnoreCase)){throw 'Git program escaped its installed root'}
        $item=Get-Item -LiteralPath $file
        if($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)){throw 'Ordinary Git executable required'}
        $null=$known.Add($file)
        $rows.Add([pscustomobject]@{label=$label;file=$file})
    }
    $count=0
    foreach($item in Get-ChildItem -LiteralPath $root -Recurse -File -Filter '*.exe'){
        $count+=1
        if($count -gt 4096){throw 'Git executable census exceeds its bound'}
        if($item.Attributes -band [IO.FileAttributes]::ReparsePoint){throw 'Git executable census contains a reparse point'}
        if($known.Add($item.FullName)){
            $relative=$item.FullName.Substring($root.Length+1).Replace('\','/').ToLowerInvariant()
            $algorithm=[Security.Cryptography.SHA256]::Create()
            try{$id=[BitConverter]::ToString($algorithm.ComputeHash([Text.Encoding]::UTF8.GetBytes($relative))).Replace('-','').ToLowerInvariant()}
            finally{$algorithm.Dispose()}
            $rows.Add([pscustomobject]@{label=('windows/git-'+$id+'.exe');file=$item.FullName})
        }
    }
    return $rows.ToArray()
}
