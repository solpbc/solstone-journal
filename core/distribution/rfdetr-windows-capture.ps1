# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Dot-sourced only by the RF controlled-build driver and its native controls.
# Build tools are Unowned. This does not create a Job, kill by PID, or claim
# descendant quiescence. The caller retains the fence on an incomplete capture.
# Process/EOF waits share one clock. Synchronous file open, Process.Start,
# Flush(true) and Dispose cannot be interrupted by these .NET wait deadlines.
# elapsed_ms measures through their return; native controls test actual return
# latency. A hard whole-function deadline is not asserted by this helper.

function ConvertTo-RfdetrArgument([string]$Value) {
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

function Invoke-RfdetrCapture {
    param(
        [Parameter(Mandatory = $true)][string]$Command,
        [string[]]$Arguments = @(),
        [Parameter(Mandatory = $true)][string]$Cwd,
        [Parameter(Mandatory = $true)][string]$StdoutPath,
        [Parameter(Mandatory = $true)][string]$StderrPath,
        [Parameter(Mandatory = $true)][ValidateRange(1, 7200000)][int]$TimeoutMilliseconds,
        [hashtable]$Environment = @{},
        # cmd.exe has its own parser. Only the driver's fixed batch invocation
        # supplies this after rejecting cmd metacharacters from the batch path.
        [string]$CmdArgumentLine
    )
    $result = [pscustomobject]@{
        launch_attempted = $false; started = $false; pid = $null; exit_code = $null
        completed = $false; elapsed_ms = 0; error = $null
        process = $null; retained_handle = $null
        stdout_file = $null; stderr_file = $null
        stdout_task = $null; stderr_task = $null
    }
    $clock = [Diagnostics.Stopwatch]::StartNew()
    try {
        $result.stdout_file = [IO.FileStream]::new($StdoutPath, [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $result.stderr_file = [IO.FileStream]::new($StderrPath, [IO.FileMode]::CreateNew,
            [IO.FileAccess]::Write, [IO.FileShare]::Read)
        $start = [Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $Command
        $start.WorkingDirectory = $Cwd
        $start.UseShellExecute = $false
        $start.RedirectStandardInput = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true
        $start.CreateNoWindow = $true
        if ($CmdArgumentLine) {
            if ([IO.Path]::GetFileName($Command) -ine 'cmd.exe') { throw 'raw arguments require cmd.exe' }
            $start.Arguments = $CmdArgumentLine
        } else {
            $encoded = foreach ($argument in $Arguments) { ConvertTo-RfdetrArgument $argument }
            $start.Arguments = $encoded -join ' '
        }
        foreach ($key in $Environment.Keys) { $start.EnvironmentVariables[$key] = $Environment[$key] }
        $result.process = [Diagnostics.Process]::new()
        $result.process.StartInfo = $start
        $result.launch_attempted = $true
        if (-not $result.process.Start()) { throw 'process start returned false' }
        $result.started = $true
        $result.retained_handle = $result.process.Handle
        $result.pid = $result.process.Id
        $result.process.StandardInput.Close()
        # Use BaseStream, never a text reader or DataReceived line callbacks.
        # On incomplete setup/timeout these references stay with the result.
        $result.stdout_task = $result.process.StandardOutput.BaseStream.CopyToAsync($result.stdout_file)
        $result.stderr_task = $result.process.StandardError.BaseStream.CopyToAsync($result.stderr_file)
        $remaining = [Math]::Max(0, $TimeoutMilliseconds - [int]$clock.ElapsedMilliseconds)
        if (-not $result.process.WaitForExit($remaining)) { throw 'native child deadline exceeded' }
        $result.process.Refresh()
        $result.exit_code = $result.process.ExitCode
        $remaining = [Math]::Max(0, $TimeoutMilliseconds - [int]$clock.ElapsedMilliseconds)
        $drained = [Threading.Tasks.Task]::WhenAll([Threading.Tasks.Task[]]@($result.stdout_task, $result.stderr_task))
        if (-not $drained.Wait($remaining)) { throw 'native output EOF deadline exceeded' }
        $result.stdout_file.Flush($true)
        $result.stderr_file.Flush($true)
        $result.stdout_file.Dispose()
        $result.stderr_file.Dispose()
        $result.process.Dispose()
        $result.completed = $true
    } catch {
        $result.error = $_.Exception.ToString()
        # No termination or disposal after an uncertain launch. These managed
        # references live only as long as the driver process. Its durable fence,
        # rules and evidence carry reconciliation after that process exits;
        # the JSON never represents a transferable or recoverable native owner.
        if (-not $result.launch_attempted) {
            if ($null -ne $result.stdout_file) { $result.stdout_file.Dispose() }
            if ($null -ne $result.stderr_file) { $result.stderr_file.Dispose() }
            if ($null -ne $result.process) { $result.process.Dispose() }
        }
    }
    $result.elapsed_ms = $clock.ElapsedMilliseconds
    return $result
}
