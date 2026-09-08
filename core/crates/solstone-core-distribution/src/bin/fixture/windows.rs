// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Test-only development payload assembly. Never a production inventory producer.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};

use minisign::KeyPair;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use solstone_core_distribution::manifest_verify::install_test_fixture_pin;
use solstone_core_distribution::windows_payload::{
    WINDOWS_ONNXRUNTIME_LIBRARY, WINDOWS_PAYLOAD_MANIFEST, WINDOWS_PAYLOAD_SIGNATURE,
    WINDOWS_PYANNOTE_MODEL, WINDOWS_SILERO_VAD_MODEL, WINDOWS_SPEAKERS_ANALYZE_WORKER,
    WINDOWS_VAD_ANALYZE_WORKER, WINDOWS_WESPEAKER_MODEL, WindowsPayloadManifest,
    render_windows_payload_manifest, verify_windows_payload,
};

const SCHEMA: &str = "solstone.windows-development-fixture.v1";
const MAX_SPEC_BYTES: u64 = 1024 * 1024;
const MAX_MEMBER_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_MEMBERS: usize = 64;
const MAX_MEMBER_PATH_BYTES: usize = 4096;
const MAX_MEMBER_COMPONENTS: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Spec {
    schema: String,
    source_commit: String,
    cargo_lock_sha256: String,
    members: Vec<Member>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Member {
    source: PathBuf,
    path: String,
    bytes: u64,
    sha256: String,
}

fn refusal(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn member_path(value: &str) -> io::Result<()> {
    // Bound work before allocating any growing directory prefixes. These are
    // fixture parser budgets, not guarantees of host filesystem path support.
    if value.len() > MAX_MEMBER_PATH_BYTES || value.split('/').count() > MAX_MEMBER_COMPONENTS {
        return Err(refusal("fixture member path exceeds length or depth limit"));
    }
    if !value.starts_with("bin/") && !value.starts_with("lib/") && !value.starts_with("share/") {
        return Err(refusal("member must be under bin, lib or share"));
    }
    if value.eq_ignore_ascii_case(WINDOWS_PAYLOAD_MANIFEST)
        || value.eq_ignore_ascii_case(WINDOWS_PAYLOAD_SIGNATURE)
    {
        return Err(refusal("manifest and signature are assembler outputs"));
    }
    for part in value.split('/') {
        // Canonical package member names are ASCII; the surrounding fixture path
        // may contain spaces and Unicode. Refuse Windows path aliases on any host.
        if part.is_empty()
            || part == "."
            || part == ".."
            || part.ends_with('.')
            || !part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        {
            return Err(refusal("invalid fixture member path"));
        }
        let stem = part.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(refusal("reserved Windows fixture member name"));
        }
    }
    Ok(())
}

fn regular(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let linked = metadata.file_type().is_symlink();
    if linked || !metadata.is_file() {
        return Err(refusal("fixture input is not a regular non-link file"));
    }
    Ok(metadata)
}

fn validate(spec: &Spec) -> io::Result<()> {
    if spec.schema != SCHEMA
        || !hex(&spec.source_commit, &[40, 64])
        || !hex(&spec.cargo_lock_sha256, &[64])
        || spec.members.is_empty()
        || spec.members.len() > MAX_MEMBERS
    {
        return Err(refusal("invalid fixture specification"));
    }
    let mut names = BTreeSet::new();
    let mut prefixes = BTreeMap::new();
    let mut total = 0_u64;
    for member in &spec.members {
        member_path(&member.path)?;
        if !names.insert(member.path.as_str())
            || !member.source.is_absolute()
            || member.bytes == 0
            || member.bytes > MAX_MEMBER_BYTES
            || !hex(&member.sha256, &[64])
        {
            return Err(refusal("invalid or duplicate fixture member"));
        }
        // Windows folds every directory component, not just complete filenames.
        // Also refuse a file used as a directory, regardless of input ordering.
        let mut prefix = String::new();
        let parts: Vec<_> = member.path.split('/').collect();
        for (index, part) in parts.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let is_file = index + 1 == parts.len();
            let declaration = (prefix.clone(), is_file);
            if let Some(previous) =
                prefixes.insert(prefix.to_ascii_lowercase(), declaration.clone())
                && previous != declaration
            {
                return Err(refusal(
                    "case-folded fixture path or file/directory collision",
                ));
            }
        }
        total = total
            .checked_add(member.bytes)
            .ok_or_else(|| refusal("fixture size overflow"))?;
        if total > MAX_TOTAL_BYTES || regular(&member.source)?.len() != member.bytes {
            return Err(refusal("fixture input size mismatch or limit exceeded"));
        }
    }
    for required in [
        WINDOWS_SPEAKERS_ANALYZE_WORKER,
        WINDOWS_VAD_ANALYZE_WORKER,
        WINDOWS_ONNXRUNTIME_LIBRARY,
        WINDOWS_WESPEAKER_MODEL,
        WINDOWS_PYANNOTE_MODEL,
        WINDOWS_SILERO_VAD_MODEL,
        "bin/transcribe-native.exe",
        "bin/solstone-system-test-child.exe",
    ] {
        if !names.contains(required) {
            return Err(refusal(format!(
                "required fixture member is missing: {required}"
            )));
        }
    }
    Ok(())
}

fn create_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn copy_member(payload: &Path, member: &Member) -> io::Result<()> {
    let source = File::open(&member.source)?;
    if !source.metadata()?.is_file() || source.metadata()?.len() != member.bytes {
        return Err(refusal("fixture input changed before copy"));
    }
    let destination = payload.join(&member.path);
    fs::create_dir_all(destination.parent().unwrap())?;
    let mut destination = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut source = source.take(member.bytes + 1);
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let length = source.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        copied += length as u64;
        if copied > member.bytes {
            return Err(refusal("fixture input grew during copy"));
        }
        digest.update(&buffer[..length]);
        destination.write_all(&buffer[..length])?;
    }
    if copied != member.bytes || format!("{:x}", digest.finalize()) != member.sha256 {
        return Err(refusal(format!(
            "fixture input digest mismatch: {}",
            member.path
        )));
    }
    destination.sync_all()
}

fn require_expected_inventory(spec: &Spec, rendered: &WindowsPayloadManifest) -> io::Result<()> {
    if rendered.files.len() != spec.members.len()
        || rendered.files.iter().any(|file| {
            !spec.members.iter().any(|member| {
                member.path == file.path
                    && member.bytes == file.bytes
                    && member.sha256 == file.sha256
            })
        })
    {
        return Err(refusal(
            "staged inventory differs from exact fixture inputs",
        ));
    }
    Ok(())
}

fn finish(spec: &Spec, output: &Path) -> io::Result<Value> {
    let payload = output.join("payload");
    fs::create_dir(&payload)?;
    for member in &spec.members {
        copy_member(&payload, member)?;
    }
    // Every helper, model, DLL and harness byte is final before this call.
    let manifest =
        render_windows_payload_manifest(&payload, &spec.source_commit, &spec.cargo_lock_sha256)
            .map_err(|error| refusal(error.to_string()))?;
    let rendered: WindowsPayloadManifest =
        serde_json::from_slice(&manifest).map_err(io::Error::other)?;
    require_expected_inventory(spec, &rendered)?;
    let KeyPair { pk, sk } =
        KeyPair::generate_unencrypted_keypair().map_err(|error| refusal(error.to_string()))?;
    let public = pk
        .to_box()
        .map_err(|error| refusal(error.to_string()))?
        .to_bytes();
    let signature = minisign::sign(
        Some(&pk),
        &sk,
        Cursor::new(&manifest),
        None,
        Some("development fixture; not production release"),
    )
    .map_err(|error| refusal(error.to_string()))?
    .into_string();
    // Private key is never persisted. The public pin stays outside the payload.
    let pin = output.join("fixture.pub");
    create_bytes(&pin, &public)?;
    let manifest_path = payload.join(WINDOWS_PAYLOAD_MANIFEST);
    fs::create_dir_all(manifest_path.parent().unwrap())?;
    create_bytes(&manifest_path, &manifest)?;
    create_bytes(
        &payload.join(WINDOWS_PAYLOAD_SIGNATURE),
        signature.as_bytes(),
    )?;
    install_test_fixture_pin(&pin).map_err(|error| refusal(error.to_string()))?;
    let verified = verify_windows_payload(&payload).map_err(|error| refusal(error.to_string()))?;
    if verified.manifest() != &rendered {
        return Err(refusal("verified fixture manifest changed"));
    }
    if fs::read(&pin)? != public {
        return Err(refusal("fixture pin changed during verification"));
    }
    let digest = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    Ok(json!({
        "status": "complete",
        "purpose": "generation and ONNX development tests; not production closure",
        "payload": payload,
        "pin": pin,
        "pin_sha256": digest(&public),
        "manifest_sha256": digest(&manifest),
        "signature_sha256": digest(signature.as_bytes()),
        "source_commit_supplied_by_caller": spec.source_commit,
        "cargo_lock_sha256_supplied_by_caller": spec.cargo_lock_sha256,
        "files": spec.members.iter().map(|m| json!({
            "source": m.source, "path": m.path, "bytes": m.bytes, "sha256": m.sha256
        })).collect::<Vec<_>>()
    }))
}

fn write_receipt(file: &mut File, value: &Value) -> io::Result<()> {
    file.rewind()?;
    file.set_len(0)?;
    serde_json::to_writer_pretty(&mut *file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.sync_all()
}

#[derive(Debug)]
struct FailureReceiptError {
    primary: io::Error,
    receipt: io::Error,
}

impl std::fmt::Display for FailureReceiptError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}; failure receipt could not be written: {}",
            self.primary, self.receipt
        )
    }
}

impl std::error::Error for FailureReceiptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.primary)
    }
}

fn record_failure(receipt: &mut File, output: &Path, primary: io::Error) -> io::Error {
    let value = json!({
        "schema":SCHEMA,"status":"incomplete","output":output,"error":primary.to_string()
    });
    match write_receipt(receipt, &value) {
        Ok(()) => primary,
        Err(receipt) => io::Error::new(primary.kind(), FailureReceiptError { primary, receipt }),
    }
}

pub(super) fn run(args: &[String]) -> io::Result<()> {
    if args.len() != 2 {
        return Err(refusal(
            "windows-fixture requires SPEC.json and a fresh absolute output directory",
        ));
    }
    if std::env::var_os("SOLSTONE_JOURNAL_MINISIGN_PIN").is_some() {
        return Err(refusal("remove inherited fixture pin before assembly"));
    }
    let spec_path = Path::new(&args[0]);
    if regular(spec_path)?.len() > MAX_SPEC_BYTES {
        return Err(refusal("fixture specification exceeds size limit"));
    }
    let mut bytes = Vec::new();
    File::open(spec_path)?
        .take(MAX_SPEC_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SPEC_BYTES {
        return Err(refusal("fixture specification grew beyond size limit"));
    }
    let spec: Spec = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    validate(&spec)?;
    let requested = Path::new(&args[1]);
    if !requested.is_absolute()
        || requested
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(refusal(
            "fixture output must be an absolute non-traversing path",
        ));
    }
    let parent = requested
        .parent()
        .ok_or_else(|| refusal("fixture output needs a parent"))?
        .canonicalize()?;
    let output = parent.join(
        requested
            .file_name()
            .ok_or_else(|| refusal("fixture output needs a name"))?,
    );
    fs::create_dir(&output)?; // never replace, merge, or reuse an earlier fixture
    let mut receipt = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output.join("receipt.json"))?;
    write_receipt(
        &mut receipt,
        &json!({"schema":SCHEMA,"status":"incomplete","output":output}),
    )?;
    match finish(&spec, &output) {
        Ok(mut value) => {
            value["schema"] = SCHEMA.into();
            value["input_sha256"] = format!("{:x}", Sha256::digest(&bytes)).into();
            write_receipt(&mut receipt, &value)?;
            println!(
                "{}",
                serde_json::to_string(&value).map_err(io::Error::other)?
            );
            Ok(())
        }
        Err(error) => {
            // Keep exact partial bytes for the caller. Never remove an input or
            // guess that process exit means the fixture was assembled successfully.
            Err(record_failure(&mut receipt, &output, error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_inventory_signing_and_refusal_contract() {
        // These are packaging controls with toy regular bytes, not native models
        // or executable acceptance. Only this test installs the process-local pin.
        let scratch = tempfile::tempdir().unwrap();
        let inputs = scratch.path().join("inputs");
        fs::create_dir(&inputs).unwrap();
        let names = [
            WINDOWS_SPEAKERS_ANALYZE_WORKER,
            WINDOWS_VAD_ANALYZE_WORKER,
            WINDOWS_ONNXRUNTIME_LIBRARY,
            WINDOWS_WESPEAKER_MODEL,
            WINDOWS_PYANNOTE_MODEL,
            WINDOWS_SILERO_VAD_MODEL,
            "bin/transcribe-native.exe",
            "bin/solstone-system-test-child.exe",
        ];
        let mut members = Vec::new();
        for (index, name) in names.into_iter().enumerate() {
            let source = inputs.join(format!("member-{index}"));
            let bytes = format!("packaging-control-only:{name}").into_bytes();
            fs::write(&source, &bytes).unwrap();
            members.push(json!({
                "source": source, "path":name,"bytes":bytes.len(),
                "sha256":format!("{:x}",Sha256::digest(&bytes))
            }));
        }
        let value = json!({
            "schema":SCHEMA,"source_commit":"a".repeat(40),
            "cargo_lock_sha256":"b".repeat(64),"members":members
        });
        // Exercise the exact preflight boundary without pretending that a host
        // filesystem must support every path permitted by these parser budgets.
        for (path, accepted) in [
            (
                format!("bin/{}", "a".repeat(MAX_MEMBER_PATH_BYTES - 4)),
                true,
            ),
            (
                format!("bin/{}", "a".repeat(MAX_MEMBER_PATH_BYTES - 3)),
                false,
            ),
            (
                format!("bin/{}", vec!["a"; MAX_MEMBER_COMPONENTS - 1].join("/")),
                true,
            ),
            (
                format!("bin/{}", vec!["a"; MAX_MEMBER_COMPONENTS].join("/")),
                false,
            ),
        ] {
            let mut boundary = value.clone();
            let mut extra = boundary["members"][0].clone();
            extra["path"] = path.into();
            boundary["members"].as_array_mut().unwrap().push(extra);
            let spec: Spec = serde_json::from_value(boundary).unwrap();
            assert_eq!(validate(&spec).is_ok(), accepted);
        }
        let spec_file = scratch.path().join("spec.json");
        let write_spec = |value: &Value| {
            fs::write(&spec_file, serde_json::to_vec(value).unwrap()).unwrap();
        };
        let arguments = |output: &Path| {
            vec![
                spec_file.to_str().unwrap().to_owned(),
                output.to_str().unwrap().to_owned(),
            ]
        };
        for mode in [
            "missing",
            "duplicate",
            "case",
            "directory-case",
            "file-before-directory",
            "directory-before-file",
            "path-too-long",
            "path-too-deep",
            "traversal",
            "reserved",
            "manifest",
            "unknown",
            "zero",
            "oversized",
        ] {
            let mut changed = value.clone();
            match mode {
                "missing" => {
                    changed["members"].as_array_mut().unwrap().pop();
                }
                "duplicate" => {
                    let duplicate = changed["members"][0].clone();
                    changed["members"].as_array_mut().unwrap().push(duplicate);
                }
                "case" => {
                    let mut duplicate = changed["members"][0].clone();
                    duplicate["path"] = "bin/SOLSTONE-CORE-SPEAKERS-ANALYZE.exe".into();
                    changed["members"].as_array_mut().unwrap().push(duplicate);
                }
                "directory-case" | "file-before-directory" | "directory-before-file" => {
                    let paths = match mode {
                        "directory-case" => ["bin/Sub/a.dll", "bin/sub/b.dll"],
                        "file-before-directory" => ["bin/sub", "bin/sub/a.dll"],
                        "directory-before-file" => ["bin/sub/a.dll", "bin/sub"],
                        _ => unreachable!(),
                    };
                    for path in paths {
                        let mut extra = changed["members"][0].clone();
                        extra["path"] = path.into();
                        changed["members"].as_array_mut().unwrap().push(extra);
                    }
                }
                "traversal" => changed["members"][0]["path"] = "bin/../../escape".into(),
                "path-too-long" => {
                    changed["members"][0]["path"] =
                        format!("bin/{}", "a".repeat(MAX_MEMBER_PATH_BYTES - 3)).into()
                }
                "path-too-deep" => {
                    changed["members"][0]["path"] =
                        format!("bin/{}", vec!["a"; MAX_MEMBER_COMPONENTS].join("/")).into()
                }
                "reserved" => changed["members"][0]["path"] = "bin/NUL.exe".into(),
                "manifest" => changed["members"][0]["path"] = WINDOWS_PAYLOAD_MANIFEST.into(),
                "unknown" => changed["unexpected"] = true.into(),
                "zero" => changed["members"][0]["bytes"] = 0.into(),
                "oversized" => changed["members"][0]["bytes"] = (MAX_MEMBER_BYTES + 1).into(),
                _ => unreachable!(),
            }
            write_spec(&changed);
            let output = scratch.path().join(mode);
            assert!(run(&arguments(&output)).is_err(), "{mode}");
            assert!(!output.exists(), "{mode}: refused input mutated output");
        }
        let mut changed = value.clone();
        changed["members"][0]["sha256"] = "0".repeat(64).into();
        write_spec(&changed);
        let partial = scratch.path().join("bad-digest");
        assert!(run(&arguments(&partial)).is_err());
        let receipt: Value =
            serde_json::from_slice(&fs::read(partial.join("receipt.json")).unwrap()).unwrap();
        assert_eq!(receipt["status"], "incomplete");
        assert!(
            !partial
                .join("payload")
                .join(WINDOWS_PAYLOAD_SIGNATURE)
                .exists()
        );

        // Reach an actual assembly error, then make its retained receipt File
        // unwritable without changing filesystem permissions or process privileges.
        let failed_output = scratch.path().join("receipt-io-failure");
        fs::create_dir(&failed_output).unwrap();
        let failed_spec: Spec = serde_json::from_value(changed).unwrap();
        let primary = finish(&failed_spec, &failed_output).unwrap_err();
        assert!(
            primary
                .to_string()
                .contains("fixture input digest mismatch")
        );
        let primary_message = primary.to_string();
        let primary_kind = primary.kind();
        let receipt_path = failed_output.join("receipt.json");
        let retained = b"original incomplete receipt";
        fs::write(&receipt_path, retained).unwrap();
        let mut read_only_receipt = File::open(&receipt_path).unwrap();
        let reported = record_failure(&mut read_only_receipt, &failed_output, primary);
        assert_eq!(reported.kind(), primary_kind);
        let combined = reported
            .get_ref()
            .unwrap()
            .downcast_ref::<FailureReceiptError>()
            .unwrap();
        assert_eq!(combined.primary.to_string(), primary_message);
        assert_eq!(
            std::error::Error::source(combined).unwrap().to_string(),
            primary_message
        );
        assert!(combined.receipt.raw_os_error().is_some());
        assert!(
            reported
                .to_string()
                .contains("failure receipt could not be written")
        );
        assert_eq!(fs::read(&receipt_path).unwrap(), retained);
        drop(read_only_receipt);

        // A valid copy is not enough: compare the actual renderer's final bytes
        // with the expected inventory before signing. Prove digest and length
        // mismatches independently, then restore a positive control.
        let spec: Spec = serde_json::from_value(value.clone()).unwrap();
        let staged = scratch.path().join("final-inventory");
        fs::create_dir(&staged).unwrap();
        for member in &spec.members {
            copy_member(&staged, member).unwrap();
        }
        let render = || -> WindowsPayloadManifest {
            let bytes = render_windows_payload_manifest(
                &staged,
                &spec.source_commit,
                &spec.cargo_lock_sha256,
            )
            .unwrap();
            serde_json::from_slice(&bytes).unwrap()
        };
        require_expected_inventory(&spec, &render()).unwrap();
        let staged_member = staged.join(&spec.members[0].path);
        let original = fs::read(&staged_member).unwrap();
        let mut wrong_digest = original.clone();
        wrong_digest[0] ^= 1;
        fs::write(&staged_member, &wrong_digest).unwrap();
        let rendered = render();
        let changed = rendered
            .files
            .iter()
            .find(|f| f.path == spec.members[0].path)
            .unwrap();
        assert_eq!(changed.bytes, spec.members[0].bytes);
        assert_ne!(changed.sha256, spec.members[0].sha256);
        assert!(require_expected_inventory(&spec, &rendered).is_err());
        let mut wrong_length = original.clone();
        wrong_length.push(0);
        fs::write(&staged_member, &wrong_length).unwrap();
        let rendered = render();
        let changed = rendered
            .files
            .iter()
            .find(|f| f.path == spec.members[0].path)
            .unwrap();
        assert_ne!(changed.bytes, spec.members[0].bytes);
        assert!(require_expected_inventory(&spec, &rendered).is_err());
        assert!(!staged.join(WINDOWS_PAYLOAD_SIGNATURE).exists());
        fs::write(&staged_member, &original).unwrap();
        require_expected_inventory(&spec, &render()).unwrap();

        write_spec(&value);
        let output = scratch.path().join("signed Zoë & fixture");
        run(&arguments(&output)).unwrap();
        let receipt_bytes = fs::read(output.join("receipt.json")).unwrap();
        let receipt: Value = serde_json::from_slice(&receipt_bytes).unwrap();
        assert_eq!(receipt["status"], "complete");
        let payload = output.join("payload");
        let verified = verify_windows_payload(&payload).unwrap();
        assert_eq!(verified.manifest().files.len(), names.len());
        let pin = fs::read(output.join("fixture.pub")).unwrap();
        assert_eq!(receipt["pin_sha256"], format!("{:x}", Sha256::digest(&pin)));
        assert!(!payload.join("fixture.pub").exists());
        assert!(run(&arguments(&output)).is_err(), "never reuse output");
        assert_eq!(
            fs::read(output.join("receipt.json")).unwrap(),
            receipt_bytes
        );
        assert_eq!(fs::read(output.join("fixture.pub")).unwrap(), pin);
        let unchanged = verify_windows_payload(&payload).unwrap();
        assert_eq!(unchanged.manifest(), verified.manifest());
        let member = payload.join(WINDOWS_SPEAKERS_ANALYZE_WORKER);
        let original = fs::read(&member).unwrap();
        fs::write(&member, b"tampered staged member").unwrap();
        assert!(verify_windows_payload(&payload).is_err());
        fs::write(&member, original).unwrap();
        verify_windows_payload(&payload).unwrap();
        for member in value["members"].as_array().unwrap() {
            let original = fs::read(member["source"].as_str().unwrap()).unwrap();
            assert_eq!(member["sha256"], format!("{:x}", Sha256::digest(&original)));
        }
    }
}
