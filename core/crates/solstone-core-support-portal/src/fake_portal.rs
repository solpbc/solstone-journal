// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! In-process transport fake used by the portal client's unit tests.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::client::{MultipartPart, PortalResponse, PortalTransport};
use crate::errors::PortalClientError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RequestLog {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Option<String>,
}

pub(crate) struct StubTransport {
    base: String,
    replies: VecDeque<PortalResponse>,
    log: Arc<Mutex<Vec<RequestLog>>>,
    pub(crate) multipart_bodies: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl StubTransport {
    pub(crate) fn new(
        base: impl Into<String>,
        replies: Vec<PortalResponse>,
    ) -> (Self, Arc<Mutex<Vec<RequestLog>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                base: base.into(),
                replies: replies.into(),
                log: log.clone(),
                multipart_bodies: Arc::new(Mutex::new(Vec::new())),
            },
            log,
        )
    }

    fn reply(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<String>,
    ) -> Result<PortalResponse, PortalClientError> {
        let path = url.strip_prefix(&self.base).unwrap_or(url).to_owned();
        self.log.lock().expect("log lock").push(RequestLog {
            method: method.to_owned(),
            path,
            headers: headers.to_vec(),
            body,
        });
        self.replies
            .pop_front()
            .ok_or_else(|| PortalClientError::Transport {
                message: "fake has no response".to_owned(),
            })
    }
}

impl PortalTransport for StubTransport {
    fn get(
        &mut self,
        url: &str,
        headers: &[(String, String)],
    ) -> Result<PortalResponse, PortalClientError> {
        self.reply("GET", url, headers, None)
    }
    fn post_json(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        body: &str,
    ) -> Result<PortalResponse, PortalClientError> {
        self.reply("POST", url, headers, Some(body.to_owned()))
    }
    fn post_multipart(
        &mut self,
        url: &str,
        headers: &[(String, String)],
        files: &[MultipartPart],
    ) -> Result<PortalResponse, PortalClientError> {
        self.multipart_bodies
            .lock()
            .expect("body lock")
            .push(files.iter().flat_map(|part| part.bytes.clone()).collect());
        self.reply("POST", url, headers, None)
    }
}

pub(crate) struct HttpReply {
    pub(crate) status: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

pub(crate) struct LoopbackPortal {
    base_url: String,
    log: Arc<Mutex<Vec<RequestLog>>>,
    stop: Arc<AtomicBool>,
    wake: std::net::SocketAddr,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackPortal {
    pub(crate) fn new(replies: Vec<HttpReply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fake");
        listener.set_nonblocking(true).expect("nonblocking fake");
        let address = listener.local_addr().expect("loopback address");
        let log = Arc::new(Mutex::new(Vec::new()));
        let thread_log = log.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let thread = thread::spawn(move || {
            let mut replies: VecDeque<_> = replies.into();
            while !thread_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        if thread_stop.load(Ordering::Acquire) {
                            break;
                        }
                        let request = read_request(&mut stream);
                        thread_log.lock().expect("loopback log").push(request);
                        let reply = replies.pop_front().unwrap_or(HttpReply {
                            status: 500,
                            headers: Vec::new(),
                            body: "unexpected request".to_owned(),
                        });
                        write_reply(&mut stream, reply);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            log,
            stop,
            wake: address,
            thread: Some(thread),
        }
    }

    pub(crate) fn url(&self) -> &str {
        &self.base_url
    }
    pub(crate) fn log(&self) -> Arc<Mutex<Vec<RequestLog>>> {
        self.log.clone()
    }
}

impl Drop for LoopbackPortal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Ok(stream) = TcpStream::connect(self.wake) {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().expect("loopback fake thread");
        }
    }
}

fn read_request(stream: &mut TcpStream) -> RequestLog {
    let mut bytes = Vec::new();
    let mut buffer = [0; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).expect("read loopback request");
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split("\r\n");
    let start = lines.next().unwrap_or_default();
    let mut words = start.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let path = words.next().unwrap_or_default().to_owned();
    let headers = lines
        .filter_map(|line| {
            line.split_once(':')
                .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
        })
        .collect();
    RequestLog {
        method,
        path,
        headers,
        body: None,
    }
}

fn write_reply(stream: &mut TcpStream, reply: HttpReply) {
    let reason = match reply.status {
        200 => "OK",
        302 => "Found",
        401 => "Unauthorized",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "Response",
    };
    let mut response = format!(
        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (name, value) in reply.headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str("\r\n");
    response.push_str(&reply.body);
    stream
        .write_all(response.as_bytes())
        .expect("write loopback response");
}
