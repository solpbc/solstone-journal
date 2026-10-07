# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
$ProgressPreference = 'SilentlyContinue'
$request = [Console]::In.ReadToEnd() | ConvertFrom-Json
$policy = New-Object -ComObject HNetCfg.FwPolicy2
$active = [int]$policy.CurrentProfileTypes
$enabled = 0
foreach ($profile in @(1, 2, 4)) {
    if (($active -band $profile) -ne 0 -and $policy.FirewallEnabled($profile)) {
        $enabled = $enabled -bor $profile
    }
}
$rules = @()
foreach ($rule in $policy.Rules) {
    if ([string]::Equals($rule.ApplicationName, $request.executable, [StringComparison]::OrdinalIgnoreCase)) {
        if ($rules.Count -ge 256) { throw 'too many application rules' }
        $rules += @{
            enabled = [bool]$rule.Enabled
            direction = [int]$rule.Direction
            action = [int]$rule.Action
            profiles = [int]$rule.Profiles
            protocol = [int]$rule.Protocol
            local_ports = [string]$rule.LocalPorts
            local_addresses = [string]$rule.LocalAddresses
            remote_addresses = [string]$rule.RemoteAddresses
            interface_types = [string]$rule.InterfaceTypes
            service_name = [string]$rule.ServiceName
        }
    }
}
@{ enabled_profiles = $enabled; rules = @($rules) } | ConvertTo-Json -Depth 4 -Compress
