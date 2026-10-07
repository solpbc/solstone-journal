// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Whether the paired-device door listens on the local network or only on
//! this computer.
//!
//! Windows lets only an administrator allow an app to take connections from
//! the network, and it asks the moment a program first listens on one. So on
//! Windows the door listens only on this computer until the owner chooses
//! something that needs the network: pairing a device over the network, or the
//! LAN agent door. Same-computer pairing and the relay
//! both reach the door over loopback, so neither needs the network.
//!
//! The choice is `pairing.local_network` in `config/journal.json`. Without one,
//! macOS and Linux keep listening on the network, as they always have. On
//! Windows a journal that already reached past this computer — a device paired
//! from elsewhere, or the LAN agent door on — keeps the network, and the
//! carried-over answer is saved the first time the door starts, so nothing
//! flips later when the ledger changes.
//!
//! The LAN agent door also needs the door open, so while it is on the door
//! counts as open too, whatever is saved;
//! a watcher rebinds the door when either changes.
//!
//! These routes sit on the loopback listener alone. Opening the network raises
//! a prompt on this computer's screen, so a paired device cannot ask for it.

use std::path::Path;
use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use solstone_core_journal_config::{lan_door_enabled, read_journal_config};
use solstone_core_journal_config_write::{JournalConfigMutation, mutate_journal_config};
use solstone_core_sol_link::ledger::{AuthorizedClientsRead, read_authorized_clients};

use crate::door::DoorLifecycle;
use crate::network::{NETWORK_ROUTE_PREFIXES, refusal};

/// The `pairing` key that holds the owner's choice.
pub(crate) const CONFIG_KEY: &str = "local_network";
/// The ledger's mark for a device paired from this same computer.
const SAME_MACHINE_NETWORK: &str = "home";

/// Where a resolved answer came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LocalNetworkSource {
    /// The owner's saved choice, including a saved carried-over answer.
    Saved,
    /// Nothing saved, so the platform default applies.
    Default,
    /// Nothing saved, and this journal already reached past this computer.
    CarriedOver,
    /// Agents on your network is on, which opens the door whatever is saved.
    AgentsOnNetwork,
}

impl LocalNetworkSource {
    fn as_wire(self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Default => "default",
            Self::CarriedOver => "carried_over",
            Self::AgentsOnNetwork => "agents_on_network",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LocalNetworkState {
    pub open: bool,
    pub source: LocalNetworkSource,
}

/// Whether the door listens on the network when nothing is saved.
pub(crate) const fn platform_default_open() -> bool {
    !cfg!(windows)
}

/// Read the effective choice without writing anything.
pub(crate) fn resolve(journal_root: &Path) -> LocalNetworkState {
    resolve_with(journal_root, platform_default_open())
}

fn resolve_with(journal_root: &Path, default_open: bool) -> LocalNetworkState {
    with_agents_on_network(journal_root, resolve_choice(journal_root, default_open))
}

/// Agents on your network keeps the door open while it is on.
fn with_agents_on_network(journal_root: &Path, state: LocalNetworkState) -> LocalNetworkState {
    if !state.open && lan_door_on(journal_root) {
        return LocalNetworkState {
            open: true,
            source: LocalNetworkSource::AgentsOnNetwork,
        };
    }
    state
}

pub(crate) fn lan_door_on(journal_root: &Path) -> bool {
    read_journal_config(journal_root)
        .map(|read| lan_door_enabled(&read))
        .unwrap_or(false)
}

/// The owner's choice, or the default it stands in for, before agents on your
/// network is considered.
fn resolve_choice(journal_root: &Path, default_open: bool) -> LocalNetworkState {
    if let Some(open) = saved_choice(journal_root) {
        return LocalNetworkState {
            open,
            source: LocalNetworkSource::Saved,
        };
    }
    if default_open {
        return LocalNetworkState {
            open: true,
            source: LocalNetworkSource::Default,
        };
    }
    if reached_past_this_computer(journal_root) {
        LocalNetworkState {
            open: true,
            source: LocalNetworkSource::CarriedOver,
        }
    } else {
        LocalNetworkState {
            open: false,
            source: LocalNetworkSource::Default,
        }
    }
}

/// Resolve for a door start, saving a Windows answer the first time so a later
/// ledger change cannot flip it. A failed save still returns the answer.
pub(crate) fn resolve_for_door(journal_root: &Path) -> LocalNetworkState {
    resolve_for_door_with(journal_root, platform_default_open())
}

fn resolve_for_door_with(journal_root: &Path, default_open: bool) -> LocalNetworkState {
    let choice = resolve_choice(journal_root, default_open);
    if choice.source != LocalNetworkSource::Saved
        && !default_open
        && let Err(error) = save(journal_root, choice.open)
    {
        log::warn!("paired-device door could not save its local-network answer: {error}");
    }
    with_agents_on_network(journal_root, choice)
}

fn saved_choice(journal_root: &Path) -> Option<bool> {
    let read = read_journal_config(journal_root).ok()?;
    read.config?
        .get("pairing")?
        .as_object()?
        .get(CONFIG_KEY)?
        .as_bool()
}

/// A device paired from elsewhere, or the LAN agent door on. An unreadable
/// ledger counts too: never cut a live owner off on a guess.
fn reached_past_this_computer(journal_root: &Path) -> bool {
    if lan_door_on(journal_root) {
        return true;
    }
    match read_authorized_clients(&journal_root.join("link/authorized_clients.json")) {
        AuthorizedClientsRead::Present(entries) => entries
            .iter()
            .any(|entry| entry.network.as_deref() != Some(SAME_MACHINE_NETWORK)),
        AuthorizedClientsRead::Missing => false,
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => true,
    }
}

/// Save the owner's choice. Returns whether the saved value changed.
pub(crate) fn save(
    journal_root: &Path,
    open: bool,
) -> Result<bool, solstone_core_journal_config_write::ConfigMutationError> {
    mutate_journal_config(journal_root, Default::default(), |config| {
        let current = config
            .get("pairing")
            .and_then(Value::as_object)
            .and_then(|pairing| pairing.get(CONFIG_KEY))
            .and_then(Value::as_bool);
        let changed = current != Some(open);
        if changed {
            if !config.get("pairing").is_some_and(Value::is_object) {
                config.insert("pairing".to_owned(), Value::Object(Default::default()));
            }
            config
                .get_mut("pairing")
                .and_then(Value::as_object_mut)
                .expect("pairing object inserted")
                .insert(CONFIG_KEY.to_owned(), Value::Bool(open));
        }
        JournalConfigMutation {
            changed,
            value: changed,
        }
    })
    .map(|transaction| transaction.value)
}

/// Loopback-only routes, on both network prefixes.
pub(crate) fn routes(door: Arc<DoorLifecycle>) -> Router {
    let mut router = Router::new();
    for prefix in NETWORK_ROUTE_PREFIXES {
        router = router
            .route(&format!("{prefix}/api/local-network"), get(status))
            .route(&format!("{prefix}/local-network/open"), post(open))
            .route(&format!("{prefix}/local-network/close"), post(close));
    }
    router.layer(Extension(door))
}

async fn status(Extension(door): Extension<Arc<DoorLifecycle>>) -> Response {
    Json(status_body(&door, None).await).into_response()
}

async fn open(Extension(door): Extension<Arc<DoorLifecycle>>) -> Response {
    set_and_rebind(&door, true).await
}

async fn close(Extension(door): Extension<Arc<DoorLifecycle>>) -> Response {
    set_and_rebind(&door, false).await
}

/// How often the door rechecks the choice and agents on your network. The
/// agents app turns that switch on by writing the config, not through here.
pub(crate) const WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Rebind the door whenever the effective choice and the live bind disagree.
pub(crate) async fn watch(door: Arc<DoorLifecycle>) {
    loop {
        tokio::time::sleep(WATCH_INTERVAL).await;
        if door.is_stopped() {
            return;
        }
        let want = resolve(door.journal_root()).open;
        if door.listening_on_network().is_some_and(|have| have != want) {
            door.rebind().await;
        }
    }
}

async fn set_and_rebind(door: &DoorLifecycle, open: bool) -> Response {
    let changed = match save(door.journal_root(), open) {
        Ok(changed) => changed,
        Err(error) => {
            log::warn!("local-network choice was not saved: {error}");
            return refusal(
                "service_operation_failed",
                "the choice couldn't be saved. nothing changed.",
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    };
    let want = resolve(door.journal_root()).open;
    if door.listening_on_network().is_some_and(|have| have != want) {
        door.rebind().await;
    }
    let mut body = status_body(door, Some(changed)).await;
    // Closing is saved, but agents on your network keeps the door open until
    // the owner turns that off in agents.
    if !open && want {
        body["reason_code"] = Value::String("agents_on_network".to_owned());
        body["detail"] = Value::String(
            "your journal stays open to devices on your network while \"agents on your network\" is on. it closes to them when you turn that off in agents."
                .to_owned(),
        );
        return (StatusCode::CONFLICT, Json(body)).into_response();
    }
    // A withheld door (setup not finished) just keeps the choice for its first
    // start; only a door that tried and could not bind is a failure here.
    if door.bind_failed() {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
    }
    Json(body).into_response()
}

async fn status_body(door: &DoorLifecycle, changed: Option<bool>) -> Value {
    let state = resolve(door.journal_root());
    let bound = door.bound_addr();
    let listening_on = match bound {
        Some(address) if address.ip().is_loopback() => "this_pc",
        Some(_) => "local_network",
        None => "not_listening",
    };
    let mut body = json!({
        "open": state.open,
        "source": state.source.as_wire(),
        "listening_on": listening_on,
        "port": bound.map(|address| address.port()),
        "windows_asks": cfg!(windows),
        "agents_on_network": lan_door_on(door.journal_root()),
        "windows_firewall_blocked": if listening_on == "local_network" {
            crate::windows_firewall::app_blocked().await
        } else {
            None
        },
    });
    if let Some(changed) = changed {
        body["changed"] = Value::Bool(changed);
    }
    body
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::json;

    use super::*;

    struct Temp(std::path::PathBuf);

    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "local-network-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            fs::create_dir_all(path.join("config")).expect("config dir");
            fs::create_dir_all(path.join("link")).expect("link dir");
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn config(&self, value: Value) {
            fs::write(self.0.join("config/journal.json"), value.to_string()).expect("config");
        }
        fn clients(&self, value: Value) {
            fs::write(
                self.0.join("link/authorized_clients.json"),
                value.to_string(),
            )
            .expect("clients");
        }
        fn saved(&self) -> Option<bool> {
            saved_choice(&self.0)
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn client(fingerprint: &str, network: Option<&str>) -> Value {
        let mut value = json!({
            "fingerprint": fingerprint,
            "device_label": "device",
            "paired_at": "2026-09-30T00:00:00Z",
            "instance_id": "i",
            "role": "",
            "kind": "cert",
        });
        if let Some(network) = network {
            value["network"] = json!(network);
        }
        value
    }

    #[test]
    fn unix_default_keeps_the_network_and_saves_nothing() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}}));
        let state = resolve_for_door_with(journal.path(), true);
        assert_eq!(
            state,
            LocalNetworkState {
                open: true,
                source: LocalNetworkSource::Default
            }
        );
        assert_eq!(journal.saved(), None);
    }

    #[test]
    fn windows_default_is_this_computer_and_is_saved() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}}));
        journal.clients(json!([client("a", Some("home"))]));
        let state = resolve_for_door_with(journal.path(), false);
        assert_eq!(
            state,
            LocalNetworkState {
                open: false,
                source: LocalNetworkSource::Default
            }
        );
        assert_eq!(journal.saved(), Some(false));
        // Pairing a device from elsewhere later does not flip a saved answer.
        journal.clients(json!([client("a", Some("home")), client("b", None)]));
        assert!(!resolve_with(journal.path(), false).open);
    }

    #[test]
    fn windows_device_paired_from_elsewhere_carries_the_network_over() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}}));
        journal.clients(json!([client("a", Some("home")), client("b", None)]));
        let state = resolve_for_door_with(journal.path(), false);
        assert_eq!(
            state,
            LocalNetworkState {
                open: true,
                source: LocalNetworkSource::CarriedOver
            }
        );
        assert_eq!(journal.saved(), Some(true));
        assert_eq!(
            resolve_with(journal.path(), false).source,
            LocalNetworkSource::Saved
        );
    }

    #[test]
    fn agents_on_your_network_keeps_a_closed_door_open_without_saving_it() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}, "pairing": {"local_network": false}, "mcp_endpoint": {"lan_door": true}}));
        assert_eq!(
            resolve_for_door_with(journal.path(), false),
            LocalNetworkState {
                open: true,
                source: LocalNetworkSource::AgentsOnNetwork
            }
        );
        assert_eq!(journal.saved(), Some(false));
        journal.config(json!({"setup": {"completed_at": 1}, "pairing": {"local_network": false}, "mcp_endpoint": {"lan_door": false}}));
        assert!(!resolve_with(journal.path(), false).open);
    }

    #[test]
    fn windows_lan_agent_door_carries_the_network_over() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}, "mcp_endpoint": {"lan_door": true}}));
        assert_eq!(
            resolve_with(journal.path(), false).source,
            LocalNetworkSource::CarriedOver
        );
    }

    #[test]
    fn windows_unreadable_ledger_never_cuts_an_owner_off() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}}));
        fs::write(journal.path().join("link/authorized_clients.json"), "{").expect("bad ledger");
        assert!(resolve_with(journal.path(), false).open);
    }

    #[test]
    fn a_fresh_journal_listens_on_this_computer_only_on_windows() {
        let journal = Temp::new();
        journal.config(json!({"setup": {"completed_at": 1}}));
        assert_eq!(resolve(journal.path()).open, !cfg!(windows));
    }

    #[test]
    fn a_saved_choice_wins_on_every_platform() {
        let journal = Temp::new();
        journal.config(json!({"pairing": {"local_network": false}}));
        assert!(!resolve_with(journal.path(), true).open);
        journal.config(json!({"pairing": {"local_network": true}}));
        assert!(resolve_with(journal.path(), false).open);
    }

    #[test]
    fn save_reports_change_and_keeps_other_pairing_keys() {
        let journal = Temp::new();
        journal.config(json!({"pairing": {"direct_port": 7657}}));
        assert!(save(journal.path(), true).expect("save"));
        assert!(!save(journal.path(), true).expect("save again"));
        let written: Value = serde_json::from_slice(
            &fs::read(journal.path().join("config/journal.json")).expect("read"),
        )
        .expect("json");
        assert_eq!(written["pairing"]["direct_port"], 7657);
        assert_eq!(written["pairing"]["local_network"], true);
    }
}
