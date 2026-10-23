// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The embedding app owns both ends of each response pipe. No network listener.

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use solstone_core_convey_shell::EmbeddedConveyHandle;
use solstone_core_journal_io::JournalRoot;
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    sync::{Arc, Mutex, mpsc},
};
use tokio::{io::unix::AsyncFd, runtime::Handle, sync::watch, task::JoinHandle};

pub struct OwnerResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

struct ResponseTask {
    cancel: watch::Sender<bool>,
    task: JoinHandle<()>,
}

#[derive(Default)]
pub(crate) struct OwnerResponses {
    tasks: Mutex<HashMap<u64, ResponseTask>>,
}

fn request(
    method: &str,
    uri: &str,
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<Request<Body>, String> {
    if !matches!(
        method,
        "GET" | "HEAD" | "POST" | "PUT" | "PATCH" | "DELETE" | "OPTIONS"
    ) || uri.len() > 8192
        || body.len() > 256 * 1024
        || headers
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            > 16 * 1024
    {
        return Err("bounded owner request required".into());
    }
    let uri: axum::http::Uri = uri
        .parse()
        .map_err(|e: axum::http::uri::InvalidUri| e.to_string())?;
    if uri.scheme().is_some()
        || uri.authority().is_some()
        || !uri.path().starts_with('/')
        || uri.path().starts_with("//")
    {
        return Err("owner request requires an origin-form path".into());
    }
    let mut request = Request::builder().method(method).uri(uri);
    for (key, value) in headers {
        // The embedding app supplies representation metadata, never an access basis.
        if !matches!(
            key.to_ascii_lowercase().as_str(),
            "content-type"
                | "accept"
                | "range"
                | "if-none-match"
                | "if-modified-since"
                | "last-event-id"
        ) {
            return Err("owner request header not admitted".into());
        }
        request = request.header(key, value);
    }
    request.body(Body::from(body)).map_err(|e| e.to_string())
}

impl OwnerResponses {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        &self,
        runtime: &Handle,
        router: Arc<EmbeddedConveyHandle>,
        root: Arc<JournalRoot>,
        mut stop: watch::Receiver<bool>,
        id: u64,
        method: &str,
        uri: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
        sink: File,
    ) -> Result<mpsc::Receiver<Result<OwnerResponseHead, String>>, String> {
        let request = request(method, uri, headers, body)?;
        let mut tasks = self
            .tasks
            .lock()
            .map_err(|_| "owner response registry unavailable")?;
        tasks.retain(|_, task| !task.task.is_finished());
        if *stop.borrow() || tasks.contains_key(&id) || tasks.len() >= 32 {
            return Err("owner responses stopped or busy".into());
        }
        let _entered = runtime.enter();
        let sink = AsyncFd::new(sink).map_err(|e| e.to_string())?;
        let (sender, receiver) = mpsc::channel();
        let (cancel, mut cancelled) = watch::channel(false);
        let task = runtime.spawn(async move {
            if let Err(error) = root.revalidate_canonical_binding() {
                let _ = sender.send(Err(error.to_string()));
                return;
            }
            let response = tokio::select! {
                biased;
                _ = stop.changed() => return,
                _ = cancelled.changed() => return,
                response = router.request_owner(request) => response,
            };
            let head = OwnerResponseHead {
                status: response.status().as_u16(),
                headers: response
                    .headers()
                    .iter()
                    .filter_map(|(key, value)| {
                        value
                            .to_str()
                            .ok()
                            .map(|value| (key.to_string(), value.to_owned()))
                    })
                    .collect(),
            };
            if sender.send(Ok(head)).is_err() {
                return;
            }
            let mut body = response.into_body();
            loop {
                let frame = tokio::select! {
                    biased;
                    _ = stop.changed() => return,
                    _ = cancelled.changed() => return,
                    frame = body.frame() => frame,
                };
                let Some(Ok(frame)) = frame else {
                    return;
                };
                let Ok(bytes) = frame.into_data() else {
                    continue;
                };
                let mut remaining = bytes.as_ref();
                while !remaining.is_empty() {
                    let ready = tokio::select! {
                        biased;
                        _ = stop.changed() => return,
                        _ = cancelled.changed() => return,
                        ready = sink.writable() => ready,
                    };
                    let Ok(mut ready) = ready else {
                        return;
                    };
                    match ready.try_io(|sink| sink.get_ref().write(remaining)) {
                        Ok(Ok(0)) | Ok(Err(_)) => return,
                        Ok(Ok(written)) => remaining = &remaining[written..],
                        Err(_) => continue,
                    }
                }
            }
        });
        tasks.insert(id, ResponseTask { cancel, task });
        Ok(receiver)
    }

    pub fn cancel(&self, id: u64) {
        if let Some(task) = self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&id)
        {
            task.cancel.send_replace(true);
        }
    }

    pub async fn drain(&self) -> Result<(), ()> {
        let tasks: Vec<_> = self
            .tasks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .drain()
            .map(|(_, task)| task)
            .collect();
        for task in &tasks {
            task.cancel.send_replace(true);
        }
        for task in tasks {
            task.task.await.map_err(|_| ())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn representation_is_preserved_and_authority_cannot_be_supplied() {
        let body = b"{\"text\":\"hello\"}".to_vec();
        let form = request(
            "POST",
            "/init/finalize",
            &[("content-type".into(), "text/plain".into())],
            body.clone(),
        )
        .unwrap();
        assert_eq!(form.headers()["content-type"], "text/plain");
        let json = request(
            "POST",
            "/init/finalize",
            &[("content-type".into(), "application/json".into())],
            body,
        )
        .unwrap();
        assert_eq!(json.headers()["content-type"], "application/json");
        for uri in [
            "https://outside.invalid/",
            "//outside.invalid/",
            "outside.invalid:443",
        ] {
            assert!(request("GET", uri, &[], vec![]).is_err());
        }
        assert!(
            request(
                "GET",
                "/",
                &[("authorization".into(), "Bearer other".into())],
                vec![]
            )
            .is_err()
        );
        assert!(request("POST", "/", &[], vec![0; 256 * 1024 + 1]).is_err());
        assert!(request("CONNECT", "/", &[], vec![]).is_err());
    }
}
