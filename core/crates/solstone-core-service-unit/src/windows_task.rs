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
}

pub fn render_windows_task_xml(input: &WindowsTaskInput<'_>) -> Result<String, &'static str> {
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
    </LogonTrigger>
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
    <Enabled>true</Enabled>
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
        xml_escape(input.principal_sid),
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
