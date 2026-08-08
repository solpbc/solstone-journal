// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Capability-safe explicit archive output-target acquisition.

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, open, openat};
use nix::sys::stat::{FileStat, Mode, SFlag, fstat, fstatat};
use std::ffi::{OsStr, OsString};
use std::fmt::{Display, Formatter};
use std::io::Error;
use std::option::Option;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::result::Result;

const TARGET_DIRECTORY_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_CLOEXEC)
    .union(OFlag::O_NOFOLLOW);

pub(crate) struct NormalComponent(OsString);

impl NormalComponent {
    fn parse(value: &OsStr) -> Option<Self> {
        let mut components = Path::new(value).components();
        match (components.next(), components.next()) {
            (Option::Some(Component::Normal(component)), Option::None) if component == value => {
                Option::Some(Self(component.to_os_string()))
            }
            _ => Option::None,
        }
    }

    pub(crate) fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

fn open_directory_component(
    parent: &impl AsFd,
    component: &NormalComponent,
) -> Result<OwnedFd, Errno> {
    openat(
        parent,
        component.as_os_str(),
        TARGET_DIRECTORY_FLAGS,
        Mode::empty(),
    )
}

fn stat_component(parent: &impl AsFd, component: &NormalComponent) -> Result<FileStat, Errno> {
    fstatat(parent, component.as_os_str(), AtFlags::AT_SYMLINK_NOFOLLOW)
}

fn open_filesystem_root() -> Result<OwnedFd, Errno> {
    open("/", TARGET_DIRECTORY_FLAGS, Mode::empty())
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct DirectoryProof {
    device: u128,
    inode: u128,
}

struct OpenedDirectory {
    path: PathBuf,
    proof: DirectoryProof,
    fd: OwnedFd,
}

#[derive(Clone, Eq, PartialEq)]
struct RouteProof {
    path: PathBuf,
    proof: DirectoryProof,
}

struct WalkedRoute {
    final_path: PathBuf,
    final_name: NormalComponent,
    parent: OwnedFd,
    proofs: Vec<RouteProof>,
}

/// The caller-supplied explicit archive output path and its injected cwd.
pub struct ExplicitArchiveOutputRequest {
    output: PathBuf,
    cwd: PathBuf,
}

impl ExplicitArchiveOutputRequest {
    /// Construct an explicit output selection from caller-provided path state.
    pub fn new(output: std::path::PathBuf, cwd: std::path::PathBuf) -> Self {
        Self { output, cwd }
    }
}

/// A retained, revalidatable existing-parent archive output target.
pub struct ArchiveOutputTarget {
    final_path: PathBuf,
    pub(crate) final_name: NormalComponent,
    output: PathBuf,
    cwd: PathBuf,
    pub(crate) parent: OwnedFd,
    proofs: Vec<RouteProof>,
}

impl ArchiveOutputTarget {
    /// Return the normalized absolute output path.
    pub fn final_path(&self) -> &std::path::Path {
        &self.final_path
    }

    /// Confirm that the requested route still names the retained parent and a free final name.
    pub fn revalidate(&self) -> std::result::Result<(), ExplicitTargetError> {
        let walked = walk_parent_route(&self.output, &self.cwd)
            .map_err(|error| revalidation_route_error(error, &self.final_path))?;
        if walked.final_path != self.final_path
            || walked.final_name.as_os_str() != self.final_name.as_os_str()
            || walked.proofs != self.proofs
        {
            return Err(ExplicitTargetError::TargetChanged {
                path: self.final_path.to_owned(),
            });
        }
        let retained = fstat(&self.parent).map_err(|source| {
            target_io(
                "stat retained archive output parent",
                &self.final_path,
                source,
            )
        })?;
        let retained =
            directory_proof(&retained).ok_or_else(|| ExplicitTargetError::TargetChanged {
                path: self.final_path.to_owned(),
            })?;
        let active = walked
            .proofs
            .last()
            .map(|proof| proof.proof)
            .ok_or_else(|| ExplicitTargetError::TargetChanged {
                path: self.final_path.to_owned(),
            })?;
        if retained != active {
            return Err(ExplicitTargetError::TargetChanged {
                path: self.final_path.to_owned(),
            });
        }
        inspect_final(&walked.parent, &walked.final_name, &walked.final_path)
    }
}

/// Failure while resolving or revalidating an explicit archive target.
#[derive(Debug)]
pub enum ExplicitTargetError {
    /// The supplied path or cwd cannot denote an explicit archive file.
    InvalidTarget {
        path: std::path::PathBuf,
        reason: &'static str,
    },
    /// A target filesystem operation failed without evidence of route replacement.
    TargetIo {
        operation: &'static str,
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    /// The requested route no longer names the acquired directory chain.
    TargetChanged { path: std::path::PathBuf },
    /// A regular file already occupies the final output name.
    Collision { path: std::path::PathBuf },
    /// A non-regular object occupies the final output name or parent route.
    UnsafeTarget {
        path: std::path::PathBuf,
        kind: &'static str,
    },
}

impl Display for ExplicitTargetError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidTarget { path, reason } => {
                write!(
                    formatter,
                    "invalid archive output {}: {reason}",
                    path.display()
                )
            }
            Self::TargetIo {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} failed for {}: {source}",
                path.display()
            ),
            Self::TargetChanged { path } => write!(
                formatter,
                "archive output route changed during export: {}",
                path.display()
            ),
            Self::Collision { path } => {
                write!(
                    formatter,
                    "archive output already exists: {}",
                    path.display()
                )
            }
            Self::UnsafeTarget { path, kind } => write!(
                formatter,
                "archive output is an unsafe {kind}: {}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ExplicitTargetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::TargetIo { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Acquire an explicit existing-parent archive output target without mutation.
pub fn acquire_explicit_output_target(
    request: &ExplicitArchiveOutputRequest,
) -> std::result::Result<ArchiveOutputTarget, ExplicitTargetError> {
    let walked = walk_parent_route(&request.output, &request.cwd)?;
    inspect_final(&walked.parent, &walked.final_name, &walked.final_path)?;
    let target = ArchiveOutputTarget {
        final_path: walked.final_path,
        final_name: walked.final_name,
        output: request.output.to_owned(),
        cwd: request.cwd.to_owned(),
        parent: walked.parent,
        proofs: walked.proofs,
    };
    target.revalidate()?;
    Ok(target)
}

fn walk_parent_route(output: &Path, cwd: &Path) -> Result<WalkedRoute, ExplicitTargetError> {
    validate_output_bytes(output)?;
    let final_name = output
        .components()
        .next_back()
        .and_then(|component| match component {
            Component::Normal(value) => NormalComponent::parse(value),
            _ => Option::None,
        })
        .ok_or_else(|| invalid(output, "archive output has no file name"))?;

    let mut current_path = PathBuf::from("/");
    let root = open_filesystem_root()
        .map_err(|source| target_io("open archive output filesystem root", output, source))?;
    let root_stat = fstat(&root)
        .map_err(|source| target_io("stat archive output filesystem root", output, source))?;
    let root_proof = directory_proof(&root_stat)
        .ok_or_else(|| invalid(output, "filesystem root is not a directory"))?;
    let mut stack = Vec::new();
    stack.push(OpenedDirectory {
        path: current_path.to_owned(),
        proof: root_proof,
        fd: root,
    });

    if output.is_absolute() {
        walk_components(output.parent(), output, &mut current_path, &mut stack)?;
    } else {
        if !cwd.is_absolute() {
            return Err(invalid(
                cwd,
                "relative archive output requires an absolute cwd",
            ));
        }
        validate_utf8(cwd)?;
        walk_components(Option::Some(cwd), output, &mut current_path, &mut stack)?;
        walk_components(output.parent(), output, &mut current_path, &mut stack)?;
    }

    let mut final_path = current_path.to_owned();
    final_path.push(final_name.as_os_str());
    let proofs = stack
        .iter()
        .map(|opened| RouteProof {
            path: opened.path.to_owned(),
            proof: opened.proof,
        })
        .collect();
    let parent = stack
        .pop()
        .map(|opened| opened.fd)
        .ok_or_else(|| invalid(output, "archive output has no parent"))?;
    Ok(WalkedRoute {
        final_path,
        final_name,
        parent,
        proofs,
    })
}

fn walk_components(
    path: Option<&Path>,
    requested: &Path,
    current_path: &mut PathBuf,
    stack: &mut Vec<OpenedDirectory>,
) -> Result<(), ExplicitTargetError> {
    let Some(path) = path else {
        return Ok(());
    };
    for component in path.components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => {
                if stack.len() > 1 {
                    stack.pop();
                    current_path.pop();
                }
            }
            Component::Normal(value) => {
                let component = NormalComponent::parse(value)
                    .ok_or_else(|| invalid(requested, "archive parent component is invalid"))?;
                let parent = stack
                    .last()
                    .ok_or_else(|| invalid(requested, "archive output has no parent"))?;
                let before = stat_component(&parent.fd, &component).map_err(|source| {
                    component_error(
                        source,
                        requested,
                        current_path,
                        component.as_os_str(),
                        "stat archive output parent",
                    )
                })?;
                let before_proof = directory_proof(&before).ok_or_else(|| {
                    unsafe_component(current_path, component.as_os_str(), &before)
                })?;
                let opened =
                    open_directory_component(&parent.fd, &component).map_err(|source| {
                        component_error(
                            source,
                            requested,
                            current_path,
                            component.as_os_str(),
                            "open archive output parent",
                        )
                    })?;
                let after = fstat(&opened).map_err(|source| {
                    target_io("stat opened archive output parent", requested, source)
                })?;
                let after_proof =
                    directory_proof(&after).ok_or_else(|| ExplicitTargetError::TargetChanged {
                        path: child_path(current_path, component.as_os_str()),
                    })?;
                if before_proof != after_proof {
                    return Err(ExplicitTargetError::TargetChanged {
                        path: child_path(current_path, component.as_os_str()),
                    });
                }
                current_path.push(component.as_os_str());
                stack.push(OpenedDirectory {
                    path: current_path.to_owned(),
                    proof: after_proof,
                    fd: opened,
                });
            }
            Component::Prefix(_) => {
                return Err(invalid(
                    requested,
                    "archive output uses an unsupported prefix",
                ));
            }
        }
    }
    Ok(())
}

fn inspect_final(
    parent: &impl AsFd,
    final_name: &NormalComponent,
    final_path: &Path,
) -> Result<(), ExplicitTargetError> {
    match stat_component(parent, final_name) {
        Err(Errno::ENOENT) => Ok(()),
        Err(source) => Err(target_io(
            "stat archive output final name",
            final_path,
            source,
        )),
        Ok(status) if file_kind(&status) == "regular file" => Err(ExplicitTargetError::Collision {
            path: final_path.to_owned(),
        }),
        Ok(status) => Err(ExplicitTargetError::UnsafeTarget {
            path: final_path.to_owned(),
            kind: file_kind(&status),
        }),
    }
}

fn validate_output_bytes(output: &Path) -> Result<(), ExplicitTargetError> {
    validate_utf8(output)?;
    let bytes = output.as_os_str().as_bytes();
    if bytes.contains(&b'\0') {
        return Err(invalid(output, "archive output contains a null byte"));
    }
    if bytes.last() == Option::Some(&b'/') {
        return Err(invalid(output, "archive output ends with a separator"));
    }
    if matches!(bytes, b"." | b"..") || bytes.ends_with(b"/.") || bytes.ends_with(b"/..") {
        return Err(invalid(output, "archive output has no file name"));
    }
    Ok(())
}

fn validate_utf8(path: &Path) -> Result<(), ExplicitTargetError> {
    if path.to_str().is_none() {
        return Err(invalid(path, "archive output path is not valid UTF-8"));
    }
    Ok(())
}

fn directory_proof(status: &FileStat) -> Option<DirectoryProof> {
    if SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR {
        return Option::None;
    }
    Option::Some(DirectoryProof {
        device: stat_identifier(status.st_dev)?,
        inode: stat_identifier(status.st_ino)?,
    })
}

fn stat_identifier(value: impl TryInto<u128>) -> Option<u128> {
    value.try_into().ok()
}

fn component_error(
    source: Errno,
    requested: &Path,
    parent: &Path,
    component: &OsStr,
    operation: &'static str,
) -> ExplicitTargetError {
    let unsafe_namespace = matches!(source, Errno::ENOENT | Errno::ENOTDIR | Errno::ELOOP);
    if unsafe_namespace {
        return ExplicitTargetError::InvalidTarget {
            path: child_path(parent, component),
            reason: "archive output parent is missing or unsafe",
        };
    }
    target_io(operation, requested, source)
}

fn invalid(path: &Path, reason: &'static str) -> ExplicitTargetError {
    ExplicitTargetError::InvalidTarget {
        path: path.to_owned(),
        reason,
    }
}

fn unsafe_component(parent: &Path, component: &OsStr, status: &FileStat) -> ExplicitTargetError {
    ExplicitTargetError::UnsafeTarget {
        path: child_path(parent, component),
        kind: file_kind(status),
    }
}

fn target_io(operation: &'static str, path: &Path, source: Errno) -> ExplicitTargetError {
    ExplicitTargetError::TargetIo {
        operation,
        path: path.to_owned(),
        source: Error::from_raw_os_error(source as i32),
    }
}

fn revalidation_route_error(error: ExplicitTargetError, path: &Path) -> ExplicitTargetError {
    match error {
        ExplicitTargetError::InvalidTarget { .. }
        | ExplicitTargetError::UnsafeTarget { .. }
        | ExplicitTargetError::TargetChanged { .. } => ExplicitTargetError::TargetChanged {
            path: path.to_owned(),
        },
        error => error,
    }
}

fn child_path(parent: &Path, component: &OsStr) -> PathBuf {
    let mut path = parent.to_path_buf();
    path.push(component);
    path
}

fn file_kind(status: &FileStat) -> &'static str {
    match SFlag::from_bits_truncate(status.st_mode) & SFlag::S_IFMT {
        SFlag::S_IFREG => "regular file",
        SFlag::S_IFDIR => "directory",
        SFlag::S_IFLNK => "symlink",
        SFlag::S_IFIFO => "fifo",
        SFlag::S_IFSOCK => "socket",
        SFlag::S_IFCHR => "character device",
        SFlag::S_IFBLK => "block device",
        _ => "other",
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use std::fs;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        ExplicitArchiveOutputRequest, ExplicitTargetError, acquire_explicit_output_target,
    };

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(name: &str) -> Self {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock should advance")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "solstone-archive-target-{name}-{}-{stamp}",
                std::process::id()
            ));
            fs::create_dir(&path).expect("create test directory");
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn request(output: PathBuf, cwd: PathBuf) -> ExplicitArchiveOutputRequest {
        ExplicitArchiveOutputRequest::new(output, cwd)
    }

    #[test]
    fn absolute_and_relative_targets_retain_existing_parent_without_mutation() {
        let temp = TempDir::new("ready");
        let parent = temp.path.join("output");
        fs::create_dir(&parent).expect("create output parent");

        let absolute_path = parent.join("absolute.zip");
        let absolute = acquire_explicit_output_target(&request(
            absolute_path.clone(),
            PathBuf::from("ignored-relative-cwd"),
        ))
        .expect("acquire absolute target");
        assert_eq!(absolute.final_path(), absolute_path);
        assert!(!absolute_path.exists());
        absolute.revalidate().expect("revalidate absolute target");

        let non_utf8_cwd = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        let absolute_with_irrelevant_cwd = acquire_explicit_output_target(&request(
            parent.join("absolute-with-non-utf8-cwd.zip"),
            non_utf8_cwd,
        ));
        assert!(absolute_with_irrelevant_cwd.is_ok());

        let relative = acquire_explicit_output_target(&request(
            PathBuf::from("nested/../relative.zip"),
            parent.join("nested"),
        ));
        assert!(matches!(
            relative,
            Err(ExplicitTargetError::InvalidTarget { .. })
        ));

        fs::create_dir(parent.join("nested")).expect("create relative parent");
        let relative = acquire_explicit_output_target(&request(
            PathBuf::from("../relative.zip"),
            parent.join("nested"),
        ))
        .expect("acquire relative target");
        assert_eq!(relative.final_path(), parent.join("relative.zip"));
        assert!(!parent.join("relative.zip").exists());
    }

    #[test]
    fn invalid_paths_and_unsafe_parents_fail_without_creation() {
        let temp = TempDir::new("invalid");
        let parent = temp.path.join("output");
        fs::create_dir(&parent).expect("create output parent");
        symlink(&parent, temp.path.join("linked")).expect("create parent symlink");

        for output in [
            PathBuf::from("missing/../out.zip"),
            PathBuf::from("linked/out.zip"),
            PathBuf::from("."),
            PathBuf::from(".."),
            PathBuf::from("out/."),
            PathBuf::from("out/.."),
            PathBuf::from("/"),
        ] {
            assert!(acquire_explicit_output_target(&request(output, temp.path.clone(),)).is_err());
        }

        let trailing = PathBuf::from(format!("{}/out.zip/", parent.display()));
        assert!(matches!(
            acquire_explicit_output_target(&request(trailing, PathBuf::from("ignored"),)),
            Err(ExplicitTargetError::InvalidTarget { .. })
        ));

        let non_utf8 = PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        assert!(matches!(
            acquire_explicit_output_target(&request(non_utf8, temp.path.clone(),)),
            Err(ExplicitTargetError::InvalidTarget { .. })
        ));
        let nul = PathBuf::from(std::ffi::OsString::from_vec(b"nul\0.zip".to_vec()));
        assert!(matches!(
            acquire_explicit_output_target(&request(nul, temp.path.clone())),
            Err(ExplicitTargetError::InvalidTarget { .. })
        ));
        assert_eq!(fs::read_dir(&parent).expect("read parent").count(), 0);
    }

    #[test]
    fn final_objects_are_classified_without_opening_or_replacing_them() {
        let temp = TempDir::new("final-kind");
        let parent = temp.path.join("output");
        fs::create_dir(&parent).expect("create output parent");

        let regular = parent.join("regular.zip");
        fs::write(&regular, b"keep").expect("create collision");
        assert!(matches!(
            acquire_explicit_output_target(&request(regular.clone(), temp.path.clone(),)),
            Err(ExplicitTargetError::Collision { .. })
        ));
        assert_eq!(fs::read(&regular).expect("read collision"), b"keep");

        let directory = parent.join("directory.zip");
        fs::create_dir(&directory).expect("create final directory");
        assert!(matches!(
            acquire_explicit_output_target(&request(directory, temp.path.clone(),)),
            Err(ExplicitTargetError::UnsafeTarget {
                kind: "directory",
                ..
            })
        ));

        let link = parent.join("link.zip");
        symlink(&regular, &link).expect("create final symlink");
        assert!(matches!(
            acquire_explicit_output_target(&request(link, temp.path.clone(),)),
            Err(ExplicitTargetError::UnsafeTarget {
                kind: "symlink",
                ..
            })
        ));

        let socket = parent.join("socket.zip");
        let listener = UnixListener::bind(&socket).expect("create final socket");
        assert!(matches!(
            acquire_explicit_output_target(&request(socket, temp.path.clone(),)),
            Err(ExplicitTargetError::UnsafeTarget { kind: "socket", .. })
        ));
        drop(listener);
    }

    #[test]
    fn revalidation_detects_ancestor_replacement_even_with_same_final_parent() {
        let temp = TempDir::new("ancestor-swap");
        let base = temp.path.join("base");
        let ancestor = base.join("a");
        let parent = ancestor.join("p");
        fs::create_dir_all(&parent).expect("create target route");
        let output = parent.join("out.zip");
        let target = acquire_explicit_output_target(&request(output, temp.path.clone()))
            .expect("acquire target");

        let old_ancestor = base.join("old-a");
        fs::rename(&ancestor, &old_ancestor).expect("rename ancestor");
        fs::create_dir(&ancestor).expect("create replacement ancestor");
        fs::rename(old_ancestor.join("p"), ancestor.join("p"))
            .expect("move original parent under replacement");

        assert!(matches!(
            target.revalidate(),
            Err(ExplicitTargetError::TargetChanged { .. })
        ));
    }

    #[test]
    fn revalidation_classifies_removed_or_unsafe_parent_as_changed() {
        let temp = TempDir::new("parent-disappears");
        let parent = temp.path.join("output");
        fs::create_dir(&parent).expect("create output parent");
        let output = parent.join("out.zip");
        let target = acquire_explicit_output_target(&request(output, temp.path.clone()))
            .expect("acquire target");

        fs::remove_dir(&parent).expect("remove parent");
        assert!(matches!(
            target.revalidate(),
            Err(ExplicitTargetError::TargetChanged { .. })
        ));
    }
}
