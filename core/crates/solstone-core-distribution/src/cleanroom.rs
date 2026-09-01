// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Cleanroom plan, fail-closed aggregation, and loopback origin stand-in.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use crate::archive::refuse_escape;
use crate::inventory::{CleanroomSubject, Inventory, digest_is_pinned, format_named_list};

pub const SUBJECT_NETWORK: &str = "none";
pub const FORBIDDEN_SUBJECT_TOOLS: &[&str] = &["python", "python3", "pip", "pipx", "maturin"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPlan {
    pub id: String,
    pub image: String,
    pub digest: String,
    pub network: String,
    pub python: bool,
    pub control: bool,
    pub roles: Vec<String>,
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
    // The Docker cleanroom is a Linux instrument. A macOS target has no image
    // to run in, and including it here would emit an install command for an
    // archive no subject can read.
    let mut commands = inventory
        .target
        .iter()
        .filter(|target| target.os == crate::inventory::OS_LINUX)
        .map(|target| {
            let base = inventory
                .artifact
                .render(version, &target.os, &target.arch);
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
        if subject.python != subject.control {
            unexpected.insert(format!("{} invalid-control", subject.id));
        }
        subjects.push(SubjectPlan {
            id: subject.id.clone(),
            image: subject.image.clone(),
            digest: subject.digest.clone(),
            network: SUBJECT_NETWORK.to_owned(),
            python: subject.python,
            control: subject.control,
            roles: subject.roles.clone(),
            mounts: subject.mounts.clone(),
            tools: subject.required_tools.clone(),
            commands: if subject.entry_command.is_empty() {
                cleanroom_install_commands(inventory)
            } else {
                vec![subject.entry_command.clone()]
            },
            artifacts: subject.expected.clone(),
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

pub fn serve_generation_fixture(
    listener: TcpListener,
    evidence: &Path,
    expected_fragment: &str,
) -> io::Result<()> {
    for incoming in listener.incoming() {
        let stream = incoming?;
        handle_generation(stream, evidence, expected_fragment)?;
    }
    Ok(())
}

fn handle_generation(
    mut stream: TcpStream,
    evidence: &Path,
    expected_fragment: &str,
) -> io::Result<()> {
    let request = read_http_request(&mut stream)?;
    let request_line = request.lines().next().unwrap_or_default();
    let body = request.split_once("\r\n\r\n").map_or("", |(_, body)| body);
    if request_line.starts_with("GET /health ") {
        return write_json(&mut stream, 200, r#"{"loaded_model":"cleanroom"}"#);
    }
    if request_line.starts_with("GET /props ") {
        return write_json(&mut stream, 200, r#"{"n_ctx":16384,"total_slots":16}"#);
    }
    if request_line.starts_with("POST /tokenize ") {
        return write_json(&mut stream, 200, r#"{"tokens":[1]}"#);
    }
    if !request_line.starts_with("POST /v1/chat/completions ") {
        return write_json(&mut stream, 404, r#"{"error":"unexpected-endpoint"}"#);
    }
    let prompt = serde_json::from_str::<serde_json::Value>(body)
        .map(|value| flatten_json_strings(&value))
        .unwrap_or_else(|_| body.to_owned());
    if prompt.contains("Reply with the single word OK.") {
        return write_json(
            &mut stream,
            200,
            &serde_json::json!({
                "choices": [{"message": {"content": "OK"}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string(),
        );
    }
    if prompt.contains("bounded solstone diagnostic cogitate check")
        && prompt.contains("emit_final")
    {
        return write_json(
            &mut stream,
            200,
            &serde_json::json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": "final-1",
                            "type": "function",
                            "function": {"name": "emit_final", "arguments": r#"{"content":"OK"}"#}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
            })
            .to_string(),
        );
    }
    let (talent, completion) = match generation_completion(&prompt, expected_fragment) {
        Ok(reply) => reply,
        Err(reason) => {
            append_evidence(evidence, reason)?;
            let prompt_excerpt = prompt.chars().take(16_384).collect::<String>();
            append_evidence(evidence, &format!("prompt={prompt_excerpt:?}"))?;
            return write_json(
                &mut stream,
                422,
                &serde_json::json!({"error": reason}).to_string(),
            );
        }
    };
    append_evidence(evidence, talent)?;
    write_json(
        &mut stream,
        200,
        &serde_json::json!({
            "choices": [{"message": {"content": completion}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string(),
    )
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<String> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > 4 * 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "cleanroom request exceeds 4 MiB",
            ));
        }
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
            .unwrap_or_default();
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8(request)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "cleanroom request is not UTF-8"))
}

fn flatten_json_strings(value: &serde_json::Value) -> String {
    fn visit(value: &serde_json::Value, output: &mut String) {
        match value {
            serde_json::Value::String(text) => {
                output.push_str(text);
                output.push('\n');
            }
            serde_json::Value::Array(values) => {
                for value in values {
                    visit(value, output);
                }
            }
            serde_json::Value::Object(values) => {
                for (key, value) in values {
                    output.push_str(key);
                    output.push('\n');
                    visit(value, output);
                }
            }
            _ => {}
        }
    }
    let mut output = String::new();
    visit(value, &mut output);
    output
}

fn generation_completion(
    prompt: &str,
    expected_fragment: &str,
) -> Result<(&'static str, &'static str), &'static str> {
    if prompt.contains("Maintenance Window Analysis")
        && prompt.contains("primary")
        && prompt.contains("fallback")
    {
        if !prompt.contains(expected_fragment) {
            return Err("daily_schedule-anchor-missing");
        }
        return Ok((
            "daily_schedule",
            r#"{"primary":"03:00","fallback":"04:00"}"#,
        ));
    }
    if prompt.contains("You are generating the morning briefing")
        && prompt.contains("coverage_preamble")
        && prompt.contains("needs_attention")
    {
        return Ok((
            "morning_briefing",
            r#"{"metadata":{"generated":"2099-01-01T00:00:00Z","model":"cleanroom","sources":{"segments":0,"anticipated_activities":0,"facet_newsletters":0,"followups":0,"steward_health":"missing"},"gaps":[],"coverage_preamble":""},"your_day":[],"yesterday":[],"needs_attention":[],"forward_look":[],"reading":[]}"#,
        ));
    }
    if prompt.contains("Future Schedule Extraction") && prompt.contains("cancelled") {
        return Ok(("schedule", r#"{"events":[]}"#));
    }
    Err("unexpected-talent")
}

fn append_evidence(path: &Path, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{line}")
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

fn write_json(stream: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        422 => "Unprocessable Entity",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body.as_bytes())
}

pub fn render_plan(plan: &CleanroomPlan) -> String {
    let mut out = String::new();
    for subject in &plan.subjects {
        out.push_str(&format!(
            "SUBJECT {} {} {} {}\n",
            subject.id, subject.image, subject.digest, subject.network
        ));
        out.push_str(&format!("CONTROL {} {}\n", subject.id, subject.control));
        out.push_str(&format!(
            "ROLES {} {}\n",
            subject.id,
            subject.roles.join(",")
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
        let mut controls = 0;
        let mut roles = BTreeSet::new();
        for subject in &plan.subjects {
            assert!(digest_is_pinned(&subject.digest), "{}", subject.id);
            assert_eq!(subject.network, SUBJECT_NETWORK);
            roles.extend(subject.roles.iter().cloned());
            if subject.control {
                controls += 1;
                assert!(subject.python);
                assert!(subject.tools.iter().any(|tool| tool == "python3"));
                continue;
            }
            assert!(!subject.python);
            assert!(subject.tools.iter().all(|tool| {
                !FORBIDDEN_SUBJECT_TOOLS
                    .iter()
                    .any(|forbidden| tool == forbidden)
            }));
            assert!(
                subject
                    .mounts
                    .iter()
                    .any(|mount| mount.contains("artifacts"))
            );
            assert!(
                subject
                    .commands
                    .iter()
                    .any(|command| command.contains("cleanroom.sh --inside"))
            );
            assert!(!subject.artifacts.is_empty());
        }
        assert_eq!(controls, 1);
        assert_eq!(
            roles,
            BTreeSet::from_iter([
                "bootstrap".to_owned(),
                "deb".to_owned(),
                "pdf".to_owned(),
                "python-control".to_owned(),
                "rpm".to_owned(),
                "speakers".to_owned(),
                "talent".to_owned(),
                "tar".to_owned(),
            ])
        );
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
    fn generation_fixture_requires_the_daily_activity_anchor_and_maps_the_full_batch() {
        let anchor = "20260817 (Monday):\n  03:17 - 03:28 (11m)";
        assert_eq!(
            generation_completion(
                &format!("Maintenance Window Analysis primary fallback\n{anchor}"),
                anchor
            ),
            Ok((
                "daily_schedule",
                r#"{"primary":"03:00","fallback":"04:00"}"#
            ))
        );
        assert_eq!(
            generation_completion("Maintenance Window Analysis primary fallback", anchor),
            Err("daily_schedule-anchor-missing")
        );
        assert_eq!(
            generation_completion("Future Schedule Extraction cancelled", anchor)
                .expect("schedule response")
                .0,
            "schedule"
        );
        assert_eq!(
            generation_completion(
                "You are generating the morning briefing coverage_preamble needs_attention",
                anchor
            )
            .expect("morning response")
            .0,
            "morning_briefing"
        );
        assert_eq!(
            generation_completion("unknown", anchor),
            Err("unexpected-talent")
        );
    }
}
