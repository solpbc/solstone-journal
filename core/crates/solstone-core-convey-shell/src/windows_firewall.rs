// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only detection of the app-wide inbound Block rules Windows retains
//! after Cancel. This is not a general firewall reachability verdict.

#[cfg(any(windows, test))]
use serde::Deserialize;

#[cfg(any(windows, test))]
#[derive(Deserialize)]
struct Policy {
    enabled_profiles: u32,
    rules: Vec<Rule>,
}

#[cfg(any(windows, test))]
#[derive(Clone, Deserialize)]
struct Rule {
    enabled: bool,
    direction: u32,
    action: u32,
    profiles: u32,
    protocol: u32,
    local_ports: String,
    local_addresses: String,
    remote_addresses: String,
    interface_types: String,
    service_name: String,
}

#[cfg(any(windows, test))]
impl Policy {
    fn has_app_block(&self) -> bool {
        self.rules.iter().any(|rule| {
            rule.enabled
                && rule.direction == 1
                && rule.action == 0
                && rule.profiles & self.enabled_profiles != 0
                && matches!(rule.protocol, 6 | 256)
                && matches!(rule.local_ports.as_str(), "" | "*")
                && rule.local_addresses == "*"
                && rule.remote_addresses == "*"
                && rule.interface_types.eq_ignore_ascii_case("all")
                && rule.service_name.is_empty()
        })
    }
}

/// `None` means not checked (including non-Windows and a refused/timed-out
/// policy read), not proof that Windows will let a device through.
pub(crate) async fn app_blocked() -> Option<bool> {
    #[cfg(windows)]
    {
        tokio::task::spawn_blocking(read_policy)
            .await
            .ok()
            .flatten()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

#[cfg(windows)]
fn read_policy() -> Option<bool> {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::Duration;

    use solstone_core_system::process::{
        BoundedHelperBudget, BoundedHelperRequest, BoundedHelperResources, run_bounded_helper,
    };

    let system_root = std::env::var_os("SystemRoot")?;
    let root = PathBuf::from(&system_root).join("System32");
    let executable = std::env::current_exe().ok()?;
    let executable = executable
        .to_str()?
        .strip_prefix(r"\\?\")
        .unwrap_or(executable.to_str()?);
    let output = run_bounded_helper(BoundedHelperRequest {
        executable: root.join(r"WindowsPowerShell\v1.0\powershell.exe"),
        current_directory: root.clone(),
        package_root: root,
        arguments: [
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("windows_firewall.ps1"),
        ]
        .into_iter()
        .map(str::to_owned)
        .collect(),
        environment: BTreeMap::from([(OsString::from("SystemRoot"), system_root)]),
        stdin: serde_json::to_vec(&serde_json::json!({"executable": executable})).ok()?,
        budget: BoundedHelperBudget {
            timeout: Duration::from_secs(5),
            stdin_limit_bytes: 64 * 1024,
            stdout_limit_bytes: 128 * 1024,
            stderr_limit_bytes: 4 * 1024,
        },
        resource_limits: None,
        resources: BoundedHelperResources::new(),
    })
    .ok()?;
    if output.exit_code != 0 || !output.quiescent {
        return None;
    }
    serde_json::from_slice::<Policy>(&output.stdout)
        .ok()
        .map(|policy| policy.has_app_block())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(windows, feature = "full-tests"))]
    #[test]
    fn native_policy_read_completes_through_the_bounded_helper() {
        assert!(
            read_policy().is_some(),
            "Windows policy read must return a checked result"
        );
    }

    fn cancel_rule() -> Rule {
        Rule {
            enabled: true,
            direction: 1,
            action: 0,
            profiles: 6,
            protocol: 6,
            local_ports: "*".into(),
            local_addresses: "*".into(),
            remote_addresses: "*".into(),
            interface_types: "All".into(),
            service_name: "".into(),
        }
    }

    fn blocked(rule: Rule, enabled_profiles: u32) -> bool {
        Policy {
            enabled_profiles,
            rules: vec![rule],
        }
        .has_app_block()
    }

    #[test]
    fn cancel_blocks_tcp_on_an_enabled_current_profile() {
        assert!(blocked(cancel_rule(), 2));
        assert!(blocked(cancel_rule(), 4));
        assert!(!blocked(cancel_rule(), 1));
        assert!(!blocked(cancel_rule(), 0));
    }

    #[test]
    fn inactive_allow_outbound_and_udp_rules_are_not_the_cancel_block() {
        let mut rule = cancel_rule();
        rule.enabled = false;
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.action = 1;
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.direction = 2;
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.protocol = 17;
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.protocol = 256;
        assert!(blocked(rule, 6));
    }

    #[test]
    fn scoped_custom_rules_do_not_claim_an_app_wide_block() {
        let mut rule = cancel_rule();
        rule.local_ports = "80".into();
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.local_addresses = "127.0.0.1".into();
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.remote_addresses = "192.0.2.1".into();
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.interface_types = "Wireless".into();
        assert!(!blocked(rule, 6));
        let mut rule = cancel_rule();
        rule.service_name = "other".into();
        assert!(!blocked(rule, 6));
    }
}
