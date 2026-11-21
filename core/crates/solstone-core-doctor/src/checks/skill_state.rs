// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::{
    context::CheckContext,
    vocabulary::{Check, RunnerResult, Status, make_result},
};
use solstone_core_cli_copy as copy;
pub fn run(context: &CheckContext, check: Check) -> RunnerResult {
    // The router-skill sources are payload, so they are found under the payload
    // root rather than the repository root.
    let Some(root) = context.payload_root.as_deref() else {
        return Ok(make_result(
            check,
            Status::Skip,
            copy::text("journal.cli.doctor.skill.state.checkout.required"),
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
            copy::text("journal.cli.doctor.skill.state.directories.missing"),
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
                            problems.push(copy::display(
                                "journal.cli.doctor.skill.state.link.missing",
                                [("skill", &link.name), ("path", &link.link.display())],
                            ));
                        }
                        solstone_core_skill_state::RouterSkillLinkState::Foreign => {
                            problems.push(copy::display(
                                "journal.cli.doctor.skill.state.link.foreign",
                                [("skill", &link.name), ("path", &link.link.display())],
                            ));
                        }
                    }
                }
                match solstone_core_skill_state::stale_router_skill_links(parent) {
                    Ok(stale_links) => {
                        for stale in stale_links {
                            problems.push(copy::display(
                                "journal.cli.doctor.skill.state.link.stale",
                                [("path", &stale.link.display())],
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
            copy::text("journal.cli.doctor.skill.state.links.ready"),
            None::<String>,
        ))
    } else {
        Ok(make_result(
            check,
            Status::Warn,
            problems.join("; "),
            Some(copy::text("journal.cli.doctor.skill.state.install")),
        ))
    }
}
