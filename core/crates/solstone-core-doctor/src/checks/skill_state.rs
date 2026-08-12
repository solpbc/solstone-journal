// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    let Some(root) = context.checkout_root.as_deref() else {
        return Ok(make_result(
            check,
            Status::Skip,
            "router skill state is only available from a source checkout",
            None::<String>,
        ));
    };
    let parents = [
        context.journal_path.join(".claude/skills"),
        context.journal_path.join(".agents/skills"),
    ];
    if parents.iter().all(|parent| !parent.exists()) {
        return Ok(make_result(
            check,
            Status::Skip,
            "router skill directories are unavailable",
            None::<String>,
        ));
    }
    let mut problems = Vec::new();
    for parent in &parents {
        match solstone_core_skill_state::inspect_router_skill_links(root, parent) {
            Ok(links) => {
                for link in links {
                    match link.state {
                        solstone_core_skill_state::RouterSkillLinkState::Installed => {}
                        solstone_core_skill_state::RouterSkillLinkState::Missing => {
                            problems.push(format!(
                                "{} missing at {}",
                                link.name,
                                link.link.display()
                            ));
                        }
                        solstone_core_skill_state::RouterSkillLinkState::Foreign => {
                            problems.push(format!(
                                "{} points elsewhere at {}",
                                link.name,
                                link.link.display()
                            ));
                        }
                    }
                }
                match solstone_core_skill_state::stale_router_skill_links(parent) {
                    Ok(stale_links) => {
                        for stale in stale_links {
                            problems.push(format!(
                                "stale router skill link at {}",
                                stale.link.display()
                            ));
                        }
                    }
                    Err(error) => {
                        return Ok(make_result(check, Status::Warn, error, None::<String>));
                    }
                }
            }
            Err(error) => return Ok(make_result(check, Status::Warn, error, None::<String>)),
        }
    }
    if problems.is_empty() {
        Ok(make_result(
            check,
            Status::Ok,
            "router skills sol, journal are installed and current",
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            problems.join("; "),
            Some("run sol skills install --project ."),
        ))
    }
}
