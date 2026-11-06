// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows: the owner's private pipe, and the account check on the browser port.
//!
//! Two facts keep a second account out. The pipe's name carries this account's
//! SID and its DACL admits only that SID. And Windows can say which process
//! holds the far end of a loopback TCP connection, so the browser port can ask
//! whether that process runs as this account. Another account's process can't
//! be opened from here, so the lookup fails, and failing means no.
//!
//! The connection state column is never read: it was measured to mislabel a
//! closing client, and it says nothing about who the client is.
#![allow(unsafe_code)]

use std::io;
use std::mem::size_of;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, PipeMode, ServerOptions};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_PIPE_BUSY, GetLastError, LocalFree,
    NO_ERROR,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenMutexW, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::SOCKET_ENV;

const PIPE_PREFIX: &str = r"\\.\pipe\solstone-journal-";
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// This account's SID in binary form, read once.
fn owner_sid_bytes() -> io::Result<&'static [u8]> {
    static OWNER: OnceLock<Result<Vec<u8>, i32>> = OnceLock::new();
    OWNER
        .get_or_init(|| {
            // SAFETY: GetCurrentProcess returns a pseudo-handle valid for this call.
            current_token(unsafe { GetCurrentProcess() })
                .and_then(|token| token_sid(&token))
                .map_err(|error| error.raw_os_error().unwrap_or(-1))
        })
        .as_ref()
        .map(Vec::as_slice)
        .map_err(|code| io::Error::from_raw_os_error(*code))
}

fn current_token(process: *mut core::ffi::c_void) -> io::Result<OwnedHandle> {
    let mut token = null_mut();
    // SAFETY: process is a valid process handle and token points to writable storage.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 || token.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned an owned, non-null handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(token) })
}

fn token_sid(token: &OwnedHandle) -> io::Result<Vec<u8>> {
    let mut required = 0_u32;
    // SAFETY: the documented sizing call, with a live token and a null buffer.
    let first = unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            null_mut(),
            0,
            &mut required,
        )
    };
    // SAFETY: reads the error from the sizing call just above.
    if first != 0
        || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
        || (required as usize) < size_of::<TOKEN_USER>()
    {
        return Err(io::Error::last_os_error());
    }
    // A u64 buffer keeps TOKEN_USER's pointer field aligned.
    let mut buffer = vec![0_u64; (required as usize).div_ceil(size_of::<u64>())];
    // SAFETY: the buffer holds at least `required` bytes.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: GetTokenInformation wrote a TOKEN_USER at the start of the aligned buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    if user.User.Sid.is_null() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "null token SID"));
    }
    // SAFETY: the SID points into the live buffer, and its length comes from the SID itself.
    Ok(unsafe {
        std::slice::from_raw_parts(
            user.User.Sid.cast::<u8>(),
            GetLengthSid(user.User.Sid) as usize,
        )
    }
    .to_vec())
}

/// This account's SID as text, as a pipe name and a DACL write it.
pub fn owner_sid() -> io::Result<String> {
    let sid = owner_sid_bytes()?;
    let mut text = null_mut();
    // SAFETY: sid is a valid SID that outlives the call, and text points to writable storage.
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut text) } == 0
        || text.is_null()
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call returned a NUL-terminated UTF-16 LocalAlloc buffer. Its length is bounded
    // by that terminator, and it is released exactly once, after the copy.
    let result = unsafe {
        let length = (0..).take_while(|index| *text.add(*index) != 0).count();
        let copied = String::from_utf16(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        copied
    };
    result.map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "SID is not UTF-16"))
}

const JOURNAL_CLAIM: &str = r"Global\solstone-journal";
/// Asks only whether a named object exists and may be opened.
const SYNCHRONIZE: u32 = 0x0010_0000;

fn wide_text(text: &str) -> Vec<u16> {
    std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

/// Claim this computer's one journal for this account, for the life of this
/// process. Windows releases the claim when the process ends, however it ends.
///
/// False only when another account holds it: the object's default DACL refuses
/// a different account. A claim this account already holds, or a failure that
/// says nothing about another account, leaves the rest of startup to decide.
pub fn claim_journal_on_this_computer() -> bool {
    let name = wide_text(JOURNAL_CLAIM);
    // SAFETY: name is NUL-terminated, and a null attributes pointer means the default DACL.
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        // SAFETY: reads the error from the call just above.
        return unsafe { GetLastError() } != ERROR_ACCESS_DENIED;
    }
    // Held on purpose until this process ends.
    static CLAIM: OnceLock<usize> = OnceLock::new();
    let _ = CLAIM.set(handle as usize);
    true
}

/// Whether another account's journal holds this computer right now.
pub fn other_account_journal_running() -> bool {
    let name = wide_text(JOURNAL_CLAIM);
    // SAFETY: name is NUL-terminated.
    let handle = unsafe { OpenMutexW(SYNCHRONIZE, 0, name.as_ptr()) };
    if handle.is_null() {
        // SAFETY: reads the error from the call just above.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    // SAFETY: OpenMutexW returned an owned handle, closed here on drop.
    drop(unsafe { OwnedHandle::from_raw_handle(handle) });
    false
}

/// Hand a page to the default browser, directly and never through a shell.
pub fn open_in_browser(url: &str) -> bool {
    let (verb, file) = (wide_text("open"), wide_text(url));
    // SAFETY: both strings are NUL-terminated and outlive the call; the rest are optional.
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute reports success as a value above 32.
    result as usize > 32
}

/// The DACL of anything private to this account: the owner alone.
fn owner_only_descriptor(sid: &str) -> String {
    format!("D:P(A;;GA;;;{sid})")
}

/// The pipe this account's journal listens on, or none when turned off by env.
pub fn socket_path() -> io::Result<Option<PathBuf>> {
    match std::env::var_os(SOCKET_ENV) {
        Some(value) if value.is_empty() => Ok(None),
        Some(value) => Ok(Some(PathBuf::from(value))),
        None => Ok(Some(PathBuf::from(format!(
            "{PIPE_PREFIX}{}",
            owner_sid()?
        )))),
    }
}

/// The pipe for any journal this account runs. One journal runs on a computer at
/// a time, and the first to bind the name holds it.
pub fn socket_for_journal(_journal: &Path) -> io::Result<Option<PathBuf>> {
    socket_path()
}

/// A security descriptor that frees itself, built from SDDL text.
struct Descriptor(*mut core::ffi::c_void);

impl Descriptor {
    fn new(sddl: &str) -> io::Result<Self> {
        let wide = wide_text(sddl);
        let mut descriptor = null_mut();
        // SAFETY: wide is NUL-terminated, and descriptor points to writable storage.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertString...W with LocalAlloc, and released once here.
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// The owner's pipe: every instance is created private to this account.
pub struct PipeListener {
    name: String,
    next: NamedPipeServer,
}

impl PipeListener {
    /// Claim the name. Fails when anything already holds it.
    pub fn bind(name: &Path) -> io::Result<Self> {
        let name = name
            .to_str()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "pipe name isn't text"))?
            .to_owned();
        let next = create_instance(&name, true)?;
        Ok(Self { name, next })
    }

    /// Wait for a client. A fresh instance is ready before this returns, and
    /// after a failed connect too, so one bad client can't end the listener.
    pub async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        let connected = self.next.connect().await;
        let fresh = create_instance(&self.name, false)?;
        let instance = std::mem::replace(&mut self.next, fresh);
        connected.map(|()| instance)
    }
}

fn create_instance(name: &str, first: bool) -> io::Result<NamedPipeServer> {
    let descriptor = Descriptor::new(&owner_only_descriptor(&owner_sid()?))?;
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    };
    // SAFETY: attributes and its descriptor outlive this call, which copies them into the pipe.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .pipe_mode(PipeMode::Byte)
            .create_with_security_attributes_raw(
                name,
                (&raw mut attributes).cast::<core::ffi::c_void>(),
            )
    }
}

/// A complete answer from the journal over its pipe.
#[derive(Debug)]
pub struct PipeResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One HTTP request over the owner's pipe, from a program with no runtime of
/// its own. A response cut short, say by a journal restart, is an error rather
/// than a shorter body.
pub fn pipe_request(
    pipe: &Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<PipeResponse> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        tokio::time::timeout(timeout, exchange(pipe, method, path, body, timeout))
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "the journal did not answer"))?
    })
}

async fn exchange(
    pipe: &Path,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> io::Result<PipeResponse> {
    let deadline = Instant::now() + timeout;
    let mut client = loop {
        match ClientOptions::new().open(pipe) {
            Ok(client) => break client,
            Err(error)
                if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Err(error) => return Err(error),
        }
    };
    let body = body.unwrap_or_default();
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    client.write_all(head.as_bytes()).await?;
    client.write_all(body).await?;
    client.flush().await?;
    let mut received = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        if let Some(response) = parse_response(&received)? {
            return Ok(response);
        }
        if received.len() > MAX_RESPONSE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the journal's answer is too long",
            ));
        }
        let read = match client.read(&mut chunk).await {
            Ok(read) => read,
            // A pipe whose server has gone reports a broken pipe rather than EOF.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => 0,
            Err(error) => return Err(error),
        };
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the journal's answer stopped partway",
            ));
        }
        received.extend_from_slice(&chunk[..read]);
    }
}

/// The response, once all of it has arrived. Only Content-Length framing is
/// accepted: every route this client calls answers that way.
fn parse_response(received: &[u8]) -> io::Result<Option<PipeResponse>> {
    let invalid = |message: &'static str| io::Error::new(io::ErrorKind::InvalidData, message);
    let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") else {
        return Ok(None);
    };
    let head = std::str::from_utf8(&received[..end]).map_err(|_| invalid("response head"))?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| invalid("response status"))?;
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .ok_or_else(|| invalid("response length"))?;
    let body = &received[end + 4..];
    Ok((body.len() >= length).then(|| PipeResponse {
        status,
        body: body[..length].to_vec(),
    }))
}

/// Whether this account holds the far end of an accepted loopback connection.
/// Blocking, about 10 ms with thousands of connections open: call it off the
/// async runtime. Anything short of a positive answer is no.
pub fn connection_is_owner(server: SocketAddr, client: SocketAddr) -> bool {
    let Ok(owner) = owner_sid_bytes() else {
        return false;
    };
    let Ok(pids) = client_pids(server, client) else {
        return false;
    };
    !pids.is_empty() && pids.into_iter().all(|pid| process_is_owner(pid, owner))
}

fn process_is_owner(pid: u32, owner: &[u8]) -> bool {
    if pid == std::process::id() {
        return true;
    }
    if pid == 0 || pid == 4 {
        return false;
    }
    // SAFETY: OpenProcess takes no pointers; a null return is handled just below.
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if process.is_null() {
        return false;
    }
    // SAFETY: OpenProcess returned an owned, non-null handle.
    let process = unsafe { OwnedHandle::from_raw_handle(process) };
    current_token(process.as_raw_handle())
        .and_then(|token| token_sid(&token))
        .is_ok_and(|sid| sid == owner)
}

fn client_pids(server: SocketAddr, client: SocketAddr) -> io::Result<Vec<u32>> {
    let family = if server.is_ipv4() { AF_INET } else { AF_INET6 };
    Ok(rows(family)?
        .into_iter()
        // The client's row reads from its side: its address is local, ours is remote.
        .filter(|row| row.local == client && row.remote == server && row.pid != 0)
        .map(|row| row.pid)
        .collect())
}

struct TcpRow {
    local: SocketAddr,
    remote: SocketAddr,
    pid: u32,
}

fn port(raw: u32) -> u16 {
    u16::from_be(raw as u16)
}

fn rows(family: u16) -> io::Result<Vec<TcpRow>> {
    let table = tcp_table(family)?;
    let mut rows = Vec::new();
    if family == AF_INET {
        // SAFETY: tcp_table returned a buffer the API filled, aligned for this table.
        let header = unsafe { &*table.as_ptr().cast::<MIB_TCPTABLE_OWNER_PID>() };
        // SAFETY: the API wrote dwNumEntries rows after the count, inside the buffer.
        let entries = unsafe {
            std::slice::from_raw_parts::<MIB_TCPROW_OWNER_PID>(
                header.table.as_ptr(),
                header.dwNumEntries as usize,
            )
        };
        for row in entries {
            let address = |raw: u32| IpAddr::V4(Ipv4Addr::from(u32::from_be(raw)));
            rows.push(TcpRow {
                local: SocketAddr::new(address(row.dwLocalAddr), port(row.dwLocalPort)),
                remote: SocketAddr::new(address(row.dwRemoteAddr), port(row.dwRemotePort)),
                pid: row.dwOwningPid,
            });
        }
    } else {
        // SAFETY: as above, for the IPv6 table this family requested.
        let header = unsafe { &*table.as_ptr().cast::<MIB_TCP6TABLE_OWNER_PID>() };
        // SAFETY: as above.
        let entries = unsafe {
            std::slice::from_raw_parts::<MIB_TCP6ROW_OWNER_PID>(
                header.table.as_ptr(),
                header.dwNumEntries as usize,
            )
        };
        for row in entries {
            let address = |raw: [u8; 16]| IpAddr::V6(Ipv6Addr::from(raw));
            rows.push(TcpRow {
                local: SocketAddr::new(address(row.ucLocalAddr), port(row.dwLocalPort)),
                remote: SocketAddr::new(address(row.ucRemoteAddr), port(row.dwRemotePort)),
                pid: row.dwOwningPid,
            });
        }
    }
    Ok(rows)
}

fn tcp_table(family: u16) -> io::Result<Vec<u32>> {
    let mut size = 0_u32;
    // The table can grow between the sizing call and the read, so try a few times.
    for _ in 0..8 {
        let mut buffer = vec![0_u32; (size as usize).div_ceil(size_of::<u32>()).max(1)];
        // SAFETY: buffer holds at least `size` bytes, or size is 0 for the sizing call.
        let result = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                u32::from(family),
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        };
        if result == NO_ERROR {
            return Ok(buffer);
        }
        if result != ERROR_INSUFFICIENT_BUFFER {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
    }
    Err(io::Error::other("the TCP table kept growing"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_is_complete_only_at_its_declared_length() {
        let head = b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\n";
        assert!(parse_response(&head[..10]).unwrap().is_none());
        let mut partial = head.to_vec();
        partial.extend_from_slice(b"ab");
        assert!(parse_response(&partial).unwrap().is_none());
        partial.extend_from_slice(b"cd");
        let response = parse_response(&partial).unwrap().unwrap();
        assert_eq!(
            (response.status, response.body.as_slice()),
            (200, &b"abcd"[..])
        );
    }

    #[test]
    fn a_response_without_a_length_is_refused() {
        let head = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(parse_response(head).is_err());
    }

    #[test]
    fn a_claim_this_account_holds_is_not_another_accounts() {
        assert!(claim_journal_on_this_computer());
        assert!(!other_account_journal_running());
        assert!(claim_journal_on_this_computer());
    }

    #[test]
    fn this_process_is_its_own_owner() {
        assert!(process_is_owner(
            std::process::id(),
            owner_sid_bytes().unwrap()
        ));
    }

    #[test]
    fn system_and_idle_are_never_the_owner() {
        let owner = owner_sid_bytes().unwrap();
        assert!(!process_is_owner(0, owner));
        assert!(!process_is_owner(4, owner));
    }

    #[tokio::test]
    async fn the_pipe_serves_its_owner_and_a_second_claim_fails() {
        let name = PathBuf::from(format!(
            r"\\.\pipe\solstone-journal-test-{}",
            std::process::id()
        ));
        let mut listener = PipeListener::bind(&name).unwrap();
        assert!(PipeListener::bind(&name).is_err());
        let server = tokio::spawn(async move {
            let mut stream = listener.accept().await.unwrap();
            let mut chunk = [0_u8; 4096];
            let _ = stream.read(&mut chunk).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                .await
                .unwrap();
        });
        let client_name = name.clone();
        let response = tokio::task::spawn_blocking(move || {
            pipe_request(&client_name, "GET", "/", None, Duration::from_secs(5))
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (response.status, response.body.as_slice()),
            (200, &b"ok"[..])
        );
        server.await.unwrap();
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod network_tests {
    use super::*;

    #[test]
    fn a_loopback_client_in_this_account_is_recognized_on_both_families() {
        for bind in ["127.0.0.1:0", "[::1]:0"] {
            let listener = std::net::TcpListener::bind(bind).unwrap();
            let server_address = listener.local_addr().unwrap();
            let client = std::net::TcpStream::connect(server_address).unwrap();
            let (accepted, peer) = listener.accept().unwrap();
            assert!(connection_is_owner(server_address, peer));
            // A tuple nobody holds is no.
            let stranger = SocketAddr::new(peer.ip(), peer.port().wrapping_add(1));
            assert!(!connection_is_owner(server_address, stranger));
            drop((client, accepted));
        }
    }
}
