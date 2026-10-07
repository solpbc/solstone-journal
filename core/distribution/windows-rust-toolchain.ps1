# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Both controlled drivers supply their existing bounded Unowned capture,
# environment restoration and firewall/evidence functions. Never invoke a
# rustup cargo/rustc shim as the program admitted by a firewall rule.
function Select-WindowsRustToolchain([string]$SourceRoot, [string]$ReportRoot) {
    foreach ($name in @('RUSTUP_TOOLCHAIN','RUSTC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER',
        'CARGO_BUILD_RUSTC','CARGO_BUILD_RUSTC_WRAPPER','CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER')) {
        if (-not [string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($name, 'Process'))) {
            throw "inherited Rust selection override refused: $name"
        }
    }
    $configuration = Join-Path $SourceRoot 'rust-toolchain.toml'
    Require-File $configuration
    $configurationHash = Digest $configuration
    $channels = [regex]::Matches([IO.File]::ReadAllText($configuration),
        '(?m)^channel\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"\s*\r?$')
    if ($channels.Count -ne 1) { throw 'source must declare one exact Rust release' }
    $channel = $channels[0].Groups[1].Value + '-x86_64-pc-windows-msvc'
    $rustup = (Get-Command rustup.exe -CommandType Application -ErrorAction Stop).Source
    Require-File $rustup
    Add-Deny $rustup
    # Every target and component the source pins must already be installed.
    # Otherwise the first rustup call below reaches for the network to finish
    # the toolchain, which these drivers have already denied, and fails with a
    # download error instead of naming what is missing. The build image is
    # older than the source whenever rust-toolchain.toml gains an entry.
    $configurationText = [IO.File]::ReadAllText($configuration)
    $wanted = @()
    foreach ($key in @('targets','components')) {
        $list = [regex]::Match($configurationText, "(?ms)^$key\s*=\s*\[(.*?)\]")
        if ($list.Success) {
            foreach ($item in [regex]::Matches($list.Groups[1].Value, '"([^"]+)"')) {
                $wanted += [pscustomobject]@{kind=$key.TrimEnd('s'); name=$item.Groups[1].Value}
            }
        }
    }
    Invoke-Native 'rust-installed-targets' $rustup @('target','list','--installed','--toolchain',$channel) $ReportRoot 30
    Invoke-Native 'rust-installed-components' $rustup @('component','list','--installed','--toolchain',$channel) $ReportRoot 30
    $installedTargets = @((Log-Text 'rust-installed-targets') -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $installedComponents = @((Log-Text 'rust-installed-components') -split "`r?`n" | ForEach-Object { $_.Trim() } | Where-Object { $_ })
    $missing = @(foreach ($item in $wanted) {
        if ($item.kind -eq 'target') {
            if ($installedTargets -notcontains $item.name) { "target $($item.name)" }
        } elseif (-not @($installedComponents | Where-Object { $_ -eq $item.name -or $_.StartsWith("$($item.name)-") }).Count) {
            "component $($item.name)"
        }
    })
    if ($missing.Count -gt 0) {
        throw ("Rust toolchain $channel lacks $($missing -join ', ') from rust-toolchain.toml; " +
            "run 'rustup toolchain install' in the source checkout while the network is open, then rerun")
    }
    $selected = [ordered]@{}
    foreach ($tool in @('cargo','rustc')) {
        # `which` without --install only reads the installed selection. Compare
        # source-context selection with the exact source pin, including host.
        Invoke-Native "rust-selected-$tool" $rustup @('which',$tool) $SourceRoot 30
        Invoke-Native "rust-pinned-$tool" $rustup @('which','--toolchain',$channel,$tool) $SourceRoot 30
        $actual = (Log-Text "rust-selected-$tool").Trim()
        $pinned = (Log-Text "rust-pinned-$tool").Trim()
        if ($actual -match '[\r\n]' -or -not [IO.Path]::IsPathRooted($actual) -or
            -not $actual.Equals($pinned, [StringComparison]::OrdinalIgnoreCase) -or
            -not [IO.Path]::GetFileName($actual).Equals("$tool.exe", [StringComparison]::OrdinalIgnoreCase)) {
            throw "Rust selection differs from source pin: $tool"
        }
        Require-File $actual
        $selected[$tool] = $actual
    }
    $bin = Split-Path -Parent $selected.cargo
    if (-not $bin.Equals((Split-Path -Parent $selected.rustc), [StringComparison]::OrdinalIgnoreCase)) {
        throw 'cargo and rustc must belong to the same selected toolchain'
    }
    if ((Digest $configuration) -cne $configurationHash) { throw 'Rust source configuration changed during selection' }
    # Cargo and its build scripts receive this exact compiler. Prepending its
    # directory also makes nested cargo invocations select the admitted file.
    Set-BuildEnvironment 'RUSTC' $selected.rustc
    Set-BuildEnvironment 'PATH' ($bin+';'+$env:PATH)
    foreach ($tool in @('cargo','rustc')) {
        $applications = @(Get-Command "$tool.exe" -CommandType Application -ErrorAction Stop)
        $sourceProperty = $null
        if ($applications.Count -gt 0 -and $null -ne $applications[0]) {
            $sourceProperty = $applications[0].PSObject.Properties['Source']
        }
        if ($null -eq $sourceProperty -or $sourceProperty.Value -isnot [string] -or
            [string]::IsNullOrEmpty($sourceProperty.Value)) {
            throw "first Rust PATH application has no scalar nonempty Source: $tool"
        }
        $resolved = $sourceProperty.Value
        if (-not $resolved.Equals($selected[$tool], [StringComparison]::OrdinalIgnoreCase)) {
            throw "selected Rust tool is not first on PATH: $tool"
        }
    }
    Write-NewJson (Join-Path $ReportRoot 'rust-toolchain.json') ([ordered]@{
        source_configuration=$configuration; source_configuration_sha256=$configurationHash; channel=$channel;
        rustup=[ordered]@{path=$rustup;sha256=(Digest $rustup)};
        cargo=[ordered]@{path=$selected.cargo;sha256=(Digest $selected.cargo)};
        rustc=[ordered]@{path=$selected.rustc;sha256=(Digest $selected.rustc)};
        compiler_environment=$selected.rustc
    })
    return $selected
}
