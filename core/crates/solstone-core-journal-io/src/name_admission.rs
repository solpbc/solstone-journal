// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Portable stream/segment name admission and case-insensitive collision scan.

use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::errors::PathEscapeError;

/// Why a candidate name is not admissible.
///
/// Variant order is evaluation order. First match wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameAdmissionReason {
    /// The name is not valid UTF-8 text.
    NotUtf8,
    /// The name is empty.
    Empty,
    /// `.` and `..` are not valid names.
    DotComponent,
    /// The name must be a single relative component.
    RootOrPrefix,
    /// The name contains a path separator.
    Separator,
    /// The name contains a control character.
    Control,
    /// The name contains a colon.
    AlternateDataStream,
    /// The name contains a character Windows does not allow.
    ForbiddenCharacter,
    /// The name is longer than 255 UTF-8 bytes.
    TooLong,
    /// The name is reserved by Windows.
    ReservedDevice,
    /// The name ends in a dot or space.
    TrailingDotOrSpace,
    /// Stream names must start with a lowercase ASCII letter or digit and then
    /// use only lowercase ASCII letters, digits, dots, underscores, or hyphens.
    StreamGrammar,
    /// Claim names must use the reserved operation-claim grammar.
    ClaimGrammar,
}

impl fmt::Display for NameAdmissionReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotUtf8 => "the name is not valid UTF-8 text",
            Self::Empty => "the name is empty",
            Self::DotComponent => "'.' and '..' are not valid names",
            Self::RootOrPrefix => "the name must be a single relative component",
            Self::Separator => "the name contains a path separator",
            Self::Control => "the name contains a control character",
            Self::AlternateDataStream => "the name contains a colon",
            Self::ForbiddenCharacter => "the name contains a character Windows does not allow",
            Self::TooLong => "the name is longer than 255 UTF-8 bytes",
            Self::ReservedDevice => "the name is reserved by Windows",
            Self::TrailingDotOrSpace => "the name ends in a dot or space",
            Self::StreamGrammar => {
                "stream names must start with a lowercase ASCII letter or digit and then use only lowercase ASCII letters, digits, dots, underscores, or hyphens"
            }
            Self::ClaimGrammar => {
                "claim names must be !solstone-claim- followed by 8 lowercase hexadecimal bytes, a hyphen, and 16 lowercase hexadecimal bytes"
            }
        })
    }
}

/// An admitted stream directory name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamName(String);

impl StreamName {
    /// Admit `candidate` under portable policy, then stream grammar.
    pub fn parse(candidate: &str) -> Result<Self, NameAdmissionReason> {
        check_portable_component(candidate)?;
        check_stream_grammar(candidate)?;
        Ok(Self(candidate.to_owned()))
    }

    /// Borrow the admitted name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A caller-owned operation-unique claim name.
///
/// Its leading `!` is never a valid first byte under [`check_stream_grammar`],
/// which requires a lowercase ASCII letter or digit. ASCII `!` is unchanged by
/// case folding and NFC/NFD normalization, so a valid product entry can never
/// compare equal to a `ClaimName` under case- or normalization-insensitive
/// comparison.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimName(String);

impl ClaimName {
    /// Admit an exact reserved claim-name spelling.
    pub fn parse(candidate: &str) -> Result<Self, NameAdmissionReason> {
        check_portable_component(candidate)?;
        check_claim_grammar(candidate)?;
        Ok(Self(candidate.to_owned()))
    }

    /// Borrow the admitted claim name as an operating-system component.
    #[must_use]
    pub fn as_os_str(&self) -> &OsStr {
        OsStr::new(&self.0)
    }

    /// Borrow the admitted claim name as UTF-8 text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Codes 1–10 of the portable-component policy, in precedence order.
pub fn check_portable_component(candidate: &str) -> Result<(), NameAdmissionReason> {
    if candidate.is_empty() {
        return Err(NameAdmissionReason::Empty);
    }
    if candidate == "." || candidate == ".." {
        return Err(NameAdmissionReason::DotComponent);
    }
    if candidate.starts_with('/') {
        return Err(NameAdmissionReason::RootOrPrefix);
    }
    if candidate.contains('/') || candidate.contains('\\') {
        return Err(NameAdmissionReason::Separator);
    }
    if candidate.bytes().any(|byte| byte < 0x20 || byte == 0x7F) {
        return Err(NameAdmissionReason::Control);
    }
    if candidate.contains(':') {
        return Err(NameAdmissionReason::AlternateDataStream);
    }
    if candidate
        .bytes()
        .any(|byte| matches!(byte, b'<' | b'>' | b'"' | b'|' | b'?' | b'*'))
    {
        return Err(NameAdmissionReason::ForbiddenCharacter);
    }
    if candidate.len() > 255 {
        return Err(NameAdmissionReason::TooLong);
    }
    if is_reserved_device(candidate) {
        return Err(NameAdmissionReason::ReservedDevice);
    }
    if candidate.ends_with('.') || candidate.ends_with(' ') {
        return Err(NameAdmissionReason::TrailingDotOrSpace);
    }
    Ok(())
}

/// Exact-lookup syntax check: Empty, DotComponent, RootOrPrefix, `/`-only
/// Separator, and NUL-only Control. Codes 6–11 never apply.
pub(crate) fn check_lookup_component(candidate: &str) -> Result<(), NameAdmissionReason> {
    if candidate.is_empty() {
        return Err(NameAdmissionReason::Empty);
    }
    if candidate == "." || candidate == ".." {
        return Err(NameAdmissionReason::DotComponent);
    }
    if candidate.starts_with('/') {
        return Err(NameAdmissionReason::RootOrPrefix);
    }
    if candidate.contains('/') {
        return Err(NameAdmissionReason::Separator);
    }
    if candidate.contains('\0') {
        return Err(NameAdmissionReason::Control);
    }
    Ok(())
}

fn check_stream_grammar(candidate: &str) -> Result<(), NameAdmissionReason> {
    let bytes = candidate.as_bytes();
    let Some((first, rest)) = bytes.split_first() else {
        return Err(NameAdmissionReason::Empty);
    };
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(NameAdmissionReason::StreamGrammar);
    }
    if rest.iter().any(|byte| {
        !byte.is_ascii_lowercase()
            && !byte.is_ascii_digit()
            && *byte != b'.'
            && *byte != b'_'
            && *byte != b'-'
    }) {
        return Err(NameAdmissionReason::StreamGrammar);
    }
    Ok(())
}

fn check_claim_grammar(candidate: &str) -> Result<(), NameAdmissionReason> {
    const PREFIX: &str = "!solstone-claim-";
    let Some(rest) = candidate.strip_prefix(PREFIX) else {
        return Err(NameAdmissionReason::ClaimGrammar);
    };
    let Some((pid, operation)) = rest.split_once('-') else {
        return Err(NameAdmissionReason::ClaimGrammar);
    };
    if pid.len() != 8
        || operation.len() != 16
        || !pid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || !operation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(NameAdmissionReason::ClaimGrammar);
    }
    Ok(())
}

fn is_reserved_device(candidate: &str) -> bool {
    let trimmed = candidate.trim_end_matches(['.', ' ']);
    let stem = trimmed.split('.').next().unwrap_or(trimmed);
    is_device_stem(stem)
}

fn is_device_stem(stem: &str) -> bool {
    if stem.eq_ignore_ascii_case("CON")
        || stem.eq_ignore_ascii_case("PRN")
        || stem.eq_ignore_ascii_case("AUX")
        || stem.eq_ignore_ascii_case("NUL")
    {
        return true;
    }
    is_numbered_device(stem, "COM") || is_numbered_device(stem, "LPT")
}

const SUPERSCRIPT_DIGITS: &[&str] = &[
    "\u{00b9}", "\u{00b2}", "\u{00b3}", "\u{2074}", "\u{2075}", "\u{2076}", "\u{2077}", "\u{2078}",
    "\u{2079}",
];

fn is_numbered_device(stem: &str, prefix: &str) -> bool {
    let Some(rest) = strip_ascii_prefix_ignore_case(stem, prefix) else {
        return false;
    };
    matches!(rest, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
        || SUPERSCRIPT_DIGITS.contains(&rest)
}

fn strip_ascii_prefix_ignore_case<'a>(stem: &'a str, prefix: &str) -> Option<&'a str> {
    let stem_bytes = stem.as_bytes();
    let prefix_bytes = prefix.as_bytes();
    if stem_bytes.len() < prefix_bytes.len() {
        return None;
    }
    if !stem_bytes[..prefix_bytes.len()].eq_ignore_ascii_case(prefix_bytes) {
        return None;
    }
    std::str::from_utf8(&stem_bytes[prefix_bytes.len()..]).ok()
}

/// Non-directory kind observed without following a symlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoFollowEntryKind {
    /// A regular file.
    RegularFile,
    /// A symlink, including a dangling symlink.
    Symlink,
    /// FIFO, socket, device, or any other non-directory.
    Other,
}

/// Kind of a colliding directory entry, observed without following symlinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    /// A real directory.
    Directory,
    /// A regular file.
    RegularFile,
    /// A symlink, including a dangling symlink.
    Symlink,
    /// FIFO, socket, device, or any other non-directory.
    Other,
}

impl ConflictKind {
    pub(crate) fn as_wrong_kind(self) -> Option<NoFollowEntryKind> {
        match self {
            Self::Directory => None,
            Self::RegularFile => Some(NoFollowEntryKind::RegularFile),
            Self::Symlink => Some(NoFollowEntryKind::Symlink),
            Self::Other => Some(NoFollowEntryKind::Other),
        }
    }
}

/// One case-insensitive match from a collision scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConflictEntry {
    /// UTF-8 entry basename.
    pub name: String,
    /// No-follow kind of the entry.
    pub kind: ConflictKind,
}

/// Failure while admitting or collision-scanning a name.
#[derive(Debug)]
pub enum NameAdmissionError {
    /// The candidate failed portable or grammar checks.
    Invalid {
        /// Rejected name.
        candidate: String,
        /// First matching reason.
        reason: NameAdmissionReason,
    },
    /// The candidate collides with existing names when letter case is ignored.
    Collision {
        /// Requested name.
        candidate: String,
        /// Every case-insensitive match, sorted by name.
        conflicts: Vec<ConflictEntry>,
    },
    /// Listing or metadata failed. Never treated as "no conflict."
    Io {
        /// Path that could not be listed or inspected.
        path: PathBuf,
        /// Underlying filesystem error.
        source: io::Error,
    },
    /// A prospective path escaped the journal root.
    Containment(PathEscapeError),
}

impl fmt::Display for NameAdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid { candidate, reason } => {
                write!(
                    formatter,
                    "invalid name '{}': {reason}",
                    escape_name(candidate)
                )
            }
            Self::Collision {
                candidate,
                conflicts,
            } => {
                let existing = conflicts
                    .first()
                    .map(|entry| escape_name(&entry.name))
                    .unwrap_or_default();
                write!(
                    formatter,
                    "name '{}' conflicts with '{existing}' when letter case is ignored",
                    escape_name(candidate)
                )
            }
            Self::Io { path, source } => {
                write!(formatter, "{}: {source}", escape_path(path))
            }
            Self::Containment(error) => error.fmt(formatter),
        }
    }
}

impl Error for NameAdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Containment(error) => Some(error),
            Self::Invalid { .. } | Self::Collision { .. } => None,
        }
    }
}

/// Escape a filesystem path for owner-facing text.
pub(crate) fn escape_path(path: &Path) -> String {
    escape_name(&path.display().to_string())
}

/// Escape a user-controlled name for owner-facing text.
pub(crate) fn escape_name(name: &str) -> String {
    let mut escaped = String::new();
    for character in name.chars() {
        match character {
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\u{202d}' => escaped.push_str("\\u{202d}"),
            '\u{202e}' => escaped.push_str("\\u{202e}"),
            '\u{2066}' => escaped.push_str("\\u{2066}"),
            '\u{2067}' => escaped.push_str("\\u{2067}"),
            '\u{2068}' => escaped.push_str("\\u{2068}"),
            '\u{2069}' => escaped.push_str("\\u{2069}"),
            character if (character as u32) < 0x20 || character == '\u{7f}' => {
                escaped.push_str(&format!("\\u{{{:x}}}", character as u32));
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// Whether an admitted name can reuse an existing byte-exact directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NameReuse {
    /// No match; the caller may create this name.
    Create,
    /// Exactly one byte-exact directory; reuse it.
    Reuse,
}

pub(crate) fn classify_no_follow(file_type: fs::FileType) -> ConflictKind {
    if file_type.is_symlink() {
        ConflictKind::Symlink
    } else if file_type.is_dir() {
        ConflictKind::Directory
    } else if file_type.is_file() {
        ConflictKind::RegularFile
    } else {
        ConflictKind::Other
    }
}

fn read_no_follow_entries(parent: &Path) -> Result<Vec<(OsString, ConflictKind)>, io::Error> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut listed = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        listed.push((entry.file_name(), classify_no_follow(metadata.file_type())));
    }
    Ok(listed)
}

pub(crate) fn conflicts_for_candidate(
    candidate: &str,
    entries: &[(OsString, ConflictKind)],
) -> Vec<ConflictEntry> {
    let mut conflicts = Vec::new();
    for (name, kind) in entries {
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.eq_ignore_ascii_case(candidate) {
            conflicts.push(ConflictEntry {
                name: name.to_owned(),
                kind: *kind,
            });
        }
    }
    conflicts.sort_by(|left, right| left.name.cmp(&right.name));
    conflicts
}

pub(crate) fn decide_reuse(
    candidate: &str,
    conflicts: Vec<ConflictEntry>,
) -> Result<NameReuse, NameAdmissionError> {
    match conflicts.as_slice() {
        [] => Ok(NameReuse::Create),
        [one] if one.name == candidate && matches!(one.kind, ConflictKind::Directory) => {
            Ok(NameReuse::Reuse)
        }
        _ => Err(NameAdmissionError::Collision {
            candidate: candidate.to_owned(),
            conflicts,
        }),
    }
}

/// Scan `parent` for case-insensitive conflicts with `candidate`.
///
/// A missing parent is zero conflicts. Any other listing failure is I/O.
/// Not atomic with a later create: there is no lock file, and a concurrent
/// raw-filesystem writer (including `segment_path` / talent-runtime
/// `DEFAULT_STREAM`) can still plant a colliding name between scan and mutation.
pub(crate) fn scan_directory_conflicts(
    parent: &Path,
    candidate: &str,
) -> Result<NameReuse, NameAdmissionError> {
    let entries = match read_no_follow_entries(parent) {
        Ok(entries) => entries,
        Err(source) => {
            return Err(NameAdmissionError::Io {
                path: parent.to_path_buf(),
                source,
            });
        }
    };
    decide_reuse(candidate, conflicts_for_candidate(candidate, &entries))
}

#[cfg(test)]
mod tests {
    use super::{
        ClaimName, ConflictKind, NameAdmissionError, NameAdmissionReason, NameReuse, StreamName,
        check_lookup_component, check_portable_component, conflicts_for_candidate, decide_reuse,
        escape_name, scan_directory_conflicts,
    };
    use crate::test_support::TempDir;
    use std::ffi::OsString;
    use std::fs;

    #[test]
    fn claim_name_uses_the_exact_reserved_disjoint_grammar() {
        let valid = "!solstone-claim-0012abcd-0123456789abcdef";
        assert_eq!(ClaimName::parse(valid).unwrap().as_str(), valid);
        for invalid in [
            "solstone-claim-0012abcd-0123456789abcdef",
            "!solstone-claim-0012ABCD-0123456789abcdef",
            "!solstone-claim-0012abcd-0123456789ABCDEF",
            "!solstone-claim-0012abc-0123456789abcdef",
            "!solstone-claim-0012abcd-0123456789abcde",
            "!solstone-claim-0012abcd-0123456789abcdef-extra",
        ] {
            assert_eq!(
                ClaimName::parse(invalid),
                Err(NameAdmissionReason::ClaimGrammar)
            );
        }
        assert_eq!(
            StreamName::parse(valid),
            Err(NameAdmissionReason::StreamGrammar)
        );
    }

    #[test]
    fn portable_policy_rejects_in_precedence_order() {
        let cases = [
            ("", NameAdmissionReason::Empty),
            (".", NameAdmissionReason::DotComponent),
            ("..", NameAdmissionReason::DotComponent),
            ("/abs", NameAdmissionReason::RootOrPrefix),
            ("a/", NameAdmissionReason::Separator),
            (r"a\", NameAdmissionReason::Separator),
            ("a\nb", NameAdmissionReason::Control),
            ("a\u{7f}b", NameAdmissionReason::Control),
            ("a:b", NameAdmissionReason::AlternateDataStream),
            ("a<b", NameAdmissionReason::ForbiddenCharacter),
            ("a>b", NameAdmissionReason::ForbiddenCharacter),
            ("a\"b", NameAdmissionReason::ForbiddenCharacter),
            ("a|b", NameAdmissionReason::ForbiddenCharacter),
            ("a?b", NameAdmissionReason::ForbiddenCharacter),
            ("a*b", NameAdmissionReason::ForbiddenCharacter),
            ("con", NameAdmissionReason::ReservedDevice),
            ("CON", NameAdmissionReason::ReservedDevice),
            ("con.txt", NameAdmissionReason::ReservedDevice),
            ("CON.", NameAdmissionReason::ReservedDevice),
            ("com1", NameAdmissionReason::ReservedDevice),
            ("lpt9", NameAdmissionReason::ReservedDevice),
            ("COM\u{00b9}", NameAdmissionReason::ReservedDevice),
            ("foo.", NameAdmissionReason::TrailingDotOrSpace),
            ("foo ", NameAdmissionReason::TrailingDotOrSpace),
        ];
        for (candidate, reason) in cases {
            assert_eq!(
                check_portable_component(candidate),
                Err(reason),
                "{candidate:?}"
            );
        }
        assert_eq!(
            check_portable_component(r"a\"),
            Err(NameAdmissionReason::Separator)
        );
        assert_ne!(
            check_portable_component(r"a\"),
            Err(NameAdmissionReason::ForbiddenCharacter)
        );
        assert_eq!(
            check_portable_component(&"a".repeat(256)),
            Err(NameAdmissionReason::TooLong)
        );
        check_portable_component(&"a".repeat(255)).unwrap();
    }

    #[test]
    fn seed_stream_and_segment_admit_cleanly() {
        assert_eq!(
            StreamName::parse("import.apple_health").unwrap().as_str(),
            "import.apple_health"
        );
        check_portable_component("000000_300").unwrap();
        StreamName::parse("000000_300").unwrap();
    }

    #[test]
    fn segment_admission_does_not_apply_stream_grammar() {
        check_portable_component("Foo").unwrap();
        assert_eq!(
            StreamName::parse("Foo"),
            Err(NameAdmissionReason::StreamGrammar)
        );
        check_portable_component("-foo").unwrap();
        assert_eq!(
            StreamName::parse("-foo"),
            Err(NameAdmissionReason::StreamGrammar)
        );
    }

    #[test]
    fn lookup_allows_backslash_uppercase_colon_and_non_nul_controls() {
        check_lookup_component(r"foo\bar").unwrap();
        check_lookup_component("CON").unwrap();
        check_lookup_component("Foo").unwrap();
        check_lookup_component("a:b").unwrap();
        check_lookup_component("foo.").unwrap();
        check_lookup_component("café").unwrap();
        check_lookup_component("a\tb").unwrap();
        assert_eq!(check_lookup_component(""), Err(NameAdmissionReason::Empty));
        assert_eq!(
            check_lookup_component("."),
            Err(NameAdmissionReason::DotComponent)
        );
        assert_eq!(
            check_lookup_component("/abs"),
            Err(NameAdmissionReason::RootOrPrefix)
        );
        assert_eq!(
            check_lookup_component("a/b"),
            Err(NameAdmissionReason::Separator)
        );
        assert_eq!(
            check_lookup_component("a\0b"),
            Err(NameAdmissionReason::Control)
        );
    }

    #[test]
    fn collision_reuses_byte_exact_directory() {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join("import.apple_health")).unwrap();
        assert_eq!(
            scan_directory_conflicts(temporary.path(), "import.apple_health").unwrap(),
            NameReuse::Reuse
        );
    }

    #[test]
    fn collision_rejects_case_variant_directory() {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join("Import.Apple_Health")).unwrap();
        let error = scan_directory_conflicts(temporary.path(), "import.apple_health").unwrap_err();
        match error {
            NameAdmissionError::Collision {
                candidate,
                conflicts,
            } => {
                assert_eq!(candidate, "import.apple_health");
                assert_eq!(conflicts[0].name, "Import.Apple_Health");
                assert_eq!(conflicts[0].kind, ConflictKind::Directory);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn collision_rejects_exact_regular_file() {
        let temporary = TempDir::new();
        fs::write(temporary.path().join("import.apple_health"), b"file").unwrap();
        let error = scan_directory_conflicts(temporary.path(), "import.apple_health").unwrap_err();
        match error {
            NameAdmissionError::Collision { conflicts, .. } => {
                assert_eq!(conflicts[0].kind, ConflictKind::RegularFile);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn collision_two_matches_from_synthetic_entries() {
        // Linux cannot host two ASCII case-variants in one directory, so the
        // 2+ match arm is exercised with an in-memory listing.
        let conflicts = conflicts_for_candidate(
            "main",
            &[
                (OsString::from("Main"), ConflictKind::Directory),
                (OsString::from("MAIN"), ConflictKind::Directory),
            ],
        );
        assert_eq!(conflicts.len(), 2);
        assert_eq!(conflicts[0].name, "MAIN");
        assert_eq!(conflicts[1].name, "Main");
        assert!(matches!(
            decide_reuse("main", conflicts),
            Err(NameAdmissionError::Collision { .. })
        ));
    }

    #[test]
    fn missing_parent_is_zero_conflicts() {
        let temporary = TempDir::new();
        assert_eq!(
            scan_directory_conflicts(&temporary.path().join("missing"), "main").unwrap(),
            NameReuse::Create
        );
    }

    #[test]
    fn regular_file_at_parent_is_io_not_empty() {
        let temporary = TempDir::new();
        let parent = temporary.path().join("day");
        fs::write(&parent, b"not-a-directory").unwrap();
        match scan_directory_conflicts(&parent, "import.apple_health") {
            Err(NameAdmissionError::Io { path, .. }) => assert_eq!(path, parent),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn corpus_mapping_round_trips_through_eq_ignore_ascii_case() {
        let raw = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/windows-compare-string-ordinal-ascii-corpus-260823.md"
        ));
        let line = raw
            .lines()
            .find(|line| line.starts_with("a:U+0041"))
            .expect("mapping block");
        let mut pairs = Vec::new();
        for pair in line.split(',') {
            let (unit, code) = pair.split_once(':').expect("unit:code");
            let hex = code.strip_prefix("U+").expect("U+");
            let value = u32::from_str_radix(hex, 16).expect("hex");
            let character = char::from_u32(value).expect("scalar");
            assert!(
                unit.eq_ignore_ascii_case(&character.to_string()),
                "{unit} vs {character:?}"
            );
            pairs.push((unit.to_owned(), character));
        }
        assert_eq!(pairs.len(), 65);
    }

    #[test]
    fn collision_does_not_fold_kelvin_u212a() {
        assert_unicode_not_equivalent("k", "\u{212a}");
    }

    #[test]
    fn collision_does_not_fold_long_s_u017f() {
        assert_unicode_not_equivalent("s", "\u{017f}");
    }

    #[test]
    fn collision_does_not_fold_dotless_i_u0131() {
        assert_unicode_not_equivalent("i", "\u{0131}");
    }

    #[test]
    fn collision_does_not_fold_sharp_s_u00df() {
        assert_unicode_not_equivalent("s", "\u{00df}");
        assert_unicode_not_equivalent("ss", "\u{00df}");
    }

    #[test]
    fn collision_does_not_fold_fullwidth_a_uff41() {
        assert_unicode_not_equivalent("a", "\u{ff41}");
    }

    #[test]
    fn collision_does_not_fold_a_ring_u00c5_or_u212b() {
        assert_unicode_not_equivalent("a", "\u{00c5}");
        assert_unicode_not_equivalent("a", "\u{212b}");
    }

    #[test]
    fn collision_does_not_fold_composed_or_decomposed_e_acute() {
        assert_unicode_not_equivalent("e", "\u{00e9}");
        assert_unicode_not_equivalent("e", "e\u{0301}");
        assert!(!"\u{00e9}".eq_ignore_ascii_case("e\u{0301}"));
    }

    #[test]
    fn stream_name_parse_rejects_unicode_fold_candidates() {
        for candidate in [
            "\u{212a}", "\u{017f}", "\u{0131}", "\u{00df}", "\u{ff41}", "\u{00c5}", "\u{212b}",
            "\u{00e9}",
        ] {
            assert_eq!(
                StreamName::parse(candidate),
                Err(NameAdmissionReason::StreamGrammar),
                "{candidate:?}"
            );
        }
    }

    #[test]
    fn escape_name_renders_controls_and_direction_marks() {
        assert_eq!(escape_name("a\nb"), "a\\nb");
        assert_eq!(escape_name("a\"b"), "a\\\"b");
        assert_eq!(escape_name("a\u{202e}b"), "a\\u{202e}b");
        assert_eq!(escape_name("a\\b"), "a\\\\b");
    }

    fn assert_unicode_not_equivalent(candidate: &str, on_disk: &str) {
        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join(on_disk)).unwrap();
        assert_eq!(
            scan_directory_conflicts(temporary.path(), candidate).unwrap(),
            NameReuse::Create
        );
        assert!(!candidate.eq_ignore_ascii_case(on_disk));
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use super::{ConflictKind, NameAdmissionError, classify_no_follow, scan_directory_conflicts};
    use crate::test_support::TempDir;
    use std::fs;
    #[cfg(target_os = "linux")]
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;
    use std::path::Path;

    #[test]
    fn collision_rejects_exact_symlink() {
        let temporary = TempDir::new();
        let target = temporary.path().join("target");
        fs::create_dir(&target).unwrap();
        symlink(&target, temporary.path().join("import.apple_health")).unwrap();
        match scan_directory_conflicts(temporary.path(), "import.apple_health") {
            Err(NameAdmissionError::Collision { conflicts, .. }) => {
                assert_eq!(conflicts[0].kind, ConflictKind::Symlink);
            }
            other => panic!("{other:?}"),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn collision_skips_non_utf8_names() {
        use std::ffi::OsStr;

        let temporary = TempDir::new();
        fs::create_dir(temporary.path().join(OsStr::from_bytes(b"main\xff"))).unwrap();
        fs::create_dir(temporary.path().join("other")).unwrap();
        super::decide_reuse(
            "main",
            super::conflicts_for_candidate(
                "main",
                &[(
                    OsStr::from_bytes(b"main\xff").to_os_string(),
                    ConflictKind::Directory,
                )],
            ),
        )
        .expect("non-UTF-8 names cannot match an ASCII candidate");
        assert_eq!(
            scan_directory_conflicts(temporary.path(), "main").unwrap(),
            super::NameReuse::Create
        );
    }

    #[test]
    fn classify_fifo_is_other() {
        let temporary = TempDir::new();
        let fifo = temporary.path().join("pipe");
        mkfifo(&fifo);
        let metadata = fs::symlink_metadata(&fifo).unwrap();
        assert_eq!(
            classify_no_follow(metadata.file_type()),
            ConflictKind::Other
        );
    }

    fn mkfifo(path: &Path) {
        nix::unistd::mkfifo(
            path,
            nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
        )
        .expect("mkfifo");
    }
}
