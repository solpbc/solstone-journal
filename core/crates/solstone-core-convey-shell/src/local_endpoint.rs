// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    Json, Router,
    extract::{Request, State},
    middleware::Next,
    response::Response,
    routing::get,
};
use serde::Serialize;
use solstone_core_local_endpoint::{RuntimeRecord, SocketLease};

#[derive(Clone, Default)]
pub(crate) struct TcpPrograms(Arc<Mutex<BTreeMap<String, Program>>>);
#[derive(Clone, Serialize)]
struct Program {
    program: String,
    count: u64,
    last_seen: u64,
}

impl TcpPrograms {
    fn observe(&self, headers: &axum::http::HeaderMap, now: u64) {
        if headers
            .keys()
            .any(|name| name.as_str().starts_with("sec-fetch-"))
        {
            return;
        }
        let product = headers
            .get("user-agent")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or("unknown");
        let product = product.to_owned();
        let mut programs = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let row = programs.entry(product.clone()).or_insert(Program {
            program: product,
            count: 0,
            last_seen: 0,
        });
        row.count += 1;
        row.last_seen = now;
    }
}

pub(crate) async fn count_tcp(
    State(programs): State<TcpPrograms>,
    request: Request,
    next: Next,
) -> Response {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    programs.observe(request.headers(), now);
    next.run(request).await
}

pub(crate) fn routes(programs: TcpPrograms, record: Arc<Mutex<RuntimeRecord>>) -> Router {
    Router::new()
        .route(
            "/app/health/api/local-programs",
            get(move || {
                let programs = programs.clone();
                async move {
                    let items = programs
                        .0
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .values()
                        .cloned()
                        .collect::<Vec<_>>();
                    (
                        [(axum::http::header::CACHE_CONTROL, "no-store")],
                        Json(serde_json::json!({"total": items.len(), "items": items})),
                    )
                }
            }),
        )
        .route(
            solstone_core_local_endpoint::RUNTIME_ROUTE,
            get(move || {
                let record = record.clone();
                async move { Json(record.lock().unwrap_or_else(|e| e.into_inner()).clone()) }
            }),
        )
}

pub(crate) fn runtime_record(journal: &Path, port: u16, socket: Option<&Path>) -> RuntimeRecord {
    use solstone_core_system::process::{
        InspectResult, ProcessInstanceSource, SystemProcessInstanceSource,
    };
    let birth = match SystemProcessInstanceSource.inspect(std::process::id()) {
        InspectResult::Present { instance, .. } => {
            serde_json::to_value(instance.birth).unwrap_or_default()
        }
        _ => serde_json::Value::Null,
    };
    RuntimeRecord {
        generation: std::env::var(solstone_core_local_endpoint::GENERATION_ENV)
            .unwrap_or_else(|_| solstone_core_local_endpoint::fresh_generation()),
        pid: std::process::id(),
        process_start_time: birth,
        journal_root: std::fs::canonicalize(journal).unwrap_or_else(|_| journal.to_owned()),
        tcp_port: port,
        socket_path: socket.unwrap_or_else(|| Path::new("")).to_owned(),
    }
}

pub(crate) struct OwnerSocket {
    pub lease: SocketLease,
    pub task: tokio::task::JoinHandle<()>,
}
impl OwnerSocket {
    pub fn shutdown(&self) {
        self.task.abort();
        self.lease.cleanup();
    }
}
impl Drop for OwnerSocket {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn start(journal: &Path, record: &RuntimeRecord, router: Router) -> Option<OwnerSocket> {
    let result = (|| -> std::io::Result<Option<OwnerSocket>> {
        let Some(path) = solstone_core_local_endpoint::socket_for_journal(journal)? else {
            return Ok(None);
        };
        let lease = SocketLease::bind(&path)?;
        let listener = tokio::net::UnixListener::from_std(lease.listener.try_clone()?)?;
        lease.publish(record)?;
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(error) => {
                        log::warn!("journal socket accept failed: {error}");
                        break;
                    }
                };
                if !stream
                    .peer_cred()
                    .is_ok_and(|peer| peer.uid() == solstone_core_local_endpoint::owner_uid())
                {
                    continue;
                }
                let router = router.clone();
                tokio::spawn(async move {
                    use solstone_core_convey_http::{
                        identity::AccessBasis,
                        serve::{serve_connection, tcp_builder},
                    };
                    if let Err(error) =
                        serve_connection(stream, router, AccessBasis::Localhost, &tcp_builder())
                            .await
                    {
                        log::debug!("journal socket connection failed: {error}");
                    }
                });
            }
        });
        Ok(Some(OwnerSocket { lease, task }))
    })();
    match result {
        Ok(socket) => socket,
        Err(error) => {
            log::warn!("journal socket unavailable; keeping TCP access: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_fetches_are_excluded_and_programs_share_the_product_token() {
        let programs = TcpPrograms::default();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("user-agent", "a-client/1.2 details".parse().unwrap());
        programs.observe(&headers, 1);
        headers.insert("user-agent", "a-client/1.2 other".parse().unwrap());
        programs.observe(&headers, 2);
        headers.insert("sec-fetch-mode", "cors".parse().unwrap());
        programs.observe(&headers, 3);
        let rows = programs.0.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows["a-client/1.2"].count, 2);
        assert_eq!(rows["a-client/1.2"].last_seen, 2);
    }
}
