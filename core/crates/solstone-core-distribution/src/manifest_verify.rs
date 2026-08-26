// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
#[cfg(feature = "test-fixture-pin")]
use std::sync::OnceLock;

use minisign::{PublicKey, PublicKeyBox, SignatureBox};
use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::digest::sha256_hex;
use crate::inspect;

pub const PRODUCT_PIN: &str =
    include_str!("../../../../packaging/keys/solstone-journal-release.pub");
#[cfg(feature = "test-fixture-pin")]
const PIN_OVERRIDE_ENV: &str = "SOLSTONE_JOURNAL_MINISIGN_PIN";
#[cfg(feature = "test-fixture-pin")]
static FIXTURE_PIN: OnceLock<String> = OnceLock::new();

const MANIFEST_SUFFIX: &str = ".manifest.json";
const MINISIG_SUFFIX: &str = ".minisig";
const RELEASE_KEYS: &[&str] = &["product", "version", "target", "commit", "lock_sha256"];
const ARCHIVE_CHAIN_RELEASE_KEYS: &[&str] = &[
    "archive_prebuild_input_sha256",
    "archive_delivery_contract_sha256",
    "archive_final_invocation_sha256",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestVerifyRefusal {
    MissingManifest,
    AmbiguousManifest,
    UnparseableManifest,
    ManifestSymlink,
    MissingSignature,
    AmbiguousSignature,
    UnparseableSignature,
    SignatureSymlink,
    SignaturePinMismatch,
    MemberAbsent,
    MemberDigestMismatch,
    DuplicateMember,
    UnsafeMemberName,
    ListedMemberSymlink,
    ListedMemberNotRegular,
    ChecksumSidecarMismatch,
    UnexpectedTopLevelRegular,
    UnexpectedTopLevelSymlink,
    UnexpectedTopLevelOther,
    ReleaseDeclarationMismatch,
    ReleaseBasenameMismatch,
}

impl ManifestVerifyRefusal {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingManifest => "missing-manifest",
            Self::AmbiguousManifest => "ambiguous-manifest",
            Self::UnparseableManifest => "unparseable-manifest",
            Self::ManifestSymlink => "manifest-symlink",
            Self::MissingSignature => "missing-signature",
            Self::AmbiguousSignature => "ambiguous-signature",
            Self::UnparseableSignature => "unparseable-signature",
            Self::SignatureSymlink => "signature-symlink",
            Self::SignaturePinMismatch => "signature-pin-mismatch",
            Self::MemberAbsent => "member-absent",
            Self::MemberDigestMismatch => "member-digest-mismatch",
            Self::DuplicateMember => "duplicate-member",
            Self::UnsafeMemberName => "unsafe-member-name",
            Self::ListedMemberSymlink => "listed-member-symlink",
            Self::ListedMemberNotRegular => "listed-member-not-regular",
            Self::ChecksumSidecarMismatch => "checksum-sidecar-mismatch",
            Self::UnexpectedTopLevelRegular => "unexpected-top-level-regular",
            Self::UnexpectedTopLevelSymlink => "unexpected-top-level-symlink",
            Self::UnexpectedTopLevelOther => "unexpected-top-level-other",
            Self::ReleaseDeclarationMismatch => "release-declaration-mismatch",
            Self::ReleaseBasenameMismatch => "release-basename-mismatch",
        }
    }
}

#[derive(Debug)]
pub struct ManifestVerifyError {
    pub kind: ManifestVerifyRefusal,
    pub detail: String,
}

impl ManifestVerifyError {
    fn new(kind: ManifestVerifyRefusal, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for ManifestVerifyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}\n  {}", self.kind.as_str(), self.detail)
    }
}

impl std::error::Error for ManifestVerifyError {}

#[derive(Debug, Clone)]
pub struct ManifestSource {
    pub path: PathBuf,
    pub name: String,
    pub basename: String,
    pub bytes: Vec<u8>,
    manifest: Manifest,
}

#[derive(Debug, Clone)]
pub struct SignatureSource {
    pub path: PathBuf,
    pub name: String,
    pub bytes: Vec<u8>,
}

impl SignatureSource {
    #[must_use]
    pub fn captured(path: PathBuf, name: String, bytes: Vec<u8>) -> Self {
        Self { path, name, bytes }
    }
}

#[derive(Debug, Clone)]
pub struct ValidatedReleaseSet {
    pub product: String,
    pub version: String,
    pub target: String,
    pub basename: String,
    pub manifest_name: String,
    pub manifest_bytes: Vec<u8>,
    pub signature_name: Option<String>,
    pub signature_bytes: Option<Vec<u8>>,
    /// Every declared manifest member, captured once while its digest is checked.
    pub members: BTreeMap<String, Vec<u8>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    product: String,
    version: String,
    target: String,
    #[serde(deserialize_with = "deserialize_files")]
    files: BTreeMap<String, String>,
}

fn deserialize_files<'de, D>(deserializer: D) -> Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    struct FilesVisitor;

    impl<'de> Visitor<'de> for FilesVisitor {
        type Value = BTreeMap<String, String>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a manifest files object with unique names")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut files = BTreeMap::new();
            while let Some((name, digest)) = map.next_entry::<String, String>()? {
                if files.insert(name.clone(), digest).is_some() {
                    return Err(de::Error::custom(format!("duplicate-member: {name}")));
                }
            }
            Ok(files)
        }
    }

    deserializer.deserialize_map(FilesVisitor)
}

pub fn discover_manifest(dir: &Path) -> Result<ManifestSource, ManifestVerifyError> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::MissingManifest,
            format!("could not read {}: {error}", dir.display()),
        )
    })? {
        let entry = entry.map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::MissingManifest,
                format!("could not read {}: {error}", dir.display()),
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(MANIFEST_SUFFIX) {
            candidates.push((name, entry.path()));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    let [(name, path)] = candidates.as_slice() else {
        return if candidates.is_empty() {
            Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::MissingManifest,
                format!("no *{MANIFEST_SUFFIX} in {}", dir.display()),
            ))
        } else {
            Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::AmbiguousManifest,
                candidates
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            ))
        };
    };
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::MissingManifest,
            format!("{}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::ManifestSymlink,
            name.clone(),
        ));
    }
    if !metadata.file_type().is_file() {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableManifest,
            format!("{} is not a regular file", name),
        ));
    }
    let bytes = fs::read(path).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableManifest,
            format!("{}: {error}", path.display()),
        )
    })?;
    let manifest = parse_manifest(&bytes)?;
    let basename = name
        .strip_suffix(MANIFEST_SUFFIX)
        .expect("candidate has manifest suffix")
        .to_owned();
    let expected_basename = format!(
        "{}-{}-{}",
        manifest.product, manifest.version, manifest.target
    );
    if basename != expected_basename {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::ReleaseBasenameMismatch,
            format!("{basename} != {expected_basename}"),
        ));
    }
    Ok(ManifestSource {
        path: path.clone(),
        name: name.clone(),
        basename,
        bytes,
        manifest,
    })
}

pub fn capture_signature(
    dir: &Path,
    manifest: &ManifestSource,
    required: bool,
) -> Result<Option<SignatureSource>, ManifestVerifyError> {
    let expected_name = format!("{}{}", manifest.name, MINISIG_SUFFIX);
    let mut candidates = Vec::new();
    for entry in fs::read_dir(dir).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::MissingSignature,
            format!("could not read {}: {error}", dir.display()),
        )
    })? {
        let entry = entry.map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::MissingSignature,
                format!("could not read {}: {error}", dir.display()),
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(&format!("{MANIFEST_SUFFIX}{MINISIG_SUFFIX}")) {
            candidates.push((name, entry.path()));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));
    if candidates.len() > 1 {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::AmbiguousSignature,
            candidates
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    let Some((name, path)) = candidates.pop() else {
        return if required {
            Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::MissingSignature,
                expected_name,
            ))
        } else {
            Ok(None)
        };
    };
    if name != expected_name {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::MissingSignature,
            expected_name,
        ));
    }
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::MissingSignature,
            format!("{}: {error}", path.display()),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::SignatureSymlink,
            name,
        ));
    }
    if !metadata.file_type().is_file() {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableSignature,
            format!("{name} is not a regular file"),
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableSignature,
            format!("{}: {error}", path.display()),
        )
    })?;
    Ok(Some(SignatureSource { path, name, bytes }))
}

pub fn verify_manifest_signature(
    manifest: &ManifestSource,
    signature: &SignatureSource,
) -> Result<(), ManifestVerifyError> {
    let signature_text = std::str::from_utf8(&signature.bytes).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableSignature,
            format!("{}: {error}", signature.path.display()),
        )
    })?;
    let signature_box = SignatureBox::from_string(signature_text).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::UnparseableSignature,
            format!("{}: {error}", signature.path.display()),
        )
    })?;
    let pin = resolve_pin()?;
    minisign::verify(
        &pin,
        &signature_box,
        Cursor::new(manifest.bytes.as_slice()),
        true,
        false,
        false,
    )
    .map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::SignaturePinMismatch,
            format!("{}: {error}", signature.path.display()),
        )
    })
}

pub fn validate_release_set(
    dir: &Path,
    manifest: &ManifestSource,
    signature: Option<&SignatureSource>,
) -> Result<ValidatedReleaseSet, ManifestVerifyError> {
    validate_manifest_members(manifest, signature)?;

    let mut expected = BTreeSet::from([manifest.name.clone()]);
    if let Some(signature) = signature {
        expected.insert(signature.name.clone());
    }
    expected.extend(manifest.manifest.files.keys().cloned());

    let mut members = BTreeMap::new();
    for entry in fs::read_dir(dir).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::MemberAbsent,
            format!("could not read {}: {error}", dir.display()),
        )
    })? {
        let entry = entry.map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::MemberAbsent,
                format!("could not read {}: {error}", dir.display()),
            )
        })?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::MemberAbsent,
                format!("{name}: {error}"),
            )
        })?;
        if !expected.contains(&name) {
            let kind = if metadata.file_type().is_symlink() {
                ManifestVerifyRefusal::UnexpectedTopLevelSymlink
            } else if metadata.file_type().is_file() {
                ManifestVerifyRefusal::UnexpectedTopLevelRegular
            } else {
                ManifestVerifyRefusal::UnexpectedTopLevelOther
            };
            return Err(ManifestVerifyError::new(kind, name));
        }
        if name == manifest.name {
            if metadata.file_type().is_symlink() {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::ManifestSymlink,
                    name,
                ));
            }
            if !metadata.file_type().is_file() {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::UnparseableManifest,
                    name,
                ));
            }
            let actual = fs::read(&path).map_err(|error| {
                ManifestVerifyError::new(
                    ManifestVerifyRefusal::UnparseableManifest,
                    format!("{name}: {error}"),
                )
            })?;
            if actual != manifest.bytes {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::UnparseableManifest,
                    name,
                ));
            }
            continue;
        }
        if signature.is_some_and(|signature| name == signature.name) {
            let signature = signature.expect("is_some_and confirmed");
            if metadata.file_type().is_symlink() {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::SignatureSymlink,
                    name,
                ));
            }
            if !metadata.file_type().is_file() {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::UnparseableSignature,
                    name,
                ));
            }
            let actual = fs::read(&path).map_err(|error| {
                ManifestVerifyError::new(
                    ManifestVerifyRefusal::UnparseableSignature,
                    format!("{name}: {error}"),
                )
            })?;
            if actual != signature.bytes {
                return Err(ManifestVerifyError::new(
                    ManifestVerifyRefusal::SignaturePinMismatch,
                    name,
                ));
            }
            continue;
        }
        if metadata.file_type().is_symlink() {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::ListedMemberSymlink,
                name,
            ));
        }
        if !metadata.file_type().is_file() {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::ListedMemberNotRegular,
                name,
            ));
        }
        let bytes = fs::read(&path).map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::MemberAbsent,
                format!("{name}: {error}"),
            )
        })?;
        let expected_digest = manifest
            .manifest
            .files
            .get(&name)
            .expect("declared member was expected");
        if sha256_hex(&bytes) != *expected_digest {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::MemberDigestMismatch,
                name,
            ));
        }
        members.insert(name, bytes);
    }
    for name in manifest.manifest.files.keys() {
        if !members.contains_key(name) {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::MemberAbsent,
                name.clone(),
            ));
        }
    }

    validate_checksum_sidecar(manifest, &members)?;
    validate_release_declaration(manifest, &members)?;

    Ok(ValidatedReleaseSet {
        product: manifest.manifest.product.clone(),
        version: manifest.manifest.version.clone(),
        target: manifest.manifest.target.clone(),
        basename: manifest.basename.clone(),
        manifest_name: manifest.name.clone(),
        manifest_bytes: manifest.bytes.clone(),
        signature_name: signature.map(|signature| signature.name.clone()),
        signature_bytes: signature.map(|signature| signature.bytes.clone()),
        members,
    })
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, ManifestVerifyError> {
    serde_json::from_slice(bytes).map_err(|error| {
        let kind = if error.to_string().contains("duplicate-member:") {
            ManifestVerifyRefusal::DuplicateMember
        } else {
            ManifestVerifyRefusal::UnparseableManifest
        };
        ManifestVerifyError::new(kind, error.to_string())
    })
}

fn validate_manifest_members(
    manifest: &ManifestSource,
    signature: Option<&SignatureSource>,
) -> Result<(), ManifestVerifyError> {
    let checksum_name = format!("{}.sha256", manifest.basename);
    for (name, digest) in &manifest.manifest.files {
        if !is_safe_name(name)
            || name == &manifest.name
            || signature.is_some_and(|signature| name == &signature.name)
        {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::UnsafeMemberName,
                name.clone(),
            ));
        }
        if !is_sha256(digest) {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::UnparseableManifest,
                format!("invalid digest for {name}"),
            ));
        }
    }
    if !manifest.manifest.files.contains_key(&checksum_name) {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::MemberAbsent,
            checksum_name,
        ));
    }
    Ok(())
}

fn validate_checksum_sidecar(
    manifest: &ManifestSource,
    members: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ManifestVerifyError> {
    let checksum_name = format!("{}.sha256", manifest.basename);
    let bytes = members
        .get(&checksum_name)
        .expect("checksum member required");
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::ChecksumSidecarMismatch,
            checksum_name.clone(),
        )
    })?;
    let mut listed = BTreeMap::new();
    for line in text.lines() {
        let Some((digest, name)) = line.split_once("  ") else {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::ChecksumSidecarMismatch,
                checksum_name,
            ));
        };
        if !is_safe_name(name)
            || !is_sha256(digest)
            || listed.insert(name.to_owned(), digest.to_owned()).is_some()
        {
            return Err(ManifestVerifyError::new(
                ManifestVerifyRefusal::ChecksumSidecarMismatch,
                checksum_name,
            ));
        }
    }
    let expected = manifest
        .manifest
        .files
        .iter()
        .filter(|(name, _)| *name != &checksum_name)
        .map(|(name, digest)| (name.clone(), digest.clone()))
        .collect::<BTreeMap<_, _>>();
    if listed != expected {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::ChecksumSidecarMismatch,
            checksum_name,
        ));
    }
    Ok(())
}

fn validate_release_declaration(
    manifest: &ManifestSource,
    members: &BTreeMap<String, Vec<u8>>,
) -> Result<(), ManifestVerifyError> {
    let release_name = format!("{}.release", manifest.basename);
    let bytes = members.get(&release_name).ok_or_else(|| {
        ManifestVerifyError::new(ManifestVerifyRefusal::MemberAbsent, release_name.clone())
    })?;
    let text = std::str::from_utf8(bytes).map_err(|_| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::ReleaseDeclarationMismatch,
            release_name.clone(),
        )
    })?;
    let pairs = inspect::parse_release(text).map_err(|_| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::ReleaseDeclarationMismatch,
            release_name.clone(),
        )
    })?;
    let pair_count = pairs.len();
    let fields = pairs.into_iter().collect::<BTreeMap<_, _>>();
    let macos_target = manifest.manifest.target.starts_with("macos");
    let mut expected_keys = RELEASE_KEYS.iter().copied().collect::<BTreeSet<_>>();
    if macos_target {
        expected_keys.extend(ARCHIVE_CHAIN_RELEASE_KEYS.iter().copied());
    }
    if pair_count != expected_keys.len()
        || fields.len() != expected_keys.len()
        || fields.keys().map(String::as_str).collect::<BTreeSet<_>>() != expected_keys
        || fields.get("product") != Some(&manifest.manifest.product)
        || fields.get("version") != Some(&manifest.manifest.version)
        || fields.get("target") != Some(&manifest.manifest.target)
        || (macos_target
            && ARCHIVE_CHAIN_RELEASE_KEYS
                .iter()
                .any(|key| fields.get(*key).is_none_or(|value| !is_sha256(value))))
    {
        return Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::ReleaseDeclarationMismatch,
            release_name,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod release_contract_tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::{
        ARCHIVE_CHAIN_RELEASE_KEYS, Manifest, ManifestSource, ManifestVerifyRefusal,
        validate_release_declaration,
    };

    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn manifest(target: &str) -> ManifestSource {
        ManifestSource {
            path: PathBuf::from("fixture.manifest.json"),
            name: "fixture.manifest.json".to_owned(),
            basename: "fixture".to_owned(),
            bytes: Vec::new(),
            manifest: Manifest {
                product: "solstone-journal".to_owned(),
                version: "1.2.3".to_owned(),
                target: target.to_owned(),
                files: BTreeMap::new(),
            },
        }
    }

    fn validate(target: &str, release: String) -> Result<(), ManifestVerifyRefusal> {
        let source = manifest(target);
        let members = BTreeMap::from([("fixture.release".to_owned(), release.into_bytes())]);
        validate_release_declaration(&source, &members).map_err(|error| error.kind)
    }

    fn release(target: &str) -> String {
        format!(
            "product=solstone-journal\nversion=1.2.3\ntarget={target}\ncommit=commit\nlock_sha256=lock\n"
        )
    }

    fn macos_release() -> String {
        let mut release = release("macos-arm64");
        for key in ARCHIVE_CHAIN_RELEASE_KEYS {
            release.push_str(&format!("{key}={DIGEST}\n"));
        }
        release
    }

    #[test]
    fn macos_release_requires_all_archive_chain_digests() {
        assert!(validate("macos-arm64", macos_release()).is_ok());
        for missing in ARCHIVE_CHAIN_RELEASE_KEYS {
            let release = macos_release()
                .lines()
                .filter(|line| !line.starts_with(missing))
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                validate("macos-arm64", format!("{release}\n")),
                Err(ManifestVerifyRefusal::ReleaseDeclarationMismatch),
                "missing {missing}"
            );
        }
    }

    #[test]
    fn macos_release_refuses_an_invalid_archive_chain_digest() {
        let release = macos_release().replace(
            &format!("archive_delivery_contract_sha256={DIGEST}"),
            "archive_delivery_contract_sha256=invalid",
        );
        assert_eq!(
            validate("macos-arm64", release),
            Err(ManifestVerifyRefusal::ReleaseDeclarationMismatch)
        );
    }

    #[test]
    fn linux_release_keeps_its_five_key_contract() {
        assert!(validate("linux-x86_64", release("linux-x86_64")).is_ok());
        for extra in ARCHIVE_CHAIN_RELEASE_KEYS {
            let mut release = release("linux-x86_64");
            release.push_str(&format!("{extra}={DIGEST}\n"));
            assert_eq!(
                validate("linux-x86_64", release),
                Err(ManifestVerifyRefusal::ReleaseDeclarationMismatch),
                "extra {extra}"
            );
        }
    }
}

fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && name != "." && name != ".."
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn parse_pin(text: &str) -> Result<PublicKey, ManifestVerifyError> {
    let boxed = PublicKeyBox::from_string(text).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::SignaturePinMismatch,
            format!("could not parse public pin: {error}"),
        )
    })?;
    PublicKey::from_box(boxed).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::SignaturePinMismatch,
            format!("could not parse public pin: {error}"),
        )
    })
}

#[cfg(not(feature = "test-fixture-pin"))]
pub fn resolve_pin() -> Result<PublicKey, ManifestVerifyError> {
    parse_pin(PRODUCT_PIN)
}

#[cfg(feature = "test-fixture-pin")]
pub fn resolve_pin() -> Result<PublicKey, ManifestVerifyError> {
    let text = match std::env::var(PIN_OVERRIDE_ENV) {
        Ok(path) if !path.is_empty() => fs::read_to_string(&path).map_err(|error| {
            ManifestVerifyError::new(
                ManifestVerifyRefusal::SignaturePinMismatch,
                format!("could not read {PIN_OVERRIDE_ENV} {path}: {error}"),
            )
        })?,
        _ => FIXTURE_PIN
            .get()
            .cloned()
            .unwrap_or_else(|| PRODUCT_PIN.to_owned()),
    };
    parse_pin(&text)
}

#[cfg(feature = "test-fixture-pin")]
pub fn install_test_fixture_pin(path: &Path) -> Result<(), ManifestVerifyError> {
    let text = fs::read_to_string(path).map_err(|error| {
        ManifestVerifyError::new(
            ManifestVerifyRefusal::SignaturePinMismatch,
            format!("could not read fixture pin {}: {error}", path.display()),
        )
    })?;
    match FIXTURE_PIN.get() {
        Some(existing) if existing != &text => Err(ManifestVerifyError::new(
            ManifestVerifyRefusal::SignaturePinMismatch,
            "fixture pin was already installed with different bytes",
        )),
        Some(_) => Ok(()),
        None => {
            let _ = FIXTURE_PIN.set(text);
            Ok(())
        }
    }
}

#[cfg(test)]
mod pin_source {
    const SOURCE: &str = include_str!("manifest_verify.rs");
    const OVERRIDE: &str = concat!("SOLSTONE_JOURNAL_MINISIGN", "_PIN");
    const FEATURE_ON: &str = "#[cfg(feature = \"test-fixture-pin\")]";
    const FEATURE_OFF: &str = "#[cfg(not(feature = \"test-fixture-pin\"))]";

    #[test]
    fn pin_override_env_lives_only_in_the_fixture_pin_feature_sibling() {
        assert_eq!(SOURCE.matches(OVERRIDE).count(), 1);
        let mut off_body = String::new();
        let mut in_off = false;
        for line in SOURCE.lines() {
            let trimmed = line.trim();
            if trimmed == FEATURE_OFF
                || trimmed.starts_with("#[cfg(all(test, not(feature = \"test-fixture-pin\")))]")
            {
                in_off = true;
                continue;
            }
            if trimmed == FEATURE_ON {
                in_off = false;
                continue;
            }
            if in_off {
                off_body.push_str(line);
                off_body.push('\n');
            }
        }
        assert!(!off_body.contains(OVERRIDE));
    }
}

#[cfg(all(test, not(feature = "test-fixture-pin")))]
mod tests {
    use super::{PRODUCT_PIN, parse_pin, resolve_pin};

    #[test]
    fn default_resolve_pin_returns_the_compiled_product_pin() {
        let resolved = resolve_pin().expect("compiled pin parses");
        let expected = parse_pin(PRODUCT_PIN).expect("product pin parses");
        assert_eq!(resolved, expected);
    }
}
