// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cleanroom plan, fail-closed aggregation, and loopback origin stand-in.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use crate::archive::refuse_escape;
use crate::inventory::{CleanroomSubject, Inventory, digest_is_pinned, format_named_list};

pub const LOOPBACK_HOST: &str = "127.0.0.1";
pub const SUBJECT_NETWORK: &str = "none";
pub const FORBIDDEN_SUBJECT_TOOLS: &[&str] = &["python", "python3", "pip", "pipx", "maturin"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPlan {
    pub id: String,
    pub image: String,
    pub digest: String,
    pub network: String,
    pub python: bool,
    pub mounts: Vec<String>,
    pub tools: Vec<String>,
    pub commands: Vec<String>,
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuilderPlan {
    pub id: String,
    pub from_subject: String,
    pub rustc: String,
    pub zig: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CleanroomPlan {
    pub subjects: Vec<SubjectPlan>,
    pub builders: Vec<BuilderPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepResult {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aggregation {
    pub ok: bool,
    pub failed: Vec<String>,
}

fn cleanroom_install_commands(inventory: &Inventory) -> Vec<String> {
    let version = env!("CARGO_PKG_VERSION");
    let mut commands = inventory
        .target
        .iter()
        .map(|target| {
            let base = inventory.artifact.render(version, &target.arch);
            format!(
                "sh /opt/solstone/install.sh --archive /opt/solstone/{base}.tar.gz --sha256 /opt/solstone/{base}.sha256 --release /opt/solstone/{base}.release"
            )
        })
        .collect::<Vec<_>>();
    commands.push("test -L /opt/prefix/current".to_owned());
    commands
}

pub fn refuse_unpinned(subject: &CleanroomSubject) -> Result<(), String> {
    if digest_is_pinned(&subject.digest) {
        return Ok(());
    }
    let mut names = BTreeSet::new();
    names.insert(subject.id.clone());
    Err(format_named_list("unpinned cleanroom subject", &names))
}

pub fn plan_from_inventory(inventory: &Inventory) -> Result<CleanroomPlan, String> {
    let mut unexpected = BTreeSet::new();
    let mut subjects = Vec::new();
    for subject in &inventory.cleanroom.subject {
        refuse_unpinned(subject)?;
        if subject.network != SUBJECT_NETWORK {
            unexpected.insert(format!("{} network {}", subject.id, subject.network));
        }
        if subject.python {
            unexpected.insert(format!("{} python", subject.id));
        }
        subjects.push(SubjectPlan {
            id: subject.id.clone(),
            image: subject.image.clone(),
            digest: subject.digest.clone(),
            network: SUBJECT_NETWORK.to_owned(),
            python: false,
            mounts: vec![
                "versions:ro".to_owned(),
                "install.sh:ro".to_owned(),
                "archive:ro".to_owned(),
            ],
            tools: vec!["sh".to_owned(), "tar".to_owned(), "sha256sum".to_owned()],
            commands: cleanroom_install_commands(inventory),
            artifacts: vec![
                "current".to_owned(),
                "current/bin".to_owned(),
                ".profile".to_owned(),
            ],
        });
    }
    let subject_ids = subjects
        .iter()
        .map(|subject| subject.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut builders = Vec::new();
    for builder in &inventory.cleanroom.builder {
        if !subject_ids.contains(builder.from_subject.as_str()) {
            unexpected.insert(format!("{} from {}", builder.id, builder.from_subject));
        }
        builders.push(BuilderPlan {
            id: builder.id.clone(),
            from_subject: builder.from_subject.clone(),
            rustc: builder.rustc.clone(),
            zig: builder.zig.clone(),
        });
    }
    if !unexpected.is_empty() {
        return Err(format_named_list("unexpected cleanroom plan", &unexpected));
    }
    Ok(CleanroomPlan { subjects, builders })
}

pub fn aggregate(results: &[StepResult]) -> Aggregation {
    let failed = results
        .iter()
        .filter(|result| !result.ok)
        .map(|result| result.name.clone())
        .collect::<Vec<_>>();
    Aggregation {
        ok: failed.is_empty(),
        failed,
    }
}

pub fn bind_loopback() -> io::Result<(TcpListener, u16)> {
    let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
    let port = listener.local_addr()?.port();
    Ok((listener, port))
}

pub fn serve_directory(listener: TcpListener, root: &Path) -> io::Result<()> {
    for incoming in listener.incoming() {
        let stream = incoming?;
        handle_get(stream, root)?;
    }
    Ok(())
}

fn handle_get(mut stream: TcpStream, root: &Path) -> io::Result<()> {
    let mut buf = [0_u8; 4096];
    let read = stream.read(&mut buf)?;
    let request = std::str::from_utf8(&buf[..read]).unwrap_or("");
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");
    let relative = path.trim_start_matches('/');
    if refuse_escape(relative).is_err() {
        write_http(&mut stream, 400, b"origin-refused")?;
        return Ok(());
    }
    let file = root.join(relative);
    match fs::read(&file) {
        Ok(bytes) => write_http(&mut stream, 200, &bytes),
        Err(_) => write_http(&mut stream, 404, b"missing"),
    }
}

fn write_http(stream: &mut TcpStream, status: u16, body: &[u8]) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        _ => "Not Found",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    Ok(())
}

pub fn render_plan(plan: &CleanroomPlan) -> String {
    let mut out = String::new();
    for subject in &plan.subjects {
        out.push_str(&format!(
            "SUBJECT {} {} {} {}\n",
            subject.id, subject.image, subject.digest, subject.network
        ));
        out.push_str(&format!("MOUNTS {}\n", subject.mounts.join(",")));
        out.push_str(&format!("TOOLS {}\n", subject.tools.join(",")));
        out.push_str(&format!("COMMANDS {}\n", subject.commands.join(" ;; ")));
        out.push_str(&format!("ARTIFACTS {}\n", subject.artifacts.join(",")));
    }
    for builder in &plan.builders {
        out.push_str(&format!(
            "BUILDER {} {} rustc={} zig={}\n",
            builder.id, builder.from_subject, builder.rustc, builder.zig
        ));
    }
    out
}

pub fn plan_text_from_inventory_path(path: &Path) -> Result<String, String> {
    let inventory =
        crate::validate_distribution_inventory(path).map_err(|error| error.to_string())?;
    let plan = plan_from_inventory(&inventory)?;
    Ok(render_plan(&plan))
}

pub fn serve_root_from_args(args: &[String]) -> Result<PathBuf, String> {
    let root = args
        .first()
        .map(PathBuf::from)
        .ok_or_else(|| "usage: solstone-distribution cleanroom-serve DIR".to_owned())?;
    if !root.is_dir() {
        return Err(format!("cleanroom-serve root missing: {}", root.display()));
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::load_inventory;
    use std::path::Path;

    fn committed_inventory() -> crate::inventory::Inventory {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/distribution/inventory.toml");
        load_inventory(&path).expect("committed inventory")
    }

    #[test]
    fn committed_subjects_are_pinned_python_absent_and_network_none() {
        let inventory = committed_inventory();
        let plan = plan_from_inventory(&inventory).expect("plan");
        assert!(!plan.subjects.is_empty());
        for subject in &plan.subjects {
            assert!(digest_is_pinned(&subject.digest), "{}", subject.id);
            assert_eq!(subject.network, SUBJECT_NETWORK);
            assert!(!subject.python);
            assert!(subject.tools.iter().all(|tool| {
                !FORBIDDEN_SUBJECT_TOOLS
                    .iter()
                    .any(|forbidden| tool == forbidden)
            }));
            assert!(subject.mounts.iter().any(|mount| mount.contains("archive")));
            assert!(
                subject
                    .commands
                    .iter()
                    .any(|command| command.contains("install.sh"))
            );
            let version = env!("CARGO_PKG_VERSION");
            for target in &inventory.target {
                let base = inventory.artifact.render(version, &target.arch);
                assert!(
                    subject
                        .commands
                        .iter()
                        .any(|command| command.contains(&format!("{base}.tar.gz"))
                            && command.contains(&format!("{base}.sha256"))
                            && command.contains(&format!("{base}.release"))),
                    "{} missing {base}",
                    subject.id
                );
            }
            assert!(subject.artifacts.iter().any(|item| item == "current"));
        }
        assert!(plan.builders.iter().all(|builder| {
            plan.subjects
                .iter()
                .any(|subject| subject.id == builder.from_subject)
        }));
    }

    #[test]
    fn unpinned_subject_is_refused() {
        let inventory = committed_inventory();
        let mut subject = inventory.cleanroom.subject[0].clone();
        subject.digest = "sha256:REFUSE-UNPINNED".to_owned();
        let error = refuse_unpinned(&subject).expect_err("unpinned");
        assert!(error.contains("unpinned cleanroom subject"));
        assert!(error.contains(&subject.id));
    }

    #[test]
    fn aggregation_fails_closed_on_any_failed_step() {
        let results = [
            StepResult {
                name: "fetch".to_owned(),
                ok: true,
                detail: String::new(),
            },
            StepResult {
                name: "subject".to_owned(),
                ok: false,
                detail: "network".to_owned(),
            },
        ];
        let summary = aggregate(&results);
        assert!(!summary.ok);
        assert_eq!(summary.failed, ["subject"]);
        assert!(
            aggregate(&[StepResult {
                name: "fetch".to_owned(),
                ok: true,
                detail: String::new(),
            }])
            .ok
        );
    }

    #[test]
    fn loopback_bind_is_localhost_ephemeral() {
        let (listener, port) = bind_loopback().expect("bind");
        let addr = listener.local_addr().expect("addr");
        assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
        assert_ne!(port, 0);
    }
}
