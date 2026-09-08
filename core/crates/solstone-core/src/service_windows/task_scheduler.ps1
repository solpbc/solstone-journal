# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
# Invoked by the Unowned OS-manager control adapter with fixed script and JSON stdin.
# Task Scheduler COM calls are isolated in this process so the caller deadline
# also covers a stalled RPC. No command text is accepted from the request.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false, $true)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false, $true)

function Is-Missing($Exception) {
    $errorObject = $Exception
    while ($null -ne $errorObject) {
        $code = [int64]$errorObject.HResult -band 4294967295L
        if ($code -eq 2147942402L -or $code -eq 2147942403L -or $code -eq 2147750669L) {
            return $true
        }
        $errorObject = $errorObject.InnerException
    }
    return $false
}

function Assert-PrivateDescriptor([string]$Sddl, [string]$OwnerSid) {
    $descriptor = [Security.AccessControl.RawSecurityDescriptor]::new($Sddl)
    if ($null -eq $descriptor.Owner -or $descriptor.Owner.Value -cne $OwnerSid) {
        throw 'task-security-owner-mismatch'
    }
    if (($descriptor.ControlFlags -band [Security.AccessControl.ControlFlags]::DiscretionaryAclProtected) -eq 0) {
        throw 'task-security-dacl-not-protected'
    }
    if ($null -eq $descriptor.DiscretionaryAcl) { throw 'task-security-null-dacl' }
    $seen = @{}
    foreach ($ace in $descriptor.DiscretionaryAcl) {
        if ($ace -isnot [Security.AccessControl.CommonAce] -or
            $ace.AceQualifier -ne [Security.AccessControl.AceQualifier]::AccessAllowed -or
            $ace.AceFlags -ne [Security.AccessControl.AceFlags]::None -or $ace.IsCallback) {
            throw 'task-security-unexpected-ace'
        }
        $sid = $ace.SecurityIdentifier.Value
        if ($sid -cne $OwnerSid -and $sid -cne 'S-1-5-18' -and $sid -cne 'S-1-5-32-544') {
            throw 'task-security-foreign-access'
        }
        # GenericAll may be mapped to FileAllAccess by Task Scheduler.
        $mask = [int64]$ace.AccessMask -band 4294967295L
        if ($mask -ne 268435456L -and $mask -ne 2032127L) {
            throw 'task-security-unexpected-access-mask'
        }
        if ($seen.ContainsKey($sid)) { throw 'task-security-duplicate-ace' }
        $seen[$sid] = $true
    }
    if ($seen.Count -ne 3 -or !$seen.ContainsKey($OwnerSid) -or
        !$seen.ContainsKey('S-1-5-18') -or !$seen.ContainsKey('S-1-5-32-544')) {
        throw 'task-security-incomplete-ace-set'
    }
}

function Read-Snapshot($Folder, [string]$Name, [string]$OwnerSid) {
    $folderSddl = [string]$Folder.GetSecurityDescriptor(7)
    Assert-PrivateDescriptor $folderSddl $OwnerSid
    try { $task = $Folder.GetTask($Name) }
    catch {
        if (Is-Missing $_.Exception) {
            return @{ present = $false; folder_sddl = $folderSddl; task_sddl = $null; xml = $null; state = $null; last_run = $null; last_result = $null; instances = @() }
        }
        throw
    }
    $taskSddl = [string]$task.GetSecurityDescriptor(7)
    Assert-PrivateDescriptor $taskSddl $OwnerSid
    $instances = @()
    foreach ($instance in $task.GetInstances(0)) {
        # InstanceGuid identifies a scheduler instance; it is not a process PID
        # and conveys no native process or descendant cleanup authority.
        $instances += [string]$instance.InstanceGuid
    }
    return @{ present = $true; folder_sddl = $folderSddl; task_sddl = $taskSddl; xml = [string]$task.Xml; state = [int]$task.State; last_run = $task.LastRunTime.ToUniversalTime().ToString("o"); last_result = [int64]$task.LastTaskResult; instances = $instances }
}

try {
    $body = [Console]::In.ReadToEnd()
    if ($body.Length -gt 131072) { throw 'task-request-too-large' }
    $request = ConvertFrom-Json -InputObject $body
    if ($request.schema -cne 'solstone-windows-task-operation-v1') { throw 'task-request-schema' }
    $ownerSid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ([string]$request.owner_sid -cne $ownerSid) { throw 'task-request-owner-mismatch' }
    if ([string]$request.installation_id -cnotmatch '^[0-9a-f]{32}$') { throw 'task-request-installation-id' }
    $folderPath = '\solstone-' + $ownerSid
    $name = [string]$request.installation_id
    $operation = [string]$request.operation
    if ($operation -cnotin @('inspect', 'create', 'update', 'run', 'delete')) { throw 'task-request-operation' }
    $service = New-Object -ComObject 'Schedule.Service'
    $service.Connect()
    try { $folder = $service.GetFolder($folderPath) }
    catch {
        if (!(Is-Missing $_.Exception)) { throw }
        if ($operation -ceq 'inspect') {
            @{ schema = 'solstone-windows-task-operation-v1'; present = $false; folder_sddl = $null; task_sddl = $null; xml = $null; state = $null; last_run = $null; last_result = $null; instances = @() } | ConvertTo-Json -Compress -Depth 6
            exit 0
        }
        if ($operation -cne 'create') { throw 'task-folder-missing-before-mutation' }
        $sddl = 'O:' + $ownerSid + 'G:' + $ownerSid + 'D:P(A;;FA;;;' + $ownerSid + ')(A;;FA;;;SY)(A;;FA;;;BA)'
        # CreateFolder refuses an existing folder; never rewrite an existing ACL.
        $folder = $service.GetFolder('\').CreateFolder($folderPath, $sddl)
    }
    $before = Read-Snapshot $folder $name $ownerSid
    if ($operation -ceq 'create') {
        if ($before.present) { throw 'task-already-exists' }
        $sddl = 'O:' + $ownerSid + 'G:' + $ownerSid + 'D:P(A;;FA;;;' + $ownerSid + ')(A;;FA;;;SY)(A;;FA;;;BA)'
        # TASK_CREATE | TASK_DONT_ADD_PRINCIPAL_ACE; never CREATE_OR_UPDATE.
        $null = $folder.RegisterTask($name, [string]$request.xml, 18, $ownerSid, $null, 3, $sddl)
    } elseif ($operation -cne 'inspect') {
        if (!$before.present -or [string]$request.expected_xml -cne $before.xml -or
            [string]$request.expected_task_sddl -cne $before.task_sddl -or
            [string]$request.expected_folder_sddl -cne $before.folder_sddl) {
            throw 'task-changed-before-mutation'
        }
        $task = $folder.GetTask($name)
        if ($operation -ceq 'run') {
            $null = $task.Run($null)
        } elseif ($operation -ceq 'update') {
            if ($before.instances.Count -ne 0 -or $before.state -ne 3) { throw 'task-not-idle-before-update' }
            # TASK_UPDATE | TASK_DONT_ADD_PRINCIPAL_ACE, after caller profile
            # verification and this exact XML + ACL recheck. Preserve the ACL.
            $null = $folder.RegisterTask($name, [string]$request.xml, 20, $ownerSid, $null, 3, $before.task_sddl)
        } elseif ($operation -ceq 'delete') {
            if ($before.instances.Count -ne 0 -or $before.state -ne 3) { throw 'task-not-idle-before-delete' }
            $folder.DeleteTask($name, 0)
        }
    }
    $after = Read-Snapshot $folder $name $ownerSid
    if ($operation -ceq 'delete' -and $after.present) { throw 'task-delete-not-observed' }
    if (($operation -ceq 'create' -or $operation -ceq 'update' -or $operation -ceq 'run') -and !$after.present) { throw 'task-mutation-not-observed' }
    $after.schema = 'solstone-windows-task-operation-v1'
    $after | ConvertTo-Json -Compress -Depth 6
    exit 0
} catch {
    [Console]::Error.WriteLine('Windows task operation failed: ' + $_.Exception.Message)
    exit 1
}
