// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unsigned POSIX installed-payload manifest and member resolver.
//!
//! The package root is the directory that contains `bin/`, `lib/`, and
//! `share/`. Discovery looks only at the parent directory name `bin`. It does
//! not walk ancestors, read an environment variable, or consult a config key.
//! Digest state lives on the operation value the caller holds.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::code;
use crate::guidance;

pub const COMPILED_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const PRODUCT: &str = "solstone-journal";
pub const INSTALLED_PAYLOAD_SCHEMA: &str = "solstone.installed-payload.v1";
pub const INSTALLED_PAYLOAD_MANIFEST: &str = "share/solstone-journal/installed-payload.json";
pub const INSTALLED_PAYLOAD_SIGNATURE: &str =
    "share/solstone-journal/installed-payload.json.minisig";
pub const TARGET_LINUX_X86_64: &str = "linux-x86_64";
pub const TARGET_LINUX_AARCH64: &str = "linux-aarch64";
pub const TARGET_MACOS_ARM64: &str = "macos-arm64";

const DELETED_SUFFIX: &str = " (deleted)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutablePlatform {
    Linux,
    Macos,
    Windows,
}

#[must_use]
pub fn host_executable_platform() -> ExecutablePlatform {
    #[cfg(target_os = "linux")]
    {
        ExecutablePlatform::Linux
    }
    #[cfg(target_os = "macos")]
    {
        ExecutablePlatform::Macos
    }
    #[cfg(target_os = "windows")]
    {
        ExecutablePlatform::Windows
    }
}

#[must_use]
pub fn canonical_target(id: &str) -> Option<&'static str> {
    match id {
        TARGET_LINUX_X86_64 => Some(TARGET_LINUX_X86_64),
        TARGET_LINUX_AARCH64 => Some(TARGET_LINUX_AARCH64),
        TARGET_MACOS_ARM64 => Some(TARGET_MACOS_ARM64),
        crate::TARGET_WINDOWS_X86_64 => Some(crate::TARGET_WINDOWS_X86_64),
        _ => None,
    }
}

#[derive(Debug)]
pub struct InstalledPayloadRefusal {
    pub code: &'static str,
    pub guidance: &'static str,
    pub path: Option<String>,
    pub versions: Option<Box<(String, String)>>,
    pub targets: Option<Box<(String, String)>>,
    pub io: Option<Box<io::Error>>,
}

impl fmt::Display for InstalledPayloadRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code)?;
        if let Some(path) = &self.path {
            write!(formatter, " {path}")?;
        }
        if let Some((expected, found)) = self.versions.as_deref() {
            write!(formatter, " {expected} {found}")?;
        }
        if let Some((expected, found)) = self.targets.as_deref() {
            write!(formatter, " {expected} {found}")?;
        }
        if let Some(error) = &self.io {
            write!(formatter, " {error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for InstalledPayloadRefusal {}

fn refusal(
    code_token: &'static str,
    guidance_text: &'static str,
    path: Option<String>,
) -> InstalledPayloadRefusal {
    InstalledPayloadRefusal {
        code: code_token,
        guidance: guidance_text,
        path,
        versions: None,
        targets: None,
        io: None,
    }
}

fn manifest_missing(path: impl Into<String>, restart: bool) -> InstalledPayloadRefusal {
    refusal(
        code::MANIFEST_MISSING,
        if restart {
            guidance::RESTART_UPDATE
        } else {
            guidance::MANIFEST_MISSING
        },
        Some(path.into()),
    )
}

fn manifest_invalid() -> InstalledPayloadRefusal {
    refusal(code::MANIFEST_INVALID, guidance::MANIFEST_INVALID, None)
}

fn unsupported(path: &Path) -> InstalledPayloadRefusal {
    refusal(
        code::UNSUPPORTED_LOCATION,
        guidance::UNSUPPORTED_LOCATION,
        Some(path.display().to_string()),
    )
}

fn unsafe_path(path: &str) -> InstalledPayloadRefusal {
    refusal(
        code::UNSAFE_PATH,
        guidance::UNSAFE_PATH,
        Some(path.to_owned()),
    )
}

fn unexpected_file(path: &str) -> InstalledPayloadRefusal {
    refusal(
        code::UNEXPECTED_FILE,
        guidance::PACKAGE_MISMATCH,
        Some(path.to_owned()),
    )
}

fn member_missing(path: &str) -> InstalledPayloadRefusal {
    refusal(
        code::MEMBER_MISSING,
        guidance::PACKAGE_MISMATCH,
        Some(path.to_owned()),
    )
}

fn member_changed(path: &str) -> InstalledPayloadRefusal {
    // A same-version respin is supposed to look like a changed member.
    // The version did not move, so the installed bytes no longer match.
    refusal(
        code::MEMBER_CHANGED,
        guidance::PACKAGE_MISMATCH,
        Some(path.to_owned()),
    )
}

fn unreadable(path: &str, error: io::Error) -> InstalledPayloadRefusal {
    InstalledPayloadRefusal {
        code: code::MEMBER_UNREADABLE,
        guidance: guidance::MEMBER_UNREADABLE,
        path: Some(path.to_owned()),
        versions: None,
        targets: None,
        io: Some(Box::new(error)),
    }
}

#[derive(Debug, Serialize)]
struct RenderedManifest<'a> {
    schema: &'a str,
    product: &'a str,
    version: &'a str,
    target: &'a str,
    source_commit: &'a str,
    files: Vec<RenderedFile>,
}

#[derive(Debug, Serialize)]
struct RenderedFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedManifest {
    schema: String,
    product: String,
    version: String,
    target: String,
    source_commit: String,
    files: Vec<ParsedFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParsedFile {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
struct ListedFile {
    bytes: u64,
    sha256: String,
}

#[derive(Debug)]
pub struct InstalledPackage {
    root: PathBuf,
    files: BTreeMap<String, ListedFile>,
    seen: Mutex<BTreeMap<String, String>>,
}

struct LooseHeader {
    product: String,
    version: String,
    target: String,
}

pub fn render_installed_payload(
    root: &Path,
    product: &str,
    version: &str,
    target: &str,
    source_commit: &str,
) -> Result<Vec<u8>, InstalledPayloadRefusal> {
    let mut collected = Vec::new();
    walk_regular(root, root, &mut collected)?;
    collected.sort_by(|left, right| left.0.cmp(&right.0));
    let mut folded = BTreeSet::new();
    for (path, _) in &collected {
        if !folded.insert(path.to_ascii_lowercase()) {
            return Err(unsafe_path(path));
        }
    }
    let files = collected
        .into_iter()
        .map(|(path, bytes)| RenderedFile {
            sha256: sha256_hex(&bytes),
            bytes: bytes.len() as u64,
            path,
        })
        .collect();
    let document = RenderedManifest {
        schema: INSTALLED_PAYLOAD_SCHEMA,
        product,
        version,
        target,
        source_commit,
        files,
    };
    serde_json::to_vec(&document).map_err(|_| manifest_invalid())
}

fn walk_regular(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), InstalledPayloadRefusal> {
    let mut entries = fs::read_dir(dir)
        .map_err(|error| unreadable(&relative_path(root, dir), error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| unreadable(&relative_path(root, dir), error))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = relative_path(root, &path);
        if relative == INSTALLED_PAYLOAD_MANIFEST || relative == INSTALLED_PAYLOAD_SIGNATURE {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| unreadable(&relative, error))?;
        if file_type.is_dir() {
            walk_regular(root, &path, out)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(unsafe_path(&relative));
        }
        let bytes = fs::read(&path).map_err(|error| unreadable(&relative, error))?;
        out.push((relative, bytes));
    }
    Ok(())
}

impl InstalledPackage {
    pub fn admit(
        root: &Path,
        version: &str,
        target: &str,
    ) -> Result<Self, InstalledPayloadRefusal> {
        let bytes = read_manifest_bytes(root)?;
        let loose = loose_header(&bytes)?;
        if loose.product != PRODUCT {
            return Err(refusal(code::WRONG_PRODUCT, guidance::WRONG_PRODUCT, None));
        }
        if loose.version != version {
            return Err(InstalledPayloadRefusal {
                code: code::RESTART_TO_FINISH_UPDATE,
                guidance: guidance::RESTART_UPDATE,
                path: None,
                versions: Some(Box::new((version.to_owned(), loose.version))),
                targets: None,
                io: None,
            });
        }
        if loose.target != target {
            return Err(InstalledPayloadRefusal {
                code: code::WRONG_TARGET,
                guidance: guidance::WRONG_TARGET,
                path: None,
                versions: None,
                targets: Some(Box::new((target.to_owned(), loose.target))),
                io: None,
            });
        }
        let parsed: ParsedManifest =
            serde_json::from_slice(&bytes).map_err(|_| manifest_invalid())?;
        if parsed.schema != INSTALLED_PAYLOAD_SCHEMA
            || parsed.product != loose.product
            || parsed.version != loose.version
            || parsed.target != loose.target
        {
            return Err(manifest_invalid());
        }
        let _ = parsed.source_commit;
        validate_listed_paths(&parsed.files)?;
        let files = parsed
            .files
            .into_iter()
            .map(|file| {
                (
                    file.path,
                    ListedFile {
                        bytes: file.bytes,
                        sha256: file.sha256,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        census(root, &files)?;
        check_listed_members(root, &files)?;
        Ok(Self {
            root: root.to_path_buf(),
            files,
            seen: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn member(&self, path: &str) -> Result<PathBuf, InstalledPayloadRefusal> {
        let listed = self.files.get(path).ok_or_else(|| member_missing(path))?;
        let full = resolve_member(&self.root, path)?;
        let bytes = read_member(&full, path)?;
        let digest = sha256_hex(&bytes);
        if !digest_matches(&bytes, listed.bytes, &listed.sha256) {
            return Err(member_changed(path));
        }
        let mut seen = self.seen.lock().expect("installed payload digest record");
        seen.insert(path.to_owned(), digest);
        let _recorded = seen.get(path);
        Ok(full)
    }
}

pub fn verify_installed_package(
    root: &Path,
    version: &str,
    target: &str,
) -> Result<(), InstalledPayloadRefusal> {
    let package = InstalledPackage::admit(root, version, target)?;
    for path in package.files.keys() {
        package.member(path)?;
    }
    Ok(())
}

pub fn bin_parent_root(executable: &Path) -> Option<PathBuf> {
    let bin = executable.parent()?;
    if bin.file_name() != Some(std::ffi::OsStr::new("bin")) {
        return None;
    }
    bin.parent().map(Path::to_path_buf)
}

pub fn package_root_from_executable_path(
    path: &Path,
    platform: ExecutablePlatform,
) -> Result<PathBuf, InstalledPayloadRefusal> {
    match platform {
        ExecutablePlatform::Linux => bin_parent_root(path).ok_or_else(|| unsupported(path)),
        ExecutablePlatform::Macos => match path.canonicalize() {
            Ok(canonical) => bin_parent_root(&canonical).ok_or_else(|| unsupported(path)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                Err(manifest_missing(path.display().to_string(), true))
            }
            Err(error) => Err(unreadable(&path.display().to_string(), error)),
        },
        ExecutablePlatform::Windows => windows_package_root(path),
    }
}

pub fn locate_installed_package(
    executable: &Path,
    platform: ExecutablePlatform,
) -> Result<PathBuf, InstalledPayloadRefusal> {
    let root = package_root_from_executable_path(executable, platform)?;
    let manifest = root.join(INSTALLED_PAYLOAD_MANIFEST);
    match fs::symlink_metadata(&manifest) {
        Ok(meta) if meta.file_type().is_symlink() => Err(unsafe_path(INSTALLED_PAYLOAD_MANIFEST)),
        Ok(_) => Ok(root),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(manifest_missing(
            executable.display().to_string(),
            deleted_executable(executable),
        )),
        Err(error) => Err(unreadable(INSTALLED_PAYLOAD_MANIFEST, error)),
    }
}

fn windows_package_root(path: &Path) -> Result<PathBuf, InstalledPayloadRefusal> {
    let Some(text) = path.to_str() else {
        return Err(unsupported(path));
    };
    let mut parts = text.split('\\').collect::<Vec<_>>();
    if parts.last() == Some(&"") {
        parts.pop();
    }
    if parts.len() < 2 || parts[parts.len() - 2] != "bin" {
        return Err(unsupported(path));
    }
    let root = parts[..parts.len() - 2].join("\\");
    if root.is_empty() {
        return Err(unsupported(path));
    }
    Ok(PathBuf::from(root))
}

fn deleted_executable(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(DELETED_SUFFIX))
}

fn loose_header(bytes: &[u8]) -> Result<LooseHeader, InstalledPayloadRefusal> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| manifest_invalid())?;
    let Some(object) = value.as_object() else {
        return Err(manifest_invalid());
    };
    let product = object
        .get("product")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(manifest_invalid)?;
    let version = object
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(manifest_invalid)?;
    let target = object
        .get("target")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(manifest_invalid)?;
    Ok(LooseHeader {
        product: product.to_owned(),
        version: version.to_owned(),
        target: target.to_owned(),
    })
}

fn validate_listed_paths(files: &[ParsedFile]) -> Result<(), InstalledPayloadRefusal> {
    let mut previous = "";
    let mut folded = BTreeSet::new();
    for file in files {
        validate_relative(&file.path)?;
        if file.path.as_str() <= previous {
            return Err(unsafe_path(&file.path));
        }
        previous = &file.path;
        if !folded.insert(file.path.to_ascii_lowercase()) {
            return Err(unsafe_path(&file.path));
        }
        if !is_sha256(&file.sha256) {
            return Err(manifest_invalid());
        }
    }
    Ok(())
}

fn validate_relative(path: &str) -> Result<(), InstalledPayloadRefusal> {
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\\')
        || path.contains('\0')
        || path.as_bytes().get(1) == Some(&b':')
    {
        return Err(unsafe_path(path));
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(unsafe_path(path));
    }
    Ok(())
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn census(
    root: &Path,
    files: &BTreeMap<String, ListedFile>,
) -> Result<(), InstalledPayloadRefusal> {
    let listed = files.keys().cloned().collect::<BTreeSet<_>>();
    for namespace in namespaces(files) {
        let Some(dir) = resolve_directory(root, &namespace)? else {
            continue;
        };
        census_directory(root, &dir, &listed)?;
    }
    Ok(())
}

fn namespaces(files: &BTreeMap<String, ListedFile>) -> BTreeSet<String> {
    let mut names = BTreeSet::from(["share/solstone-journal".to_owned()]);
    for path in files.keys() {
        let Some(rest) = path.strip_prefix("lib/") else {
            continue;
        };
        let Some((dir, _)) = rest.split_once('/') else {
            continue;
        };
        if dir == "solstone_journal_models" || dir.starts_with("solstone-") {
            names.insert(format!("lib/{dir}"));
        }
    }
    names
}

fn in_namespace(path: &str, files: &BTreeMap<String, ListedFile>) -> bool {
    namespaces(files)
        .iter()
        .any(|namespace| path.starts_with(&format!("{namespace}/")))
}

fn census_directory(
    root: &Path,
    dir: &Path,
    listed: &BTreeSet<String>,
) -> Result<(), InstalledPayloadRefusal> {
    let mut entries = fs::read_dir(dir)
        .map_err(|error| unreadable(&relative_path(root, dir), error))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| unreadable(&relative_path(root, dir), error))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let relative = relative_path(root, &path);
        if relative == INSTALLED_PAYLOAD_MANIFEST || relative == INSTALLED_PAYLOAD_SIGNATURE {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|error| unreadable(&relative, error))?;
        if file_type.is_dir() {
            census_directory(root, &path, listed)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(unsafe_path(&relative));
        }
        if !listed.contains(&relative) {
            return Err(unexpected_file(&relative));
        }
    }
    Ok(())
}

fn check_listed_members(
    root: &Path,
    files: &BTreeMap<String, ListedFile>,
) -> Result<(), InstalledPayloadRefusal> {
    for (path, listed) in files {
        let namespaced = in_namespace(path, files);
        if !namespaced && !path.starts_with("bin/") {
            continue;
        }
        let full = resolve_member(root, path)?;
        if !namespaced {
            continue;
        }
        let bytes = read_member(&full, path)?;
        if is_native(&bytes) && !digest_matches(&bytes, listed.bytes, &listed.sha256) {
            return Err(member_changed(path));
        }
    }
    Ok(())
}

fn resolve_directory(
    root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, InstalledPayloadRefusal> {
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                let file_type = meta.file_type();
                if file_type.is_symlink() || !file_type.is_dir() {
                    return Err(unsafe_path(relative));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(relative, error)),
        }
    }
    Ok(Some(current))
}

fn resolve_member(root: &Path, relative: &str) -> Result<PathBuf, InstalledPayloadRefusal> {
    validate_relative(relative)?;
    let mut current = root.to_path_buf();
    let parts = relative.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let last = index + 1 == parts.len();
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                let file_type = meta.file_type();
                if file_type.is_symlink() {
                    return Err(unsafe_path(relative));
                }
                if last {
                    if !file_type.is_file() {
                        return Err(unsafe_path(relative));
                    }
                } else if !file_type.is_dir() {
                    return Err(unsafe_path(relative));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(member_missing(relative));
            }
            Err(error) => return Err(unreadable(relative, error)),
        }
    }
    Ok(current)
}

fn read_manifest_bytes(root: &Path) -> Result<Vec<u8>, InstalledPayloadRefusal> {
    let mut current = root.to_path_buf();
    let parts = INSTALLED_PAYLOAD_MANIFEST.split('/').collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        current.push(part);
        let last = index + 1 == parts.len();
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                let file_type = meta.file_type();
                if file_type.is_symlink() {
                    return Err(unsafe_path(INSTALLED_PAYLOAD_MANIFEST));
                }
                if last {
                    if !file_type.is_file() {
                        return Err(unsafe_path(INSTALLED_PAYLOAD_MANIFEST));
                    }
                } else if !file_type.is_dir() {
                    return Err(unsafe_path(INSTALLED_PAYLOAD_MANIFEST));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(manifest_missing(INSTALLED_PAYLOAD_MANIFEST, false));
            }
            Err(error) => return Err(unreadable(INSTALLED_PAYLOAD_MANIFEST, error)),
        }
    }
    match fs::read(&current) {
        Ok(bytes) => Ok(bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(manifest_missing(INSTALLED_PAYLOAD_MANIFEST, false))
        }
        Err(error) => Err(unreadable(INSTALLED_PAYLOAD_MANIFEST, error)),
    }
}

fn read_member(path: &Path, relative: &str) -> Result<Vec<u8>, InstalledPayloadRefusal> {
    let mut file = fs::File::open(path).map_err(|error| map_read_error(relative, error))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| map_read_error(relative, error))?;
    Ok(bytes)
}

fn map_read_error(relative: &str, error: io::Error) -> InstalledPayloadRefusal {
    if error.kind() == io::ErrorKind::NotFound {
        member_missing(relative)
    } else {
        unreadable(relative, error)
    }
}

fn digest_matches(bytes: &[u8], expected_len: u64, expected_sha: &str) -> bool {
    bytes.len() as u64 == expected_len && sha256_hex(bytes).eq_ignore_ascii_case(expected_sha)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn is_native(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x7fELF") || looks_like_macho(bytes)
}

fn looks_like_macho(bytes: &[u8]) -> bool {
    let Some(magic) = bytes.get(..4) else {
        return false;
    };
    let magics = [
        0xfeed_facfu32.to_le_bytes(),
        0xcffa_edfeu32.to_le_bytes(),
        0xcafe_babeu32.to_be_bytes(),
        0xcafe_babfu32.to_be_bytes(),
    ];
    magics.iter().any(|expected| expected.as_slice() == magic)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{self, Write};
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::{
        COMPILED_VERSION, ExecutablePlatform, INSTALLED_PAYLOAD_MANIFEST,
        INSTALLED_PAYLOAD_SIGNATURE, InstalledPackage, PRODUCT, TARGET_LINUX_AARCH64,
        TARGET_LINUX_X86_64, locate_installed_package, package_root_from_executable_path,
        render_installed_payload, verify_installed_package,
    };
    use crate::{code, guidance};

    fn scratch() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn write_file(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parents");
        fs::write(&path, bytes).expect("write");
    }

    fn seal(root: &Path, version: &str, target: &str) -> Vec<u8> {
        let bytes =
            render_installed_payload(root, PRODUCT, version, target, "aaa").expect("render");
        write_file(root, INSTALLED_PAYLOAD_MANIFEST, &bytes);
        bytes
    }

    fn admit(
        root: &Path,
        version: &str,
        target: &str,
    ) -> Result<InstalledPackage, super::InstalledPayloadRefusal> {
        InstalledPackage::admit(root, version, target)
    }

    fn elf(body: &[u8]) -> Vec<u8> {
        let mut bytes = b"\x7fELF".to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    fn macho(body: &[u8]) -> Vec<u8> {
        let mut bytes = 0xfeed_facfu32.to_le_bytes().to_vec();
        bytes.extend_from_slice(body);
        bytes
    }

    fn base_tree(root: &Path) {
        write_file(root, "bin/solstone", b"core-bin");
        write_file(root, "lib/solstone-demo/model.bin", b"model-bytes");
        write_file(root, "share/LICENSE", b"license-text");
    }

    #[test]
    fn render_is_deterministic_and_omits_reserved_names() {
        let tmp = scratch();
        let root = tmp.path();
        write_file(root, "bin/z", b"z");
        write_file(root, "bin/a", b"a");
        write_file(root, INSTALLED_PAYLOAD_MANIFEST, b"stale");
        write_file(root, INSTALLED_PAYLOAD_SIGNATURE, b"not-a-signature");
        let first =
            render_installed_payload(root, PRODUCT, COMPILED_VERSION, TARGET_LINUX_X86_64, "aaa")
                .expect("first");
        let second =
            render_installed_payload(root, PRODUCT, COMPILED_VERSION, TARGET_LINUX_X86_64, "aaa")
                .expect("second");
        assert_eq!(first, second);
        let text = std::str::from_utf8(&first).expect("utf8");
        assert!(text.starts_with("{\"schema\":\"solstone.installed-payload.v1\""));
        assert!(text.contains("{\"path\":\"bin/a\",\"bytes\":1,\"sha256\":"));
        assert!(!text.contains("installed-payload.json"));
        assert!(
            text.find("\"path\":\"bin/a\"").unwrap() < text.find("\"path\":\"bin/z\"").unwrap()
        );
    }

    #[cfg(unix)]
    #[test]
    fn render_refuses_symlink_and_fifo_and_case_collision() {
        let tmp = scratch();
        let root = tmp.path();
        write_file(root, "bin/solstone", b"core");
        std::os::unix::fs::symlink("solstone", root.join("bin/link")).expect("symlink");
        let error = render_installed_payload(root, PRODUCT, "1", TARGET_LINUX_X86_64, "aaa")
            .expect_err("symlink");
        assert_eq!(error.code, code::UNSAFE_PATH);
        assert_eq!(error.guidance, guidance::UNSAFE_PATH);
        fs::remove_file(root.join("bin/link")).expect("remove link");

        nix::unistd::mkfifo(
            &root.join("bin/pipe"),
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("fifo");
        let error = render_installed_payload(root, PRODUCT, "1", TARGET_LINUX_X86_64, "aaa")
            .expect_err("fifo");
        assert_eq!(error.code, code::UNSAFE_PATH);
        fs::remove_file(root.join("bin/pipe")).expect("remove fifo");

        // A case-insensitive volume stores one file for these two names, so
        // the collision is asserted where the two writes stay distinct.
        #[cfg(target_os = "linux")]
        {
            write_file(root, "share/Readme", b"A");
            write_file(root, "share/readme", b"B");
            let error = render_installed_payload(root, PRODUCT, "1", TARGET_LINUX_X86_64, "aaa")
                .expect_err("case");
            assert_eq!(error.code, code::UNSAFE_PATH);
        }
    }

    #[test]
    fn discovery_names_the_package_root_shapes() {
        let versioned = Path::new("/tmp/example/versions/2.0.37-linux-x86_64/bin/solstone");
        assert_eq!(
            package_root_from_executable_path(versioned, ExecutablePlatform::Linux)
                .expect("versioned"),
            PathBuf::from("/tmp/example/versions/2.0.37-linux-x86_64")
        );
        assert_eq!(
            package_root_from_executable_path(
                Path::new("/usr/bin/solstone"),
                ExecutablePlatform::Linux
            )
            .expect("usr"),
            PathBuf::from("/usr")
        );
        assert_eq!(
            package_root_from_executable_path(
                Path::new("/usr/bin/solstone (deleted)"),
                ExecutablePlatform::Linux
            )
            .expect("deleted path"),
            PathBuf::from("/usr")
        );
        assert_eq!(
            package_root_from_executable_path(
                Path::new("C:\\Program Files\\solstone\\bin\\solstone.exe"),
                ExecutablePlatform::Windows
            )
            .expect("windows"),
            PathBuf::from("C:\\Program Files\\solstone")
        );
        let unsupported = package_root_from_executable_path(
            Path::new("/tmp/target/debug/solstone"),
            ExecutablePlatform::Linux,
        )
        .expect_err("not bin");
        assert_eq!(unsupported.code, code::UNSUPPORTED_LOCATION);
        assert_eq!(unsupported.guidance, guidance::UNSUPPORTED_LOCATION);
    }

    #[cfg(unix)]
    #[test]
    fn macos_discovery_canonicalizes_a_journal_app_symlink() {
        let tmp = scratch();
        let runtime = tmp
            .path()
            .join("Journal.app/Contents/Resources/solstone-runtime");
        write_file(&runtime, "bin/solstone", b"app");
        let link = tmp.path().join("invoke");
        std::os::unix::fs::symlink(runtime.join("bin/solstone"), &link).expect("link");
        let root =
            package_root_from_executable_path(&link, ExecutablePlatform::Macos).expect("root");
        assert_eq!(root, runtime.canonicalize().expect("canonical runtime"));
    }

    #[test]
    fn macos_missing_path_is_manifest_missing_with_restart_guidance() {
        let missing = Path::new("/tmp/solstone-installed-payload-missing-canonical/bin/solstone");
        let error = package_root_from_executable_path(missing, ExecutablePlatform::Macos)
            .expect_err("missing");
        assert_eq!(error.code, code::MANIFEST_MISSING);
        assert_eq!(error.guidance, guidance::RESTART_UPDATE);
    }

    #[test]
    fn deleted_executable_without_a_manifest_uses_restart_guidance() {
        let tmp = scratch();
        let executable = tmp.path().join("bin").join("solstone (deleted)");
        let error =
            locate_installed_package(&executable, ExecutablePlatform::Linux).expect_err("missing");
        assert_eq!(error.code, code::MANIFEST_MISSING);
        assert_eq!(error.guidance, guidance::RESTART_UPDATE);
        assert!(error.path.as_deref().unwrap().ends_with(" (deleted)"));
    }

    #[test]
    fn discovery_does_not_walk_to_an_ancestor_or_a_checkout() {
        let tmp = scratch();
        let nested = tmp.path().join("a/b");
        write_file(&nested, "bin/exe", b"nested");
        base_tree(&tmp.path().join("a"));
        seal(&tmp.path().join("a"), COMPILED_VERSION, TARGET_LINUX_X86_64);
        write_file(tmp.path(), "pyproject.toml", b"[project]\n");
        write_file(tmp.path(), "solstone/talent/journal.md", b"talent");
        fs::create_dir_all(tmp.path().join(".git")).expect("git");
        let executable = nested.join("bin/exe");
        let error = locate_installed_package(&executable, ExecutablePlatform::Linux)
            .expect_err("no manifest");
        assert_eq!(error.code, code::MANIFEST_MISSING);
        assert_eq!(error.guidance, guidance::MANIFEST_MISSING);
    }

    #[test]
    fn current_executable_under_target_is_unsupported() {
        let executable = std::env::current_exe().expect("current exe");
        let error =
            package_root_from_executable_path(&executable, super::host_executable_platform())
                .expect_err("target");
        assert_eq!(error.code, code::UNSUPPORTED_LOCATION);
        assert_eq!(error.guidance, guidance::UNSUPPORTED_LOCATION);
    }

    #[test]
    fn header_order_checks_product_then_version_then_target_then_schema() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        let rendered = seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        let mut value: serde_json::Value = serde_json::from_slice(&rendered).expect("json");
        let newer = format!("{COMPILED_VERSION}.1");
        value["version"] = serde_json::Value::String(newer.clone());
        value["schema"] = serde_json::Value::String("solstone.installed-payload.future".into());
        value["note"] = serde_json::Value::Bool(true);
        write_file(
            root,
            INSTALLED_PAYLOAD_MANIFEST,
            &serde_json::to_vec(&value).expect("bytes"),
        );
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("newer");
        assert_eq!(error.code, code::RESTART_TO_FINISH_UPDATE);
        assert_eq!(error.guidance, guidance::RESTART_UPDATE);
        assert_eq!(
            error
                .versions
                .as_deref()
                .map(|(expected, found)| (expected.as_str(), found.as_str())),
            Some((COMPILED_VERSION, newer.as_str()))
        );

        value["version"] = serde_json::Value::String(COMPILED_VERSION.into());
        value["target"] = serde_json::Value::String(TARGET_LINUX_AARCH64.into());
        value.as_object_mut().expect("object").remove("note");
        value["schema"] = serde_json::Value::String(super::INSTALLED_PAYLOAD_SCHEMA.into());
        write_file(
            root,
            INSTALLED_PAYLOAD_MANIFEST,
            &serde_json::to_vec(&value).expect("bytes"),
        );
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("target");
        assert_eq!(error.code, code::WRONG_TARGET);
        assert_eq!(error.guidance, guidance::WRONG_TARGET);
        assert_eq!(
            error
                .targets
                .as_deref()
                .map(|(expected, found)| (expected.as_str(), found.as_str())),
            Some((TARGET_LINUX_X86_64, TARGET_LINUX_AARCH64))
        );

        value["product"] = serde_json::Value::String("other-product".into());
        value["version"] = serde_json::Value::String("0".into());
        value["target"] = serde_json::Value::String(TARGET_LINUX_X86_64.into());
        write_file(
            root,
            INSTALLED_PAYLOAD_MANIFEST,
            &serde_json::to_vec(&value).expect("bytes"),
        );
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("product");
        assert_eq!(error.code, code::WRONG_PRODUCT);
        assert_eq!(error.guidance, guidance::WRONG_PRODUCT);

        value["product"] = serde_json::Value::String(PRODUCT.into());
        value["version"] = serde_json::Value::String(COMPILED_VERSION.into());
        value["schema"] = serde_json::Value::String("solstone.installed-payload.future".into());
        write_file(
            root,
            INSTALLED_PAYLOAD_MANIFEST,
            &serde_json::to_vec(&value).expect("bytes"),
        );
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("schema");
        assert_eq!(error.code, code::MANIFEST_INVALID);
        assert_eq!(error.guidance, guidance::MANIFEST_INVALID);
    }

    #[test]
    fn admission_tolerates_installer_files_unrelated_trees_and_skips_generic_share() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        write_file(root, "lib/solstone_journal_models/keep.bin", b"weights");
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        write_file(root, ".release", b"release");
        write_file(root, ".archive-sha256", b"abc");
        write_file(root, ".install-transaction-1", b"txn");
        write_file(root, "share/doc/notes.txt", b"notes");
        write_file(root, "lib/solstone-other/stray.bin", b"stray");
        write_file(root, "bin/extra", b"extra");
        write_file(root, INSTALLED_PAYLOAD_SIGNATURE, b"reserved");
        fs::create_dir_all(root.join("lib/solstone-demo/empty")).expect("empty dir");
        let package = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("admit");
        package
            .member("lib/solstone-demo/model.bin")
            .expect("model");
        package.member("bin/solstone").expect("bin");
        package
            .member("lib/solstone_journal_models/keep.bin")
            .expect("weights");

        write_file(root, "share/LICENSE", b"rewritten-license-bytes");
        let package = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("license changed");
        package
            .member("lib/solstone-demo/model.bin")
            .expect("model still");
        package.member("bin/solstone").expect("bin still");
        fs::remove_file(root.join("share/LICENSE")).expect("remove license");
        let package = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("license missing");
        package
            .member("lib/solstone-demo/model.bin")
            .expect("model after");
        package.member("bin/solstone").expect("bin after");
    }

    #[test]
    fn census_refuses_unlisted_namespace_files_and_ignores_unlisted_bin() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        write_file(root, "lib/solstone_journal_models/keep.bin", b"weights");
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        for relative in [
            "lib/solstone-demo/extra.bin",
            "lib/solstone_journal_models/extra.bin",
            "share/solstone-journal/.DS_Store",
        ] {
            write_file(root, relative, b"extra");
            let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err(relative);
            assert_eq!(error.code, code::UNEXPECTED_FILE, "{relative}");
            assert_eq!(error.guidance, guidance::PACKAGE_MISMATCH);
            assert_eq!(error.path.as_deref(), Some(relative));
            fs::remove_file(root.join(relative)).expect("cleanup");
        }
    }

    #[test]
    fn admission_hashes_native_namespace_members_before_a_request() {
        let tmp = scratch();
        let root = tmp.path();
        write_file(root, "bin/solstone", b"core-bin");
        let natives = [
            ("lib/solstone-demo/libdemo.so", elf(b"-so-body!!")),
            ("lib/solstone-demo/libdemo.dylib", macho(b"-dylib-body")),
            ("lib/solstone-demo/engine", elf(b"-plain-body")),
        ];
        for (relative, bytes) in &natives {
            write_file(root, relative, bytes);
        }
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        for (relative, bytes) in natives {
            let mut flipped = bytes.clone();
            *flipped.last_mut().expect("byte") ^= 0xff;
            write_file(root, relative, &flipped);
            let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err(relative);
            assert_eq!(error.code, code::MEMBER_CHANGED, "{relative}");
            assert_eq!(error.guidance, guidance::PACKAGE_MISMATCH);
            assert_eq!(error.path.as_deref(), Some(relative));
            write_file(root, relative, &bytes);
        }
        admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("restored");
    }

    #[cfg(unix)]
    #[test]
    fn census_refuses_unlisted_symlink_and_fifo_and_admits_an_empty_directory() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        fs::create_dir_all(root.join("lib/solstone-demo/empty")).expect("empty");
        admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("empty dir");
        std::os::unix::fs::symlink("model.bin", root.join("lib/solstone-demo/alias"))
            .expect("link");
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("symlink");
        assert_eq!(error.code, code::UNSAFE_PATH);
        assert_eq!(error.path.as_deref(), Some("lib/solstone-demo/alias"));
        fs::remove_file(root.join("lib/solstone-demo/alias")).expect("unlink");
        nix::unistd::mkfifo(
            &root.join("lib/solstone-demo/pipe"),
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("fifo");
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("fifo");
        assert_eq!(error.code, code::UNSAFE_PATH);
        assert_eq!(error.path.as_deref(), Some("lib/solstone-demo/pipe"));
    }

    #[cfg(unix)]
    #[test]
    fn a_mode_000_member_is_unreadable_rather_than_missing() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        let locked = root.join("lib/solstone-demo/model.bin");
        let mut permissions = fs::metadata(&locked).expect("meta").permissions();
        permissions.set_mode(0o0);
        fs::set_permissions(&locked, permissions).expect("mode");
        let result = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        if nix::unistd::Uid::effective().is_root() {
            if let Err(error) = &result {
                assert_eq!(error.code, code::MEMBER_UNREADABLE);
                assert!(error.io.is_some());
                assert_ne!(error.code, code::MEMBER_MISSING);
            }
        } else {
            let error = result.expect_err("denied");
            assert_eq!(error.code, code::MEMBER_UNREADABLE);
            assert_eq!(error.guidance, guidance::MEMBER_UNREADABLE);
            assert_eq!(
                error.io.expect("os error").kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }

    #[test]
    fn upgrade_stops_at_the_version_before_any_member_reason() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        write_file(root, "lib/solstone-demo/gone.bin", b"gone");
        let rendered = seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        let mut value: serde_json::Value = serde_json::from_slice(&rendered).expect("json");
        let newer = format!("{COMPILED_VERSION}.1");
        value["version"] = serde_json::Value::String(newer.clone());
        value["schema"] = serde_json::Value::String("nope".into());
        value["extra"] = serde_json::Value::String("field".into());
        write_file(
            root,
            INSTALLED_PAYLOAD_MANIFEST,
            &serde_json::to_vec(&value).expect("bytes"),
        );
        write_file(root, "lib/solstone-demo/model.bin", b"different");
        fs::remove_file(root.join("lib/solstone-demo/gone.bin")).expect("remove");
        write_file(root, "lib/solstone-demo/added.bin", b"added");
        write_file(root, "lib/solstone-demo/model.bin.dpkg-new", b"partial");
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("upgrade");
        assert_eq!(error.code, code::RESTART_TO_FINISH_UPDATE);
        assert_eq!(error.guidance, guidance::RESTART_UPDATE);
        assert_eq!(
            error
                .versions
                .as_deref()
                .map(|(expected, found)| (expected.as_str(), found.as_str())),
            Some((COMPILED_VERSION, newer.as_str()))
        );
    }

    #[test]
    fn mixed_same_version_state_uses_the_restart_or_reinstall_guidance() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        write_file(root, "lib/solstone-demo/model.bin", b"new-bytes");
        write_file(root, "lib/solstone-demo/temp.bin", b"temp");
        let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("mixed");
        assert!(
            error.code == code::MEMBER_CHANGED || error.code == code::UNEXPECTED_FILE,
            "{}",
            error.code
        );
        assert_eq!(error.guidance, guidance::PACKAGE_MISMATCH);
    }

    #[test]
    fn use_time_checks_see_a_change_a_deletion_and_a_second_operation() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        let operation = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("admit");
        operation
            .member("lib/solstone-demo/model.bin")
            .expect("first");
        write_file(root, "lib/solstone-demo/model.bin", b"mutated-model");
        let error = operation
            .member("lib/solstone-demo/model.bin")
            .expect_err("changed");
        assert_eq!(error.code, code::MEMBER_CHANGED);
        assert_eq!(error.guidance, guidance::PACKAGE_MISMATCH);
        fs::remove_file(root.join("lib/solstone-demo/model.bin")).expect("delete");
        let error = operation
            .member("lib/solstone-demo/model.bin")
            .expect_err("deleted");
        assert_eq!(error.code, code::MEMBER_MISSING);
        assert_eq!(error.path.as_deref(), Some("lib/solstone-demo/model.bin"));

        write_file(root, "lib/solstone-demo/model.bin", b"model-bytes");
        let first = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("first op");
        first.member("lib/solstone-demo/model.bin").expect("hash");
        write_file(root, "lib/solstone-demo/model.bin", b"after-hash");
        let second = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("second op");
        let error = second
            .member("lib/solstone-demo/model.bin")
            .expect_err("second");
        assert_eq!(error.code, code::MEMBER_CHANGED);
        let _keep_first = first;

        write_file(root, "lib/solstone-demo/model.bin", b"model-bytes");
        let held = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("held");
        held.member("lib/solstone-demo/model.bin")
            .expect("held hash");
        write_file(root, "lib/solstone-demo/model.bin", b"thread-mutated");
        let root_buf = root.to_path_buf();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let other =
                    InstalledPackage::admit(&root_buf, COMPILED_VERSION, TARGET_LINUX_X86_64)
                        .expect("other");
                let error = other
                    .member("lib/solstone-demo/model.bin")
                    .expect_err("thread");
                assert_eq!(error.code, code::MEMBER_CHANGED);
            });
        });
        let _keep_held = held;
    }

    #[test]
    fn unsafe_manifest_paths_refuse() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        for files in [
            serde_json::json!([{"path":"../secret","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
            serde_json::json!([{"path":"/etc/passwd","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]),
            serde_json::json!([
                {"path":"bin/a","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                {"path":"bin/a","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            ]),
            serde_json::json!([
                {"path":"share/Readme","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                {"path":"share/readme","bytes":1,"sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
            ]),
            serde_json::json!([
                {"path":"bin/b","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
                {"path":"bin/a","bytes":1,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}
            ]),
        ] {
            let document = serde_json::json!({
                "schema": super::INSTALLED_PAYLOAD_SCHEMA,
                "product": PRODUCT,
                "version": COMPILED_VERSION,
                "target": TARGET_LINUX_X86_64,
                "source_commit": "aaa",
                "files": files,
            });
            write_file(
                root,
                INSTALLED_PAYLOAD_MANIFEST,
                &serde_json::to_vec(&document).expect("json"),
            );
            let error = admit(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("path");
            assert_eq!(error.code, code::UNSAFE_PATH);
            assert_eq!(error.guidance, guidance::UNSAFE_PATH);
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_between_root_and_member_refuses_and_an_ancestor_symlink_admits() {
        let tmp = scratch();
        let real = tmp.path().join("real");
        base_tree(&real);
        seal(&real, COMPILED_VERSION, TARGET_LINUX_X86_64);
        let link_parent = tmp.path().join("link-parent");
        std::os::unix::fs::symlink(&real, &link_parent).expect("ancestor");
        admit(&link_parent, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("ancestor admits");
        verify_installed_package(&link_parent, COMPILED_VERSION, TARGET_LINUX_X86_64)
            .expect("full");

        let moved = tmp.path().join("moved-lib");
        fs::rename(real.join("lib"), &moved).expect("move lib");
        std::os::unix::fs::symlink(&moved, real.join("lib")).expect("lib link");
        let error = admit(&real, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("lib symlink");
        assert_eq!(error.code, code::UNSAFE_PATH);

        fs::remove_file(real.join("lib")).expect("remove lib link");
        fs::rename(&moved, real.join("lib")).expect("restore lib");
        let file = real.join("lib/solstone-demo/model.bin");
        let parked = tmp.path().join("parked-model");
        fs::rename(&file, &parked).expect("park");
        std::os::unix::fs::symlink(&parked, &file).expect("member link");
        let error =
            admit(&real, COMPILED_VERSION, TARGET_LINUX_X86_64).expect_err("member symlink");
        assert_eq!(error.code, code::UNSAFE_PATH);
        assert_eq!(error.path.as_deref(), Some("lib/solstone-demo/model.bin"));
    }

    #[test]
    fn full_verification_hashes_every_listed_member() {
        let tmp = scratch();
        let root = tmp.path();
        base_tree(root);
        seal(root, COMPILED_VERSION, TARGET_LINUX_X86_64);
        verify_installed_package(root, COMPILED_VERSION, TARGET_LINUX_X86_64).expect("full");
        let mut license = fs::OpenOptions::new()
            .write(true)
            .open(root.join("share/LICENSE"))
            .expect("open");
        license.write_all(b"license-text!").expect("grow");
        let error = verify_installed_package(root, COMPILED_VERSION, TARGET_LINUX_X86_64)
            .expect_err("generic share");
        assert_eq!(error.code, code::MEMBER_CHANGED);
        assert_eq!(error.path.as_deref(), Some("share/LICENSE"));
    }
}
