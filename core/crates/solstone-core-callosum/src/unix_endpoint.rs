// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owned local addresses for embedded Unix clients. Named desktop sockets
//! remain a separate transport when no local address was selected.

use std::{
    collections::HashMap,
    io,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use solstone_core_journal_io::JournalRoot;

type Accept = dyn Fn(UnixStream) -> io::Result<()> + Send + Sync;
type Active = dyn Fn() -> bool + Send + Sync;

struct LocalConnector {
    root: Arc<JournalRoot>,
    accept: Box<Accept>,
    active: Box<Active>,
}

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<LocalConnector>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<LocalConnector>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registry_error<T>(_: T) -> io::Error {
    io::Error::from(io::ErrorKind::Other)
}

fn find(path: &Path) -> io::Result<Option<Arc<LocalConnector>>> {
    if let Some(connector) = registry()
        .lock()
        .map_err(registry_error)?
        .get(path)
        .and_then(Weak::upgrade)
    {
        return Ok(Some(connector));
    }
    // Only lookup metadata is normalized. Authority always comes from the
    // registered original root, which is revalidated before use.
    if path.file_name() != Some(std::ffi::OsStr::new("callosum.sock")) {
        return Ok(None);
    }
    let Some(health) = path
        .parent()
        .filter(|p| p.file_name() == Some(std::ffi::OsStr::new("health")))
    else {
        return Ok(None);
    };
    let Some(journal) = health.parent() else {
        return Ok(None);
    };
    let Ok(canonical) = std::fs::canonicalize(journal) else {
        return Ok(None);
    };
    let key = canonical.join("health/callosum.sock");
    Ok(registry()
        .lock()
        .map_err(registry_error)?
        .get(&key)
        .and_then(Weak::upgrade))
}

/// A client's selected address. Once local transport is selected, losing its
/// listener never authorizes a filesystem fallback or a different root.
#[derive(Clone)]
pub struct UnixEndpoint {
    path: PathBuf,
    local_root: Arc<OnceLock<Arc<JournalRoot>>>,
}

impl std::fmt::Debug for UnixEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnixEndpoint")
            .field("path", &self.path)
            .field("local", &self.local_root.get().is_some())
            .finish()
    }
}

impl UnixEndpoint {
    pub fn new(path: impl AsRef<Path>) -> Self {
        let endpoint = Self {
            path: path.as_ref().to_owned(),
            local_root: Arc::new(OnceLock::new()),
        };
        if let Ok(Some(connector)) = find(&endpoint.path) {
            let _ = endpoint.local_root.set(Arc::clone(&connector.root));
        }
        endpoint
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `None` permits an ordinary desktop named connection. A matched local
    /// refusal is an error. iOS never falls through to a named socket.
    pub fn connect_local(&self) -> io::Result<Option<UnixStream>> {
        if let Some(root) = self.local_root.get() {
            root.revalidate_canonical_binding()
                .map_err(io::Error::other)?;
        }
        let Some(connector) = find(&self.path)? else {
            return if self.local_root.get().is_some() || cfg!(target_os = "ios") {
                Err(io::Error::from(io::ErrorKind::NotConnected))
            } else {
                Ok(None)
            };
        };
        let root = self.local_root.get_or_init(|| Arc::clone(&connector.root));
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        connector
            .root
            .revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        if root.identity() != connector.root.identity() || !(connector.active)() {
            return Err(io::Error::from(io::ErrorKind::NotConnected));
        }
        let (client, server) = UnixStream::pair()?;
        server.set_nonblocking(true)?;
        (connector.accept)(server)?;
        Ok(Some(client))
    }
}

/// Read only the actual local registration. This does not connect, start
/// work, or stand in for a successful service-status response.
pub fn local_unix_listener_status(path: &Path) -> io::Result<Option<bool>> {
    Ok(find(path)?.map(|connector| {
        connector.root.revalidate_canonical_binding().is_ok() && (connector.active)()
    }))
}

#[cfg(feature = "wire")]
pub(crate) struct LocalRegistration {
    key: PathBuf,
    connector: Arc<LocalConnector>,
}

#[cfg(feature = "wire")]
impl LocalRegistration {
    #[cfg(any(target_os = "macos", target_os = "ios", test, feature = "test-hooks"))]
    pub(crate) fn register(
        root: Arc<JournalRoot>,
        active: impl Fn() -> bool + Send + Sync + 'static,
        accept: impl Fn(UnixStream) -> io::Result<()> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let key = root.canonical_path().join("health/callosum.sock");
        let mut registry = registry().lock().map_err(registry_error)?;
        registry.retain(|_, entry| entry.strong_count() != 0);
        if registry.get(&key).and_then(Weak::upgrade).is_some() {
            return Err(io::Error::from(io::ErrorKind::AddrInUse));
        }
        root.revalidate_canonical_binding()
            .map_err(io::Error::other)?;
        let connector = Arc::new(LocalConnector {
            root,
            active: Box::new(active),
            accept: Box::new(accept),
        });
        registry.insert(key.clone(), Arc::downgrade(&connector));
        Ok(Self { key, connector })
    }

    pub(crate) fn close(&self) {
        if let Ok(mut registry) = registry().lock()
            && registry
                .get(&self.key)
                .is_some_and(|entry| entry.ptr_eq(&Arc::downgrade(&self.connector)))
        {
            registry.remove(&self.key);
        }
    }
}

#[cfg(feature = "wire")]
impl Drop for LocalRegistration {
    fn drop(&mut self) {
        self.close();
    }
}
