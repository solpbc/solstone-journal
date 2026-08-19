// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::inspect;

const ALLOWED_LANES: &[&str] = &["release", "staging", "dev"];
const RELEASE_KEYS: &[&str] = &["product", "version", "target", "commit", "lock_sha256"];

#[derive(Debug, Clone)]
pub struct PublishRequest {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub lane: String,
    pub fail_after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PublishReport {
    pub lane: String,
    pub version: String,
    pub dest: PathBuf,
    pub objects: Vec<PathBuf>,
    pub latest: PathBuf,
}

#[derive(Debug)]
pub struct PublishError {
    pub message: String,
}

impl PublishError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PublishError {}

impl From<io::Error> for PublishError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn run_cli(args: &[String]) -> Result<PublishReport, PublishError> {
    let request = parse_args(args)?;
    run(&request)
}

fn parse_args(args: &[String]) -> Result<PublishRequest, PublishError> {
    let mut lane = None;
    let mut dest = None;
    let mut src = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let next = || {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| PublishError::new(format!("missing value for {arg}")))
        };
        match arg.as_str() {
            "--lane" => {
                lane = Some(next()?);
                index += 2;
            }
            "--dest" => {
                dest = Some(PathBuf::from(next()?));
                index += 2;
            }
            other if other.starts_with('-') => {
                return Err(PublishError::new(format!("unknown flag {other}")));
            }
            other => {
                if src.is_some() {
                    return Err(PublishError::new(format!(
                        "unexpected:\n  extra argument {other}"
                    )));
                }
                src = Some(PathBuf::from(other));
                index += 1;
            }
        }
    }
    let mut missing = Vec::new();
    if lane.is_none() {
        missing.push("--lane");
    }
    if dest.is_none() {
        missing.push("--dest");
    }
    if src.is_none() {
        missing.push("src");
    }
    if !missing.is_empty() {
        return Err(PublishError::new(format!(
            "missing required:\n  {}",
            missing.join("\n  ")
        )));
    }
    Ok(PublishRequest {
        src: src.expect("src present"),
        dest: dest.expect("dest present"),
        lane: lane.expect("lane present"),
        fail_after: None,
    })
}

pub fn run(request: &PublishRequest) -> Result<PublishReport, PublishError> {
    if !ALLOWED_LANES.contains(&request.lane.as_str()) {
        return Err(PublishError::new(format!(
            "unexpected:\n  lane {}",
            request.lane
        )));
    }
    let version = src_release_version(&request.src)?;
    let files = list_src_files(&request.src)?;

    let version_dir = request
        .dest
        .join("solstone-journal")
        .join(&request.lane)
        .join(&version);
    fs::create_dir_all(&version_dir)?;

    let overwrite_objects = request.lane == "dev";
    let mut objects = Vec::new();
    for (name, bytes) in &files {
        let dest = version_dir.join(name);
        let step = format!("object:{name}");
        write_atomically(request, &dest, bytes, overwrite_objects, Some(&step))?;
        objects.push(dest);
    }
    checkpoint(request, "objects")?;

    let latest = request
        .dest
        .join("solstone-journal")
        .join(&request.lane)
        .join("latest");
    let body = format!("version={version}\n");
    let replace_latest = match fs::read_to_string(&latest) {
        Ok(existing) => latest_should_advance(&existing, &version),
        Err(_) => true,
    };
    if replace_latest {
        write_atomically(request, &latest, body.as_bytes(), true, None)?;
    }
    checkpoint(request, "latest")?;

    Ok(PublishReport {
        lane: request.lane.clone(),
        version,
        dest: request.dest.clone(),
        objects,
        latest,
    })
}

fn src_release_version(src: &Path) -> Result<String, PublishError> {
    if !src.is_dir() {
        return Err(PublishError::new("missing required:\n  src"));
    }
    let mut releases = Vec::new();
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.ends_with(".release") && entry.file_type()?.is_file() {
            releases.push(entry.path());
        }
    }
    let [path] = releases.as_slice() else {
        return Err(PublishError::new("unexpected:\n  .release"));
    };
    let text =
        fs::read_to_string(path).map_err(|_| PublishError::new("unexpected:\n  .release"))?;
    let pairs =
        inspect::parse_release(&text).map_err(|_| PublishError::new("unexpected:\n  .release"))?;
    version_from_pairs(&pairs)
}

fn version_from_pairs(pairs: &[(String, String)]) -> Result<String, PublishError> {
    let keys = pairs
        .iter()
        .map(|(key, _)| key.as_str())
        .collect::<BTreeSet<_>>();
    let expected = RELEASE_KEYS.iter().copied().collect::<BTreeSet<_>>();
    if keys != expected {
        return Err(PublishError::new("unexpected:\n  .release"));
    }
    let version = pairs
        .iter()
        .find(|(key, _)| key == "version")
        .map(|(_, value)| value.clone())
        .expect("version key present");
    if !is_safe_component(&version) {
        return Err(PublishError::new(format!(
            "unexpected:\n  version {version}"
        )));
    }
    Ok(version)
}

fn latest_should_advance(existing_body: &str, incoming: &str) -> bool {
    let Some(existing) = existing_body.strip_prefix("version=") else {
        return true;
    };
    let existing = existing.trim_end_matches('\n');
    version_order(incoming, existing) != Ordering::Less
}

/// Dot-separated numeric segments; non-numeric segments compare as strings (not semver).
fn version_order(left: &str, right: &str) -> Ordering {
    let mut left_parts = left.split('.');
    let mut right_parts = right.split('.');
    loop {
        match (left_parts.next(), right_parts.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left), Some(right)) => {
                let order = match (left.parse::<u64>(), right.parse::<u64>()) {
                    (Ok(left), Ok(right)) => left.cmp(&right),
                    _ => left.cmp(right),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

fn is_safe_component(value: &str) -> bool {
    if value.is_empty() || value.contains('/') || value == "." || value == ".." {
        return false;
    }
    let mut components = Path::new(value).components();
    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(name)), None) if name == value
    )
}

fn list_src_files(src: &Path) -> Result<Vec<(String, Vec<u8>)>, PublishError> {
    let mut names = Vec::new();
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    let mut files = Vec::new();
    for name in names {
        let bytes = fs::read(src.join(&name))?;
        files.push((name, bytes));
    }
    Ok(files)
}

fn fail_after(request: &PublishRequest) -> Option<String> {
    request
        .fail_after
        .clone()
        .or_else(|| env::var("SOLSTONE_DISTRIBUTION_FAIL_AFTER").ok())
}

fn checkpoint(request: &PublishRequest, step: &str) -> Result<(), PublishError> {
    if fail_after(request).as_deref() == Some(step) {
        return Err(PublishError::new(format!("injected-failure {step}")));
    }
    Ok(())
}

fn write_atomically(
    request: &PublishRequest,
    dest: &Path,
    bytes: &[u8],
    overwrite: bool,
    object_step: Option<&str>,
) -> Result<(), PublishError> {
    if dest.exists() && !overwrite {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let file_name = dest
        .file_name()
        .ok_or_else(|| PublishError::new("missing required:\n  dest file name"))?;
    let partial_name = format!("{}.publish-partial", file_name.to_string_lossy());
    let partial = dest.with_file_name(partial_name);
    fs::write(&partial, bytes)?;
    if let Some(step) = object_step {
        checkpoint(request, step)?;
    }
    rename_file_or_copy(&partial, dest)
}

fn rename_file_or_copy(src: &Path, dest: &Path) -> Result<(), PublishError> {
    match fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(copy_error) = fs::copy(src, dest) {
                let _ = fs::remove_file(dest);
                return Err(PublishError::new(format!(
                    "could not move {} to {}: cross-device copy failed: {copy_error}",
                    src.display(),
                    dest.display()
                )));
            }
            fs::remove_file(src).map_err(|error| {
                PublishError::new(format!(
                    "could not move {} to {}: {error}",
                    src.display(),
                    dest.display()
                ))
            })?;
            Ok(())
        }
        Err(error) => Err(PublishError::new(format!(
            "could not move {} to {}: {error}",
            src.display(),
            dest.display()
        ))),
    }
}

#[cfg(test)]
fn spawn_sh_install(
    args: &[impl AsRef<std::ffi::OsStr>],
    envs: &[(&str, impl AsRef<std::ffi::OsStr>)],
) -> std::process::Output {
    let mut command = std::process::Command::new("sh");
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run install.sh")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    use crate::digest::sha256_hex;
    use crate::inventory;
    use crate::promote::{self, PromoteRequest};
    use crate::provenance::Provenance;
    use crate::stage::write_staged_file_mode;
    use crate::tar::write_tar_gz;

    const HEX_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const HEX_LOCK: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const ORIGIN: &str = "https://updates.solstone.app";
    const FAKE_CURL: &str = r#"#!/bin/sh
set -eu
DEST=
HEADERS=
URL=
while [ "$#" -gt 0 ]; do
	case $1 in
	-sS | --http1.1)
		shift
		;;
	-D)
		HEADERS=$2
		shift 2
		;;
	-o)
		DEST=$2
		shift 2
		;;
	-w)
		shift 2
		;;
	*)
		URL=$1
		shift
		;;
	esac
done
ORIGIN=${FAKE_CURL_ORIGIN:-https://updates.solstone.app}
ORIGIN=${ORIGIN%/}
if [ -n "${FAKE_CURL_LOG:-}" ]; then
	printf '%s\n' "$URL" >>"$FAKE_CURL_LOG"
fi
REL=${URL#"$ORIGIN"}
REL=${REL#/}
FILE="$FAKE_CURL_ROOT/$REL"
if [ -f "$FILE" ]; then
	if [ -n "$HEADERS" ]; then
		printf 'HTTP/1.1 200 OK\r\n\r\n' >"$HEADERS"
	fi
	cp "$FILE" "$DEST"
	printf '200'
else
	if [ -n "$HEADERS" ]; then
		printf 'HTTP/1.1 404 Not Found\r\n\r\n' >"$HEADERS"
	fi
	: >"$DEST"
	printf '404'
fi
"#;

    fn temp() -> tempfile::TempDir {
        tempfile::TempDir::new_in("/var/tmp").expect("tempdir under /var/tmp")
    }

    fn inventory() -> inventory::Inventory {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../core/distribution/inventory.toml");
        inventory::load_inventory(&path).expect("committed inventory")
    }

    fn stub_produced(root: &Path, version: &str) -> PathBuf {
        let dest = root.join(format!("produced-{version}"));
        let work = root.join(format!("promote-work-{version}"));
        let _ = fs::remove_dir_all(&dest);
        let _ = fs::remove_dir_all(&work);
        let basename = inventory().artifact.render(version, "linux", "x86_64");
        promote::promote(&PromoteRequest {
            dest: dest.clone(),
            work,
            tree: vec![("bin/solstone-core".into(), b"core".to_vec(), 0o755)],
            version: version.to_owned(),
            basename,
            os: "linux".into(),
            arch: "linux-x86_64".into(),
            deb_arch: "amd64".into(),
            rpm_arch: "x86_64".into(),
            dirty: false,
            observed: Provenance {
                commit: HEX_COMMIT.into(),
                lock_sha256: HEX_LOCK.into(),
            },
            expected: Provenance {
                commit: HEX_COMMIT.into(),
                lock_sha256: HEX_LOCK.into(),
            },
            fail_after: None,
            apple: None,
        })
        .expect("promote stub src");
        dest
    }

    fn publish_request(src: &Path, dest: &Path, lane: &str) -> PublishRequest {
        PublishRequest {
            src: src.to_path_buf(),
            dest: dest.to_path_buf(),
            lane: lane.to_owned(),
            fail_after: None,
        }
    }

    fn cli_args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn snapshot(path: &Path) -> BTreeMap<String, Vec<u8>> {
        promote::snapshot_dir(path).expect("snapshot")
    }

    fn latest_body(dest: &Path, lane: &str) -> String {
        fs::read_to_string(dest.join("solstone-journal").join(lane).join("latest"))
            .expect("read latest")
    }

    fn repo_file(relative: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(relative)
    }

    fn install_sh() -> PathBuf {
        repo_file("core/distribution/install.sh")
    }

    fn write_fake_curl(dir: &Path) {
        let path = dir.join("curl");
        fs::write(&path, FAKE_CURL).expect("write fake curl");
        let mut permissions = fs::metadata(&path).expect("stat curl").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).expect("chmod curl");
    }

    fn prepend_path(dir: &Path) -> OsString {
        let mut path = dir.as_os_str().to_os_string();
        path.push(":");
        path.push(env::var_os("PATH").unwrap_or_default());
        path
    }

    fn plant_served(served: &Path, lane: &str, version: &str) {
        let target = "linux-x86_64";
        let base = format!("solstone-journal-{version}-{target}");
        let dir = served.join("solstone-journal").join(lane).join(version);
        fs::create_dir_all(&dir).expect("served version dir");
        let stage = served.join(format!("stage-{lane}-{version}"));
        write_staged_file_mode(&stage, "bin/journal", b"ok\n", 0o755).expect("stage journal");
        let archive = dir.join(format!("{base}.tar.gz"));
        write_tar_gz(&stage, &archive).expect("write archive");
        let digest = sha256_hex(&fs::read(&archive).expect("read archive"));
        fs::write(
            dir.join(format!("{base}.sha256")),
            format!("{digest}  {base}.tar.gz\n"),
        )
        .expect("sha256 sidecar");
        fs::write(
            dir.join(format!("{base}.release")),
            format!(
                "product=solstone-journal\nversion={version}\ntarget={target}\ncommit={HEX_COMMIT}\nlock_sha256={HEX_LOCK}\n"
            ),
        )
        .expect("release sidecar");
        fs::write(
            served.join("solstone-journal").join(lane).join("latest"),
            format!("version={version}\n"),
        )
        .expect("latest pointer");
    }

    fn plant_origin_root_legacy(served: &Path, lane: &str, version: &str) {
        let base = format!("solstone-journal-{version}-linux-x86_64");
        let src = served.join("solstone-journal").join(lane).join(version);
        for ext in ["tar.gz", "sha256", "release"] {
            let name = format!("{base}.{ext}");
            fs::copy(src.join(&name), served.join(&name)).expect("plant origin-root legacy");
        }
    }

    fn plant_served_with_origin_root(served: &Path, lane: &str, version: &str) {
        plant_served(served, lane, version);
        plant_origin_root_legacy(served, lane, version);
    }

    fn assert_no_origin_root_urls(urls: &[String]) {
        for url in urls {
            let path = url
                .strip_prefix(&format!("{ORIGIN}/"))
                .unwrap_or(url.as_str());
            assert!(
                path.starts_with("solstone-journal/"),
                "origin-root artifact URL {url}"
            );
        }
    }

    fn run_install(root: &Path, served: &Path, log: &Path, extra: &[&str]) -> std::process::Output {
        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("bin dir");
        write_fake_curl(&bin);
        let home = root.join("home");
        fs::create_dir_all(&home).expect("home");
        let prefix = root.join("prefix");
        let mut args = vec![
            install_sh().into_os_string(),
            "--prefix".into(),
            prefix.into_os_string(),
            "--origin".into(),
            ORIGIN.into(),
            "--no-path".into(),
        ];
        args.extend(extra.iter().map(OsString::from));
        let path = prepend_path(&bin);
        spawn_sh_install(
            &args,
            &[
                ("HOME", home.into_os_string()),
                ("PATH", path),
                ("TMPDIR", root.as_os_str().to_os_string()),
                ("FAKE_CURL_ROOT", served.as_os_str().to_os_string()),
                ("FAKE_CURL_LOG", log.as_os_str().to_os_string()),
                ("FAKE_CURL_ORIGIN", OsString::from(ORIGIN)),
                ("SOLSTONE_UNAME_S", OsString::from("Linux")),
                ("SOLSTONE_UNAME_M", OsString::from("x86_64")),
            ],
        )
    }

    fn recorded_urls(log: &Path) -> Vec<String> {
        if !log.exists() {
            return Vec::new();
        }
        fs::read_to_string(log)
            .expect("read curl log")
            .lines()
            .map(str::to_owned)
            .filter(|line| !line.is_empty())
            .collect()
    }

    fn object_urls(lane: &str, version: &str) -> Vec<String> {
        let base = format!("solstone-journal-{version}-linux-x86_64");
        ["tar.gz", "sha256", "release"]
            .into_iter()
            .map(|ext| format!("{ORIGIN}/solstone-journal/{lane}/{version}/{base}.{ext}"))
            .collect()
    }

    fn write_release(path: &Path, version: &str) {
        fs::write(
            path,
            format!(
                "product=solstone-journal\nversion={version}\ntarget=linux-x86_64\ncommit={HEX_COMMIT}\nlock_sha256={HEX_LOCK}\n"
            ),
        )
        .expect("write .release");
    }

    #[test]
    fn ac1_publish_writes_lane_version_layout_and_latest() {
        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let dest = root.path().join("dest");
        let report = run_cli(&cli_args(&[
            "--lane",
            "release",
            src.to_str().unwrap(),
            "--dest",
            dest.to_str().unwrap(),
        ]))
        .expect("publish");
        assert_eq!(report.lane, "release");
        assert_eq!(report.version, "1.2.3");
        let version_dir = dest.join("solstone-journal/release/1.2.3");
        let mut expected = fs::read_dir(&src)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        expected.sort();
        let mut found = fs::read_dir(&version_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        found.sort();
        assert_eq!(found, expected);
        for name in &expected {
            assert_eq!(
                fs::read(src.join(name)).unwrap(),
                fs::read(version_dir.join(name)).unwrap(),
                "{name}"
            );
        }
        assert_eq!(latest_body(&dest, "release"), "version=1.2.3\n");
        assert_eq!(report.latest, dest.join("solstone-journal/release/latest"));
    }

    #[test]
    fn ac2_latest_pointer_advances_to_v2_in_every_lane() {
        for lane in ALLOWED_LANES {
            let root = temp();
            let src_v1 = stub_produced(root.path(), "1.0.0");
            let src_v2 = stub_produced(root.path(), "2.0.0");
            let dest = root.path().join("dest");
            run(&publish_request(&src_v1, &dest, lane)).expect("publish v1");
            assert_eq!(latest_body(&dest, lane), "version=1.0.0\n");
            let v1_dir = dest.join("solstone-journal").join(lane).join("1.0.0");
            let v1_bytes = snapshot(&v1_dir);
            run(&publish_request(&src_v2, &dest, lane)).expect("publish v2");
            assert_eq!(latest_body(&dest, lane), "version=2.0.0\n");
            assert_eq!(snapshot(&v1_dir), v1_bytes, "{lane} v1 objects");
            assert!(
                dest.join("solstone-journal")
                    .join(lane)
                    .join("2.0.0")
                    .is_dir()
            );
        }
    }

    #[test]
    fn ac3_fail_after_objects_leaves_all_objects_and_prior_latest() {
        let root = temp();
        let src_v1 = stub_produced(root.path(), "1.0.0");
        let src_v2 = stub_produced(root.path(), "2.0.0");
        let dest = root.path().join("dest");
        run(&publish_request(&src_v1, &dest, "release")).expect("publish v1");
        let mut request = publish_request(&src_v2, &dest, "release");
        request.fail_after = Some("objects".into());
        let error = run(&request).expect_err("injected objects");
        assert!(error.to_string().contains("injected-failure objects"));
        assert_eq!(latest_body(&dest, "release"), "version=1.0.0\n");
        let version_dir = dest.join("solstone-journal/release/2.0.0");
        for entry in fs::read_dir(&src_v2).unwrap() {
            let name = entry.unwrap().file_name();
            let path = version_dir.join(&name);
            assert!(path.is_file(), "{}", name.to_string_lossy());
            assert_eq!(
                fs::read(src_v2.join(&name)).unwrap(),
                fs::read(&path).unwrap()
            );
            assert!(
                !version_dir
                    .join(format!("{}.publish-partial", name.to_string_lossy()))
                    .exists()
            );
        }
    }

    #[test]
    fn ac4_fail_after_object_leaves_partial_not_final() {
        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let dest = root.path().join("dest");
        let mut names = fs::read_dir(&src)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        let target = names[names.len() / 2].clone();
        let mut request = publish_request(&src, &dest, "release");
        request.fail_after = Some(format!("object:{target}"));
        let error = run(&request).expect_err("injected object");
        assert!(
            error
                .to_string()
                .contains(&format!("injected-failure object:{target}"))
        );
        let version_dir = dest.join("solstone-journal/release/1.2.3");
        let mut before = true;
        for name in &names {
            let final_path = version_dir.join(name);
            let partial = version_dir.join(format!("{name}.publish-partial"));
            if *name == target {
                assert!(!final_path.exists(), "final {name}");
                assert!(partial.is_file(), "partial {name}");
                assert_eq!(
                    fs::read(src.join(name)).unwrap(),
                    fs::read(&partial).unwrap()
                );
                before = false;
                continue;
            }
            if before {
                assert!(final_path.is_file(), "final {name}");
                assert!(!partial.exists(), "partial {name}");
            } else {
                assert!(!final_path.exists(), "later final {name}");
                assert!(!partial.exists(), "later partial {name}");
            }
        }
        assert!(!dest.join("solstone-journal/release/latest").exists());
    }

    #[test]
    fn ac5_second_publish_same_version_only_adds_new_filenames() {
        for lane in ["release", "staging"] {
            let root = temp();
            let src = stub_produced(root.path(), "1.2.3");
            let dest = root.path().join("dest");
            run(&publish_request(&src, &dest, lane)).expect("first publish");
            let version_dir = dest.join("solstone-journal").join(lane).join("1.2.3");
            let existing = version_dir.join("solstone-journal-1.2.3-linux-x86_64.tar.gz");
            let before = fs::read(&existing).unwrap();
            fs::write(src.join("extra.txt"), b"new-bytes").unwrap();
            fs::write(&existing, b"tampered").unwrap();
            run(&publish_request(&src, &dest, lane)).expect("second publish");
            assert_eq!(fs::read(&existing).unwrap(), b"tampered");
            assert_ne!(fs::read(&existing).unwrap(), before);
            assert_eq!(
                fs::read(version_dir.join("extra.txt")).unwrap(),
                b"new-bytes"
            );
            assert_eq!(latest_body(&dest, lane), "version=1.2.3\n");
        }

        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let dest = root.path().join("dest");
        run(&publish_request(&src, &dest, "dev")).expect("first");
        let version_dir = dest.join("solstone-journal/dev/1.2.3");
        let name = "solstone-journal-1.2.3-linux-x86_64.tar.gz";
        fs::write(src.join(name), b"replacement").unwrap();
        run(&publish_request(&src, &dest, "dev")).expect("overwrite");
        assert_eq!(fs::read(version_dir.join(name)).unwrap(), b"replacement");
        assert_eq!(latest_body(&dest, "dev"), "version=1.2.3\n");

        let root = temp();
        let src_v1 = stub_produced(root.path(), "1.0.0");
        let src_v2 = stub_produced(root.path(), "2.0.0");
        let dest = root.path().join("dest");
        run(&publish_request(&src_v1, &dest, "release")).expect("publish v1");
        run(&publish_request(&src_v2, &dest, "release")).expect("publish v2");
        assert_eq!(latest_body(&dest, "release"), "version=2.0.0\n");
        fs::write(src_v1.join("extra.txt"), b"late-v1").unwrap();
        run(&publish_request(&src_v1, &dest, "release")).expect("republish v1");
        assert_eq!(latest_body(&dest, "release"), "version=2.0.0\n");
        assert_eq!(
            fs::read(dest.join("solstone-journal/release/1.0.0/extra.txt")).unwrap(),
            b"late-v1"
        );
    }

    #[test]
    fn ac6_invalid_lane_refuses_before_any_dest_write() {
        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let dest = root.path().join("dest");
        fs::create_dir_all(dest.join("solstone-journal/other")).unwrap();
        fs::write(dest.join("solstone-journal/other/keep.txt"), b"keep").unwrap();
        fs::write(dest.join("outside.txt"), b"out").unwrap();
        let before = snapshot(&dest);
        for lane in ["Release", "../runtimes", "release/../../assets", "prod", ""] {
            let error = run(&publish_request(&src, &dest, lane)).expect_err(lane);
            assert!(
                error
                    .to_string()
                    .contains(&format!("unexpected:\n  lane {lane}")),
                "{error}"
            );
            assert_eq!(snapshot(&dest), before, "{lane}");
        }
    }

    #[test]
    fn ac7_invalid_src_refuses_before_any_dest_write() {
        let root = temp();
        let dest = root.path().join("dest");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("marker"), b"prior").unwrap();
        let before = snapshot(&dest);

        let missing = root.path().join("missing-src");
        let error = run(&publish_request(&missing, &dest, "release")).expect_err("missing src");
        assert!(error.to_string().contains("missing required:\n  src"));
        assert_eq!(snapshot(&dest), before);

        let empty = root.path().join("empty-src");
        fs::create_dir_all(&empty).unwrap();
        let error = run(&publish_request(&empty, &dest, "release")).expect_err("zero release");
        assert!(error.to_string().contains("unexpected:\n  .release"));
        assert_eq!(snapshot(&dest), before);

        write_release(&empty.join("a.release"), "1.2.3");
        write_release(&empty.join("b.release"), "1.2.3");
        let error = run(&publish_request(&empty, &dest, "release")).expect_err("two releases");
        assert!(error.to_string().contains("unexpected:\n  .release"));
        assert_eq!(snapshot(&dest), before);

        let malformed = root.path().join("malformed");
        fs::create_dir_all(&malformed).unwrap();
        fs::write(malformed.join("tree.release"), "not-key-value\n").unwrap();
        let error = run(&publish_request(&malformed, &dest, "release")).expect_err("malformed");
        assert!(error.to_string().contains("unexpected:\n  .release"));
        assert_eq!(snapshot(&dest), before);

        let wrong_keys = root.path().join("wrong-keys");
        fs::create_dir_all(&wrong_keys).unwrap();
        fs::write(
            wrong_keys.join("tree.release"),
            "product=solstone-journal\nversion=1.2.3\ntarget=linux-x86_64\ncommit=aaa\nfoo=bar\n",
        )
        .unwrap();
        let error = run(&publish_request(&wrong_keys, &dest, "release")).expect_err("keys");
        assert!(error.to_string().contains("unexpected:\n  .release"));
        assert_eq!(snapshot(&dest), before);

        let bad_version = root.path().join("bad-version");
        fs::create_dir_all(&bad_version).unwrap();
        write_release(&bad_version.join("tree.release"), "../escape");
        let error = run(&publish_request(&bad_version, &dest, "release")).expect_err("version");
        assert!(
            error
                .to_string()
                .contains("unexpected:\n  version ../escape")
        );
        assert_eq!(snapshot(&dest), before);
    }

    #[test]
    fn ac8_publish_writes_no_stray_root_artifacts() {
        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let dest = root.path().join("dest");
        run(&publish_request(&src, &dest, "release")).expect("publish");
        let mut top = fs::read_dir(&dest)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        top.sort();
        assert_eq!(top, ["solstone-journal"]);
        let stem = "solstone-journal-1.2.3-linux-x86_64";
        for entry in fs::read_dir(&dest).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(
                !name.starts_with(&format!("{stem}.")),
                "stray artifact at dest/: {name}"
            );
        }
        for stray in [
            "runtimes",
            "assets",
            "providers",
            "solstone-macos",
            "solstone-windows",
            "journal-macos",
        ] {
            assert!(!dest.join(stray).exists(), "{stray}");
        }
    }

    #[test]
    fn ac9_omitted_version_fetches_latest_then_release_objects() {
        let root = temp();
        let served = root.path().join("served");
        plant_served_with_origin_root(&served, "release", "1.2.3");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected = vec![format!("{ORIGIN}/solstone-journal/release/latest")];
        expected.extend(object_urls("release", "1.2.3"));
        let urls = recorded_urls(&log);
        assert_eq!(urls, expected);
        assert_no_origin_root_urls(&urls);
    }

    #[test]
    fn ac10_explicit_version_skips_latest() {
        let root = temp();
        let served = root.path().join("served");
        plant_served_with_origin_root(&served, "release", "1.2.3");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &["--version", "1.2.3"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let urls = recorded_urls(&log);
        assert_eq!(urls, object_urls("release", "1.2.3"));
        assert!(!urls.iter().any(|url| url.ends_with("/latest")));
        assert_no_origin_root_urls(&urls);
    }

    #[test]
    fn ac11_lane_flag_selects_staging_object_urls() {
        let root = temp();
        let served = root.path().join("served");
        plant_served(&served, "staging", "9.9.9");
        let log = root.path().join("curl.log");
        let output = run_install(
            root.path(),
            &served,
            &log,
            &["--lane", "staging", "--version", "9.9.9"],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(recorded_urls(&log), object_urls("staging", "9.9.9"));
        assert!(
            !recorded_urls(&log)
                .iter()
                .any(|url| url.contains("/release/"))
        );
    }

    #[test]
    fn ac12_lane_dev_omitted_version_records_dev_latest_then_objects() {
        let root = temp();
        let served = root.path().join("served");
        plant_served_with_origin_root(&served, "dev", "0.1.0");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &["--lane", "dev"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let mut expected = vec![format!("{ORIGIN}/solstone-journal/dev/latest")];
        expected.extend(object_urls("dev", "0.1.0"));
        let urls = recorded_urls(&log);
        assert_eq!(urls, expected);
        assert_no_origin_root_urls(&urls);
    }

    #[test]
    fn ac13_bad_latest_pointer_is_latest_invalid() {
        let root = temp();
        let served = root.path().join("served");
        plant_served(&served, "release", "1.2.3");
        fs::remove_file(served.join("solstone-journal/release/latest")).unwrap();
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &[]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("latest-invalid"), "{stderr}");
        assert!(!stderr.contains("origin-refused"), "{stderr}");
        assert!(!stderr.contains("release-invalid"), "{stderr}");
        assert_eq!(
            recorded_urls(&log),
            vec![format!("{ORIGIN}/solstone-journal/release/latest")]
        );
        assert!(!root.path().join("prefix/versions").exists());

        fs::write(
            served.join("solstone-journal/release/latest"),
            "version=../escape\n",
        )
        .unwrap();
        let log2 = root.path().join("curl2.log");
        let output = run_install(root.path(), &served, &log2, &[]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("latest-invalid"), "{stderr}");
        assert!(!root.path().join("prefix/versions").exists());
    }

    #[test]
    fn ac14_omitted_lane_defaults_to_release() {
        let root = temp();
        let served = root.path().join("served");
        plant_served(&served, "release", "1.2.3");
        plant_served(&served, "staging", "1.2.3");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let urls = recorded_urls(&log);
        assert!(
            urls.iter()
                .all(|url| url.contains("/solstone-journal/release/"))
        );
        assert!(
            !urls
                .iter()
                .any(|url| url.contains("/staging/") || url.contains("/dev/"))
        );
    }

    #[test]
    fn ac15_invalid_lane_is_lane_invalid_before_any_url() {
        for lane in ["../runtimes", "release/../../assets", "Release"] {
            let root = temp();
            let served = root.path().join("served");
            plant_served(&served, "release", "1.2.3");
            let log = root.path().join("curl.log");
            let output = run_install(root.path(), &served, &log, &["--lane", lane]);
            assert!(!output.status.success(), "{lane}");
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(stderr.contains("lane-invalid"), "{lane}: {stderr}");
            assert!(recorded_urls(&log).is_empty(), "{lane}");
            assert!(!root.path().join("prefix/versions").exists(), "{lane}");
            assert!(
                !root.path().join("prefix").exists()
                    || snapshot(&root.path().join("prefix")).is_empty(),
                "{lane}"
            );
        }
    }

    #[test]
    fn ac16_resume_after_objects_failure_sets_latest_to_v2() {
        let root = temp();
        let src_v1 = stub_produced(root.path(), "1.0.0");
        let src_v2 = stub_produced(root.path(), "2.0.0");
        let dest = root.path().join("dest");
        run(&publish_request(&src_v1, &dest, "release")).expect("publish v1");
        let mut failed = publish_request(&src_v2, &dest, "release");
        failed.fail_after = Some("objects".into());
        let error = run(&failed).expect_err("injected objects");
        assert!(error.to_string().contains("injected-failure objects"));
        assert_eq!(latest_body(&dest, "release"), "version=1.0.0\n");
        let v2_dir = dest.join("solstone-journal/release/2.0.0");
        let v2_bytes = snapshot(&v2_dir);
        run(&publish_request(&src_v2, &dest, "release")).expect("resume v2");
        assert_eq!(snapshot(&v2_dir), v2_bytes);
        assert_eq!(latest_body(&dest, "release"), "version=2.0.0\n");
    }

    #[test]
    fn publish_output_is_a_valid_install_sh_origin() {
        let root = temp();
        let src = stub_produced(root.path(), "1.2.3");
        let origin = root.path().join("origin");
        run(&publish_request(&src, &origin, "release")).expect("publish");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &origin, &log, &[]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root.path().join("prefix/current").is_symlink());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("installed solstone-journal 1.2.3"),
            "{stdout}"
        );
    }

    #[test]
    fn ac17_bad_origin_host_is_origin_refused_before_dest_write() {
        for origin in ["https://example.com", "http://updates.solstone.app"] {
            for extra in [&[][..], &["--version", "1.2.3"][..]] {
                let root = temp();
                let home = root.path().join("home");
                fs::create_dir_all(&home).unwrap();
                let prefix = root.path().join("prefix");
                let mut args = vec![
                    install_sh().into_os_string(),
                    "--prefix".into(),
                    prefix.clone().into_os_string(),
                    "--origin".into(),
                    origin.into(),
                    "--no-path".into(),
                ];
                args.extend(extra.iter().map(OsString::from));
                let output = spawn_sh_install(
                    &args,
                    &[
                        ("HOME", home.into_os_string()),
                        ("TMPDIR", root.path().as_os_str().to_os_string()),
                        ("SOLSTONE_UNAME_S", OsString::from("Linux")),
                        ("SOLSTONE_UNAME_M", OsString::from("x86_64")),
                    ],
                );
                assert!(!output.status.success(), "{origin} {extra:?}");
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(
                    stderr.contains("origin-refused"),
                    "{origin} {extra:?}: {stderr}"
                );
                assert!(!prefix.exists(), "{origin} {extra:?}");
            }
        }
    }

    #[test]
    fn ac18_omitted_lane_with_explicit_version_uses_release() {
        let root = temp();
        let served = root.path().join("served");
        plant_served(&served, "release", "4.5.6");
        plant_served(&served, "dev", "4.5.6");
        let log = root.path().join("curl.log");
        let output = run_install(root.path(), &served, &log, &["--version", "4.5.6"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(recorded_urls(&log), object_urls("release", "4.5.6"));
    }

    #[test]
    fn install_base_formula_and_archive_refusals_are_unchanged() {
        let install = fs::read_to_string(install_sh()).expect("install.sh");
        assert!(
            install
                .lines()
                .any(|line| line.trim() == "_base=${PRODUCT}-${VERSION}-${TARGET}"),
            "byte-matched _base= formula"
        );
        let expected_archive = [
            "# ARCHIVE_REFUSALS:",
            "#   archive-absolute-path",
            "#   archive-parent-traversal",
            "#   archive-symlink-escape",
            "#   archive-hardlink-escape",
            "#   archive-symlink-then-child",
        ];
        let header = install.lines().take(25).collect::<Vec<_>>();
        for line in expected_archive {
            assert!(
                header.iter().any(|item| item.trim_end() == line),
                "missing {line}"
            );
        }
    }

    #[test]
    fn install_sh_has_no_origin_root_fetch_fallback() {
        let install = fs::read_to_string(install_sh()).expect("install.sh");
        assert!(
            !install.contains("fetch_url \"${_origin}/${_base}.tar.gz\""),
            "origin-root fetch fallback must not remain"
        );
        assert!(
            install.contains("_object_base=\"${_origin}/solstone-journal/${LANE}/${VERSION}\"")
        );
        assert!(install.contains("lane-invalid"));
        assert!(install.contains("latest-invalid"));
    }

    #[test]
    fn cli_parse_wording_matches_acquire() {
        let error = run_cli(&cli_args(&["--lane"])).expect_err("missing value");
        assert_eq!(error.to_string(), "missing value for --lane");
        let error = run_cli(&cli_args(&["--weird", "x"])).expect_err("unknown");
        assert_eq!(error.to_string(), "unknown flag --weird");
        let error = run_cli(&cli_args(&["src"])).expect_err("missing");
        assert!(error.to_string().contains("missing required:\n  --lane"));
        assert!(error.to_string().contains("--dest"));
    }
}
