# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Unshipped environment-restoring host tools for Windows ARM FFmpeg builds.

function ConvertTo-FfmpegProxyWideLiteral([string]$Text) {
    return 'L"'+$Text.Replace('\','\\').Replace('"','\"').Replace("`r",'\r').Replace("`n",'\n')+'"'
}

function New-FfmpegWindowsArm64ProxySource {
    param(
        [Parameter(Mandatory=$true)][ValidateSet('host-cc','arm-as','arm-link','arm-lib','arm-nm')][string]$Role,
        [Parameter(Mandatory=$true)][string]$Program,
        [Parameter(Mandatory=$true)][hashtable]$Environment
    )
    if($Program -notmatch '^[A-Za-z]:[\\/]' -or -not $Program.EndsWith('.exe',[StringComparison]::OrdinalIgnoreCase) -or $Program.Contains([char]0)){
        throw 'Absolute native executable path required for FFmpeg compiler proxy'
    }
    foreach($key in @('PATH','INCLUDE','LIB','LIBPATH')){
        if(-not $Environment.ContainsKey($key) -or $Environment[$key] -isnot [string] -or $Environment[$key].Contains([char]0)){
            throw 'Complete native SDK environment required for FFmpeg compiler proxy'
        }
    }
    $lines=[Collections.Generic.List[string]]::new()
    $lines.Add('#include <process.h>');$lines.Add('#include <stdlib.h>');$lines.Add('#include <stdint.h>');$lines.Add('#include <wchar.h>')
    $lines.Add('static wchar_t *quoted(const wchar_t *s) { size_t n=wcslen(s), at=0, slashes=0; wchar_t *out=calloc(n*2+3,sizeof(*out)); if(!out) return NULL; out[at++]=L''"''; for(size_t i=0;i<=n;i++) { wchar_t c=s[i]; if(c==L''\\'') { slashes++; continue; } size_t count=(c==L''"'' || c==0) ? slashes*2 : slashes; for(size_t j=0;j<count;j++) out[at++]=L''\\''; slashes=0; if(c==L''"'') out[at++]=L''\\''; if(c==0) break; out[at++]=c; } out[at++]=L''"''; out[at]=0; return out; }')
    $lines.Add('int wmain(int argc, wchar_t **argv) {')
    foreach($key in @('PATH','INCLUDE','LIB','LIBPATH')){$lines.Add('if (_wputenv_s('+(ConvertTo-FfmpegProxyWideLiteral $key)+','+(ConvertTo-FfmpegProxyWideLiteral $environment[$key])+') != 0) return 125;')}
    $lines.Add('const wchar_t **child = calloc((size_t)argc + 2, sizeof(*child)); if (!child) return 125;')
    $lines.Add('child[0] = '+(ConvertTo-FfmpegProxyWideLiteral $program)+';')
    if($Role -eq 'host-cc'){$lines.Add('child[1] = L"--target=x86_64-pc-windows-msvc"; for (int i=1;i<argc;i++) child[i+1]=argv[i];')}
    elseif($Role -eq 'arm-as'){$lines.Add('int at=1; for (int i=1;i<argc;i++) { if(wcscmp(argv[i],L"/MT")==0) continue; child[at++]=wcscmp(argv[i],L"/O2")==0 ? L"-O2" : argv[i]; }')}
    else{$lines.Add('for (int i=1;i<argc;i++) child[i]=argv[i];')}
    $lines.Add('const wchar_t *program=child[0]; for(int i=0;child[i];i++) { child[i]=quoted(child[i]); if(!child[i]) return 125; } intptr_t code = _wspawnv(_P_WAIT, program, child); for(int i=0;child[i];i++) free((void *)child[i]); free(child); return code == -1 ? 126 : (int)code; }')
    return ($lines -join "`n")
}
