// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::windows_action::{
    WindowsServiceAction, encode_windows_task_arguments, xml_text_is_valid,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowsTaskInput<'a> {
    pub principal_sid: &'a str,
    pub command: &'a str,
    pub action: &'a WindowsServiceAction,
    pub working_directory: &'a str,
    /// Whether the owner wants the journal running. `journal service stop`
    /// records the answer here, in the registration itself, so every trigger
    /// honours it and nothing has to guess from a marker.
    pub enabled: bool,
}

/// Which generation of the managed task profile a registration carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsTaskProfile {
    /// The profile this build registers: the logon trigger plus the
    /// five-minute trigger that brings back a resident whose own process died.
    Current,
    /// The logon-only profile every build before the five-minute trigger
    /// registered. Still valid to read, start and stop; `journal service
    /// install` (and the post-update step) replaces it with [`Self::Current`].
    Legacy,
}

/// How often the scheduler looks for a resident that should be running and is
/// not.
///
/// 🔴 This trigger exists because nothing else restarts the task's own
/// process. The forwarder restarts a supervisor that dies, but when the
/// forwarder itself dies (killed, crashed, or taken down by an update) the
/// task just ends: `RestartOnFailure` covers only a failure to *start*,
/// measured on Windows 11. With `IgnoreNew`, the trigger does nothing while a
/// resident runs, and a disabled task (an owner's `service stop`) is never
/// triggered at all, so it never restarts a journal the owner stopped.
pub const WINDOWS_TASK_RECOVERY_INTERVAL: &str = "PT5M";
/// Any fixed past instant: the repetition runs from it indefinitely.
const RECOVERY_START_BOUNDARY: &str = "2026-01-01T00:00:00";

pub fn render_windows_task_xml(input: &WindowsTaskInput<'_>) -> Result<String, &'static str> {
    render_windows_task_profile_xml(input, WindowsTaskProfile::Current)
}

pub(crate) fn render_windows_task_profile_xml(
    input: &WindowsTaskInput<'_>,
    profile: WindowsTaskProfile,
) -> Result<String, &'static str> {
    if [input.principal_sid, input.command, input.working_directory]
        .iter()
        .any(|value| value.is_empty() || !xml_text_is_valid(value))
    {
        return Err("invalid Windows task field");
    }
    if input.working_directory != input.action.journal {
        return Err("Windows task working directory differs from selected journal");
    }
    let arguments = encode_windows_task_arguments(&input.action.arguments()?)?;
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.3" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Solstone Journal Supervisor</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <UserId>{}</UserId>
    </LogonTrigger>{}
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>{}</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>10</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec id="journal-supervisor">
      <Command>{}</Command>
      <Arguments>{}</Arguments>
      <WorkingDirectory>{}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>"#,
        xml_escape(input.principal_sid),
        match profile {
            WindowsTaskProfile::Current => format!(
                r#"
    <TimeTrigger>
      <Repetition>
        <Interval>{WINDOWS_TASK_RECOVERY_INTERVAL}</Interval>
        <StopAtDurationEnd>false</StopAtDurationEnd>
      </Repetition>
      <StartBoundary>{RECOVERY_START_BOUNDARY}</StartBoundary>
      <Enabled>true</Enabled>
    </TimeTrigger>"#
            ),
            WindowsTaskProfile::Legacy => String::new(),
        },
        xml_escape(input.principal_sid),
        input.enabled,
        xml_escape(input.command),
        xml_escape(&arguments),
        xml_escape(input.working_directory)
    ))
}

fn xml_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for char in value.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(char),
        }
    }
    escaped
}
