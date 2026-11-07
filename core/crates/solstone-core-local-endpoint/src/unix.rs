// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Unix: the account's owner socket, its peer check and its runtime record.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{RUNTIME_ROUTE, RuntimeRecord, SOCKET_ENV, transport};
use nix::unistd::{Uid, User};

pub const INTERNAL_ROOT_ENV: &str = "SOLSTONE_LOCAL_JOURNAL_ROOT";
pub const INTERNAL_PORT_ENV: &str = "SOLSTONE_LOCAL_CONVEY_PORT";

pub fn owner_uid() -> u32 {
    Uid::effective().as_raw()
}

pub fn owner_home() -> io::Result<PathBuf> {
    User::from_uid(Uid::effective())
        .map_err(io::Error::other)?
        .map(|user| user.dir)
        .ok_or_else(|| io::Error::other("OS account has no home directory"))
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn runtime_directory() -> io::Result<PathBuf> {
    // confstr writes at most the supplied buffer size and returns the size
    // including NUL. A second call detects an unexpected size change.
    let size = unsafe { libc::confstr(libc::_CS_DARWIN_USER_TEMP_DIR, std::ptr::null_mut(), 0) };
    if size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut bytes = vec![0u8; size];
    let written = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if written != size {
        return Err(io::Error::other(
            "could not resolve account temporary directory",
        ));
    }
    bytes.pop();
    Ok(Path::new(std::ffi::OsStr::from_bytes(&bytes)).join("solstone"))
}

#[cfg(target_os = "linux")]
fn runtime_directory() -> io::Result<PathBuf> {
    Ok(owner_home()?.join(".local/run/solstone"))
}

/// An empty override explicitly selects TCP-only for supervised children.
pub fn socket_path() -> io::Result<Option<PathBuf>> {
    match std::env::var_os(SOCKET_ENV) {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => Ok(Some(PathBuf::from(value))),
        None => Ok(Some(runtime_directory()?.join("journal.sock"))),
    }
}

/// Only the configured journal may claim the account's default namespace.
pub fn socket_for_journal(journal: &Path) -> io::Result<Option<PathBuf>> {
    if std::env::var_os(SOCKET_ENV).is_some() {
        return socket_path();
    }
    let home = owner_home()?;
    let configured = match fs::read_to_string(home.join(".config/solstone/config.toml")) {
        Ok(text) => {
            let doc = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(io::Error::other)?;
            doc.get("journal")
                .and_then(|v| v.as_str())
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("journal"))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => home.join("journal"),
        Err(e) => return Err(e),
    };
    if fs::canonicalize(journal)? == fs::canonicalize(configured).unwrap_or_default() {
        socket_path()
    } else {
        Ok(None)
    }
}

pub fn validate_socket_path(path: &Path) -> io::Result<()> {
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
    if !path.is_absolute() || path.as_os_str().as_bytes().len() >= limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("journal socket needs an absolute path shorter than {limit} bytes"),
        ));
    }
    Ok(())
}

pub fn owned_socket(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket() && m.uid() == owner_uid())
}

pub fn connect(path: &Path, timeout: Duration) -> io::Result<std::os::unix::net::UnixStream> {
    if !owned_socket(path) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "journal socket is not owned by this account",
        ));
    }
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    let fd: std::os::fd::OwnedFd = socket.into();
    Ok(fd.into())
}

pub fn record_path(socket: &Path) -> PathBuf {
    socket.with_file_name("journal.json")
}

/// Owns only the socket it bound, never a pre-existing live listener.
pub struct SocketLease {
    pub listener: std::os::unix::net::UnixListener,
    path: PathBuf,
    inode: u64,
    _directory_lock: Option<nix::fcntl::Flock<std::fs::File>>,
    owns_record: bool,
}
impl SocketLease {
    pub fn bind(path: &Path) -> io::Result<Self> {
        Self::bind_inner(path, true)
    }

    /// Bind a companion using its own creation lock, so the HTTP owner's
    /// lifetime directory lease does not prevent a companion restart.
    pub fn bind_companion(path: &Path) -> io::Result<Self> {
        Self::bind_inner(path, false)
    }

    fn bind_inner(path: &Path, owns_record: bool) -> io::Result<Self> {
        validate_socket_path(path)?;
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("journal socket has no directory"))?;
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder.recursive(true).mode(0o700).create(parent)?;
        let metadata = fs::symlink_metadata(parent)?;
        if !metadata.is_dir() || metadata.uid() != owner_uid() || metadata.mode() & 0o777 != 0o700 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "journal socket directory must belong to this account and have mode 0700",
            ));
        }
        // Serialize the stale-check/unlink/bind transaction. A concurrent start
        // must not unlink a socket another start just brought up.
        let lock_file = if owns_record {
            std::fs::File::open(parent)?
        } else {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
                .open(path.with_extension("bind.lock"))?;
            let metadata = file.metadata()?;
            if !metadata.is_file()
                || metadata.uid() != owner_uid()
                || metadata.mode() & 0o777 != 0o600
            {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "companion socket lock must be a private file owned by this account",
                ));
            }
            file
        };
        let directory_lock =
            nix::fcntl::Flock::lock(lock_file, nix::fcntl::FlockArg::LockExclusiveNonblock)
                .map_err(|(_, error)| io::Error::from(error))?;

        match fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_socket() && m.uid() == owner_uid() => {
                match connect(path, Duration::from_millis(200)) {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "journal socket is already in use; keeping TCP access",
                        ));
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                        ) =>
                    {
                        fs::remove_file(path)?
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "journal socket path is occupied",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let listener = std::os::unix::net::UnixListener::bind(path)?;
        let inode = fs::symlink_metadata(path)?.ino();
        let lease = Self {
            listener,
            path: path.to_owned(),
            inode,
            _directory_lock: owns_record.then_some(directory_lock),
            owns_record,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        lease.listener.set_nonblocking(true)?;
        Ok(lease)
    }
    pub fn publish(&self, record: &RuntimeRecord) -> io::Result<()> {
        let dest = record_path(&self.path);
        let temp = dest.with_file_name(format!(".journal-{}.tmp", record.generation));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        let result = (|| {
            serde_json::to_writer(&mut file, record)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp, &dest)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}
impl SocketLease {
    pub fn cleanup(&self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = fs::remove_file(&self.path);
            if self.owns_record {
                let _ = fs::remove_file(record_path(&self.path));
            }
        }
    }
}

impl Drop for SocketLease {
    fn drop(&mut self) {
        self.cleanup();
    }
}

pub fn socket_agent(path: &Path, timeout: Duration) -> ureq::Agent {
    let config = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .proxy(None)
        .max_redirects(0)
        .timeout_connect(Some(timeout))
        .timeout_global(Some(timeout))
        .build();
    ureq::Agent::with_parts(
        config,
        transport::UnixConnector(path.to_owned()),
        ureq::unversioned::resolver::DefaultResolver::default(),
    )
}

pub fn ping(socket: Option<&Path>, port: u16, journal: &Path, generation: Option<&str>) -> bool {
    let timeout = Duration::from_millis(250);
    let agent = match socket {
        Some(path) => socket_agent(path, timeout),
        None => ureq::Agent::new_with_config(
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .proxy(None)
                .timeout_global(Some(timeout))
                .build(),
        ),
    };
    let url = match socket {
        Some(_) => format!("http://localhost{RUNTIME_ROUTE}"),
        None => format!("http://127.0.0.1:{port}{RUNTIME_ROUTE}"),
    };
    let Ok(mut response) = agent
        .get(&url)
        .header("User-Agent", "solstone-internal")
        .call()
    else {
        return false;
    };
    if response.status() != 200 {
        return false;
    }
    let Ok(bytes) = response.body_mut().with_config().limit(16384).read_to_vec() else {
        return false;
    };
    let Ok(record) = serde_json::from_slice::<RuntimeRecord>(&bytes) else {
        return false;
    };
    record.journal_root == fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned())
        && generation.is_none_or(|expected| expected == record.generation)
}

/// Supervised checks never discover another root's socket through the CLI default.
pub fn journal_is_up(journal: &Path) -> bool {
    let root = fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let journal = root.as_path();
    let socket = socket_for_journal(journal).ok().flatten();
    if let Some(path) = socket.as_deref() {
        let record = fs::read(record_path(path))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<RuntimeRecord>(&bytes).ok());
        if let Some(record) = record.filter(|r| r.journal_root == journal)
            && ping(
                Some(path),
                record.tcp_port,
                journal,
                Some(&record.generation),
            )
        {
            return true;
        }
    }
    let port = fs::read_to_string(journal.join("health/convey.port"))
        .ok()
        .and_then(|s| s.trim().parse().ok());
    port.is_some_and(|port| ping(None, port, journal, None))
}

pub fn socket_belongs_to_journal(socket: &Path, journal: &Path) -> bool {
    fs::read(record_path(socket))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<RuntimeRecord>(&bytes).ok())
        .is_some_and(|record| record.journal_root == journal)
}

/// Resolve the endpoint already published for this root, never another journal.
pub fn internal_environment(
    journal: &Path,
) -> std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString> {
    let root = fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned());
    let socket = socket_for_journal(journal).ok().flatten().filter(|path| {
        owned_socket(path)
            && fs::read(record_path(path))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<RuntimeRecord>(&bytes).ok())
                .is_some_and(|record| record.journal_root == root)
    });
    let mut environment = std::collections::BTreeMap::new();
    environment.insert(INTERNAL_ROOT_ENV.into(), root.into_os_string());
    if let Ok(port) = fs::read_to_string(journal.join("health/convey.port")) {
        environment.insert(INTERNAL_PORT_ENV.into(), port.trim().into());
    }

    if let Some(socket) = socket {
        environment.insert(SOCKET_ENV.into(), socket.into_os_string());
    } else {
        environment.insert(SOCKET_ENV.into(), std::ffi::OsString::new());
        if let Ok(port) = fs::read_to_string(journal.join("health/convey.port")) {
            environment.insert("SOLSTONE_CONVEY_PORT".into(), port.trim().into());
        }
    }
    environment
}

pub fn prepare_child(
    command: &mut std::process::Command,
    environment: &std::collections::BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) {
    if environment.contains_key(std::ffi::OsStr::new(SOCKET_ENV)) {
        command.env_remove("SOLSTONE_CONVEY_PORT");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pathname_limits_leave_room_for_nul() {
        let limit = if cfg!(target_os = "macos") { 104 } else { 108 };
        assert!(validate_socket_path(Path::new(&format!("/{}", "a".repeat(limit - 2)))).is_ok());
        assert!(validate_socket_path(Path::new(&format!("/{}", "a".repeat(limit - 1)))).is_err());
        assert!(validate_socket_path(Path::new("relative.sock")).is_err());
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod socket_tests {
    use super::*;
    use crate::fresh_generation;
    #[test]
    fn stale_socket_is_reclaimed_and_live_socket_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("journal.sock");
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        let lease = SocketLease::bind(&path).unwrap();
        let inode = fs::metadata(&path).unwrap().ino();
        assert!(SocketLease::bind(&path).is_err());
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        let record = RuntimeRecord {
            generation: fresh_generation(),
            pid: std::process::id(),
            process_start_time: serde_json::json!(123),
            journal_root: fs::canonicalize(dir.path()).unwrap(),
            tcp_port: 15015,
            socket_path: path.clone(),
        };
        lease.publish(&record).unwrap();
        let published = record_path(&path);
        assert_eq!(fs::metadata(&published).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            serde_json::from_slice::<RuntimeRecord>(&fs::read(&published).unwrap()).unwrap(),
            record
        );
        drop(lease);
        assert!(!path.exists());
        assert!(!published.exists());
    }

    #[test]
    fn an_existing_nonprivate_directory_is_refused_without_changing_its_mode() {
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().join("journal.sock");
        assert!(SocketLease::bind(&path).is_err());
        assert_eq!(fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o755);
        assert!(!path.exists());
    }

    #[test]
    fn socket_connector_round_trips_http_and_a_stale_generation_is_not_ready() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.path().join("journal.sock");
        let lease = SocketLease::bind(&path).unwrap();
        lease.listener.set_nonblocking(false).unwrap();
        let record = RuntimeRecord {
            generation: fresh_generation(),
            pid: std::process::id(),
            process_start_time: serde_json::json!(123),
            journal_root: fs::canonicalize(dir.path()).unwrap(),
            tcp_port: 15015,
            socket_path: path.clone(),
        };
        let expected = record.generation.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..3 {
                let (mut stream, _) = lease.listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|part| part == b"\r\n\r\n") {
                    let mut chunk = [0; 4096];
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0 && request.len() + n <= 16384);
                    request.extend_from_slice(&chunk[..n]);
                }
                assert!(
                    std::str::from_utf8(&request)
                        .unwrap()
                        .starts_with("GET /_solstone/local-runtime HTTP/1.1")
                );
                let body = serde_json::to_vec(&record).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(&body).unwrap();
            }
        });
        assert!(ping(Some(&path), 1, dir.path(), Some(&expected)));
        assert!(!ping(Some(&path), 1, dir.path(), Some("stale")));
        assert!(!ping(
            Some(&path),
            1,
            Path::new("/other-journal"),
            Some(&expected)
        ));
        server.join().unwrap();
    }
}
