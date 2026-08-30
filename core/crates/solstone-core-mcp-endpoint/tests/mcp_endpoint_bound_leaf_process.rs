// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::fcntl::{AT_FDCWD, OFlag, openat};
use nix::sys::stat::Mode;
#[cfg(target_os = "linux")]
use nix::sys::stat::{SFlag, makedev, mknod};
use nix::unistd::mkfifo;
use solstone_core_journal_config::{ConfigLoadError, read_journal_config_bound};
use solstone_core_journal_io::{
    BoundReadPrimitive, JournalRoot, read_bytes_bound, run_with_bound_read_barrier,
    run_with_bound_read_fault, run_with_two_bound_read_barriers,
};
use solstone_core_mcp_endpoint::{
    McpEndpointBootstrapError, bootstrap_mcp_endpoint_owner_identity,
};
use solstone_core_sol_link::ca::{generate_ca, jid_from_spki};
use solstone_core_sol_link::committed::{CommittedIdentityError, load_committed_identity_bound};

const ROW_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Leaf {
    ConfigJournalJson,
    CaCertificatePem,
    CaPrivatePem,
    LinkStatePrimary,
    CaStateFallback,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RaceClass {
    FifoSubstitutionBeforeOpen,
    UnixSocketSubstitutionBeforeOpen,
    RegularReplacementBeforeOpen,
    DisappearanceBeforeOpen,
    RegularReplacementAfterOpen,
    DisappearanceAfterOpen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Donor {
    InitialFifo,
    InitialSocket,
    FifoSubstitution,
    SocketSubstitution,
    DisappearanceBeforeOpen,
    RegularReplacementBeforeOpen,
    DisappearanceAfterOpen,
    RegularReplacementAfterOpen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Control {
    Missing,
    Unchanged,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowKind {
    Donor(Donor),
    Control(Control),
    Direct { leaf: Leaf, race: RaceClass },
    EndpointTwin { leaf: Leaf, race: RaceClass },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Row {
    id: &'static str,
    kind: RowKind,
}

macro_rules! direct {
    ($id:literal, $race:ident, $leaf:ident) => {
        Row {
            id: $id,
            kind: RowKind::Direct {
                race: RaceClass::$race,
                leaf: Leaf::$leaf,
            },
        }
    };
}

macro_rules! endpoint {
    ($id:literal, $race:ident, $leaf:ident) => {
        Row {
            id: $id,
            kind: RowKind::EndpointTwin {
                race: RaceClass::$race,
                leaf: Leaf::$leaf,
            },
        }
    };
}

const ROWS: [Row; 70] = [
    Row {
        id: "donor-initial-fifo",
        kind: RowKind::Donor(Donor::InitialFifo),
    },
    Row {
        id: "donor-initial-unix-socket",
        kind: RowKind::Donor(Donor::InitialSocket),
    },
    Row {
        id: "donor-fifo-substitution-before-open",
        kind: RowKind::Donor(Donor::FifoSubstitution),
    },
    Row {
        id: "donor-unix-socket-substitution-before-open",
        kind: RowKind::Donor(Donor::SocketSubstitution),
    },
    Row {
        id: "donor-disappearance-before-open",
        kind: RowKind::Donor(Donor::DisappearanceBeforeOpen),
    },
    Row {
        id: "donor-regular-replacement-before-open",
        kind: RowKind::Donor(Donor::RegularReplacementBeforeOpen),
    },
    Row {
        id: "donor-disappearance-after-open",
        kind: RowKind::Donor(Donor::DisappearanceAfterOpen),
    },
    Row {
        id: "donor-regular-replacement-after-open",
        kind: RowKind::Donor(Donor::RegularReplacementAfterOpen),
    },
    Row {
        id: "control-missing",
        kind: RowKind::Control(Control::Missing),
    },
    Row {
        id: "control-unchanged-bytes",
        kind: RowKind::Control(Control::Unchanged),
    },
    direct!(
        "direct-fifo-before-open-config",
        FifoSubstitutionBeforeOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-fifo-before-open-certificate",
        FifoSubstitutionBeforeOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-fifo-before-open-private-key",
        FifoSubstitutionBeforeOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-fifo-before-open-primary-state",
        FifoSubstitutionBeforeOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-fifo-before-open-fallback-state",
        FifoSubstitutionBeforeOpen,
        CaStateFallback
    ),
    direct!(
        "direct-replacement-before-open-config",
        RegularReplacementBeforeOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-replacement-before-open-certificate",
        RegularReplacementBeforeOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-replacement-before-open-private-key",
        RegularReplacementBeforeOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-replacement-before-open-primary-state",
        RegularReplacementBeforeOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-replacement-before-open-fallback-state",
        RegularReplacementBeforeOpen,
        CaStateFallback
    ),
    direct!(
        "direct-disappearance-before-open-config",
        DisappearanceBeforeOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-disappearance-before-open-certificate",
        DisappearanceBeforeOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-disappearance-before-open-private-key",
        DisappearanceBeforeOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-disappearance-before-open-primary-state",
        DisappearanceBeforeOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-disappearance-before-open-fallback-state",
        DisappearanceBeforeOpen,
        CaStateFallback
    ),
    direct!(
        "direct-replacement-after-open-config",
        RegularReplacementAfterOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-replacement-after-open-certificate",
        RegularReplacementAfterOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-replacement-after-open-private-key",
        RegularReplacementAfterOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-replacement-after-open-primary-state",
        RegularReplacementAfterOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-replacement-after-open-fallback-state",
        RegularReplacementAfterOpen,
        CaStateFallback
    ),
    direct!(
        "direct-disappearance-after-open-config",
        DisappearanceAfterOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-disappearance-after-open-certificate",
        DisappearanceAfterOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-disappearance-after-open-private-key",
        DisappearanceAfterOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-disappearance-after-open-primary-state",
        DisappearanceAfterOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-disappearance-after-open-fallback-state",
        DisappearanceAfterOpen,
        CaStateFallback
    ),
    direct!(
        "direct-unix-socket-before-open-config",
        UnixSocketSubstitutionBeforeOpen,
        ConfigJournalJson
    ),
    direct!(
        "direct-unix-socket-before-open-certificate",
        UnixSocketSubstitutionBeforeOpen,
        CaCertificatePem
    ),
    direct!(
        "direct-unix-socket-before-open-private-key",
        UnixSocketSubstitutionBeforeOpen,
        CaPrivatePem
    ),
    direct!(
        "direct-unix-socket-before-open-primary-state",
        UnixSocketSubstitutionBeforeOpen,
        LinkStatePrimary
    ),
    direct!(
        "direct-unix-socket-before-open-fallback-state",
        UnixSocketSubstitutionBeforeOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-fifo-before-open-config",
        FifoSubstitutionBeforeOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-fifo-before-open-certificate",
        FifoSubstitutionBeforeOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-fifo-before-open-private-key",
        FifoSubstitutionBeforeOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-fifo-before-open-primary-state",
        FifoSubstitutionBeforeOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-fifo-before-open-fallback-state",
        FifoSubstitutionBeforeOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-replacement-before-open-config",
        RegularReplacementBeforeOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-replacement-before-open-certificate",
        RegularReplacementBeforeOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-replacement-before-open-private-key",
        RegularReplacementBeforeOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-replacement-before-open-primary-state",
        RegularReplacementBeforeOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-replacement-before-open-fallback-state",
        RegularReplacementBeforeOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-disappearance-before-open-config",
        DisappearanceBeforeOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-disappearance-before-open-certificate",
        DisappearanceBeforeOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-disappearance-before-open-private-key",
        DisappearanceBeforeOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-disappearance-before-open-primary-state",
        DisappearanceBeforeOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-disappearance-before-open-fallback-state",
        DisappearanceBeforeOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-replacement-after-open-config",
        RegularReplacementAfterOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-replacement-after-open-certificate",
        RegularReplacementAfterOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-replacement-after-open-private-key",
        RegularReplacementAfterOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-replacement-after-open-primary-state",
        RegularReplacementAfterOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-replacement-after-open-fallback-state",
        RegularReplacementAfterOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-disappearance-after-open-config",
        DisappearanceAfterOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-disappearance-after-open-certificate",
        DisappearanceAfterOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-disappearance-after-open-private-key",
        DisappearanceAfterOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-disappearance-after-open-primary-state",
        DisappearanceAfterOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-disappearance-after-open-fallback-state",
        DisappearanceAfterOpen,
        CaStateFallback
    ),
    endpoint!(
        "endpoint-unix-socket-before-open-config",
        UnixSocketSubstitutionBeforeOpen,
        ConfigJournalJson
    ),
    endpoint!(
        "endpoint-unix-socket-before-open-certificate",
        UnixSocketSubstitutionBeforeOpen,
        CaCertificatePem
    ),
    endpoint!(
        "endpoint-unix-socket-before-open-private-key",
        UnixSocketSubstitutionBeforeOpen,
        CaPrivatePem
    ),
    endpoint!(
        "endpoint-unix-socket-before-open-primary-state",
        UnixSocketSubstitutionBeforeOpen,
        LinkStatePrimary
    ),
    endpoint!(
        "endpoint-unix-socket-before-open-fallback-state",
        UnixSocketSubstitutionBeforeOpen,
        CaStateFallback
    ),
];

#[test]
fn bound_read_leaf_process_rows() {
    assert_eq!(ROWS.len(), 70, "the process target owns all 70 rows");
    let mut categories = [0_usize; 4];
    for row in ROWS {
        categories[match row.kind {
            RowKind::Donor(_) => 0,
            RowKind::Control(_) => 1,
            RowKind::Direct { .. } => 2,
            RowKind::EndpointTwin { .. } => 3,
        }] += 1;
        run_detached(row).unwrap_or_else(|error| panic!("{}: {error}", row.id));
    }
    assert_eq!(categories, [8, 2, 30, 30]);
    println!(
        "verified bound-read rows: donors=8 controls=2 direct=30 endpoint=30 total={}",
        ROWS.len()
    );
}

fn run_detached(row: Row) -> Result<(), String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let result = execute_row(row);
        let _ = sender.send(result);
    });
    match receiver.recv_timeout(ROW_TIMEOUT) {
        Ok(result) => match worker.join() {
            Ok(()) => result,
            Err(_) => Err("row worker panicked after reporting a result".to_owned()),
        },
        Err(mpsc::RecvTimeoutError::Timeout) => {
            drop(worker);
            Err("row timed out after five seconds".to_owned())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            drop(worker);
            Err("row worker panicked before reporting a result".to_owned())
        }
    }
}

fn execute_row(row: Row) -> Result<(), String> {
    match row.kind {
        RowKind::Donor(donor) => execute_donor(donor),
        RowKind::Control(control) => execute_control(control),
        RowKind::Direct { leaf, race } => execute_direct(leaf, race),
        RowKind::EndpointTwin { leaf, race } => execute_endpoint(leaf, race),
    }
}

fn execute_donor(donor: Donor) -> Result<(), String> {
    match donor {
        Donor::InitialFifo => donor_initial_fifo(),
        Donor::InitialSocket => donor_initial_socket(),
        Donor::FifoSubstitution => donor_race(RaceClass::FifoSubstitutionBeforeOpen),
        Donor::SocketSubstitution => donor_race(RaceClass::UnixSocketSubstitutionBeforeOpen),
        Donor::DisappearanceBeforeOpen => donor_race(RaceClass::DisappearanceBeforeOpen),
        Donor::RegularReplacementBeforeOpen => donor_race(RaceClass::RegularReplacementBeforeOpen),
        Donor::DisappearanceAfterOpen => donor_race(RaceClass::DisappearanceAfterOpen),
        Donor::RegularReplacementAfterOpen => donor_race(RaceClass::RegularReplacementAfterOpen),
    }
}

fn execute_control(control: Control) -> Result<(), String> {
    match control {
        Control::Missing => {
            let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
            let directory = open_directory(root.path())?;
            if read_bytes_bound(&directory, OsStr::new("record"))
                .map_err(|error| error.to_string())?
                .is_some()
            {
                return Err("missing raw record did not remain absent".to_owned());
            }
            assert_no_endpoint(root.path())
        }
        Control::Unchanged => {
            let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
            let expected = b"unchanged donor bytes";
            fs::write(root.path().join("record"), expected).map_err(|error| error.to_string())?;
            let directory = open_directory(root.path())?;
            let observed = read_bytes_bound(&directory, OsStr::new("record"))
                .map_err(|error| error.to_string())?;
            if observed.as_deref() != Some(expected.as_slice()) {
                return Err("unchanged raw record did not round-trip byte-exactly".to_owned());
            }
            assert_no_endpoint(root.path())
        }
    }
}

fn donor_initial_fifo() -> Result<(), String> {
    let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
    let path = root.path().join("record");
    mkfifo(&path, Mode::from_bits_truncate(0o600)).map_err(|error| error.to_string())?;
    let directory = open_directory(root.path())?;
    let (result, open_attempted) =
        run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
            read_bytes_bound(&directory, OsStr::new("record"))
        });
    if result.is_ok() || open_attempted {
        return Err("initial FIFO reached open or was accepted".to_owned());
    }
    assert_no_endpoint(root.path())
}

fn donor_initial_socket() -> Result<(), String> {
    let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
    let path = root.path().join("record");
    let listener = UnixListener::bind(&path).map_err(|error| error.to_string())?;
    let directory = open_directory(root.path())?;
    let (result, open_attempted) =
        run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
            read_bytes_bound(&directory, OsStr::new("record"))
        });
    drop(listener);
    if result.is_ok() || open_attempted {
        return Err("initial socket reached open or was accepted".to_owned());
    }
    assert_no_endpoint(root.path())
}

fn donor_race(race: RaceClass) -> Result<(), String> {
    let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
    let path = root.path().join("record");
    fs::write(&path, b"original").map_err(|error| error.to_string())?;
    let directory = open_directory(root.path())?;
    exercise_race(race, path, 1, || {
        match read_bytes_bound(&directory, OsStr::new("record")) {
            Err(_) => Ok(()),
            Ok(other) => Err(format!("adversarial donor returned {other:?}")),
        }
    })?;
    assert_no_endpoint(root.path())
}

#[test]
fn initial_device_control_is_outside_the_70_row_denominator() {
    if donor_device_control_if_available().expect("optional initial-device control") {
        println!("executed optional initial-device control outside 70-row denominator");
    } else {
        println!("optional initial-device control unavailable outside 70-row denominator");
    }
}

fn donor_device_control_if_available() -> Result<bool, String> {
    #[cfg(target_os = "linux")]
    {
        let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
        let target = root.path().join("record");
        match mknod(
            &target,
            SFlag::S_IFCHR,
            Mode::from_bits_truncate(0o600),
            makedev(1, 3),
        ) {
            Ok(()) => {}
            Err(Errno::EPERM | Errno::EACCES) => return Ok(false),
            Err(error) => return Err(format!("device fixture creates: {error}")),
        }
        let directory = open_directory(root.path())?;
        let (result, open_attempted) =
            run_with_bound_read_fault(BoundReadPrimitive::Open, 1, Errno::EIO as i32, || {
                read_bytes_bound(&directory, OsStr::new("record"))
            });
        if result.is_ok() || open_attempted {
            return Err("initial device did not reject before open".to_owned());
        }
        assert_no_endpoint(root.path())?;
        Ok(true)
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(false)
    }
}

fn execute_direct(leaf: Leaf, race: RaceClass) -> Result<(), String> {
    let root = prepared_root(leaf)?;
    assert_state_precedence(root.path(), leaf)?;
    let target = leaf_path(root.path(), leaf);
    let admitted = open_root(root.path())?;
    let result = exercise_race(race, target, direct_ordinal(leaf), || {
        invoke_direct(&admitted, leaf)
    });
    assert_no_endpoint(root.path())?;
    result
}

fn execute_endpoint(leaf: Leaf, race: RaceClass) -> Result<(), String> {
    let root = prepared_root(leaf)?;
    assert_state_precedence(root.path(), leaf)?;
    let target = leaf_path(root.path(), leaf);
    let result = exercise_race(race, target, endpoint_ordinal(leaf), || {
        invoke_endpoint(root.path(), leaf)
    });
    assert_no_endpoint(root.path())?;
    result
}

fn exercise_race(
    race: RaceClass,
    target: PathBuf,
    ordinal: usize,
    operation: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    match race {
        RaceClass::FifoSubstitutionBeforeOpen => {
            let (result, fired) = run_with_bound_read_barrier(
                BoundReadPrimitive::Open,
                ordinal,
                move || {
                    fs::remove_file(&target).expect("leaf removes");
                    mkfifo(&target, Mode::from_bits_truncate(0o600)).expect("FIFO creates");
                },
                operation,
            );
            if !fired {
                return Err("FIFO barrier did not fire".to_owned());
            }
            result
        }
        RaceClass::UnixSocketSubstitutionBeforeOpen => {
            let (result, fired) = run_with_bound_read_barrier(
                BoundReadPrimitive::Open,
                ordinal,
                move || replace_with_socket(&target),
                operation,
            );
            if !fired {
                return Err("Unix-socket barrier did not fire".to_owned());
            }
            result
        }
        RaceClass::RegularReplacementBeforeOpen => {
            let replacement = replacement_path(&target)?;
            let (result, fired) = run_with_bound_read_barrier(
                BoundReadPrimitive::Open,
                ordinal,
                move || fs::rename(&replacement, &target).expect("replacement installs"),
                operation,
            );
            if !fired {
                return Err("replacement-before-open barrier did not fire".to_owned());
            }
            result
        }
        RaceClass::DisappearanceBeforeOpen => {
            let (result, fired) = run_with_bound_read_barrier(
                BoundReadPrimitive::Open,
                ordinal,
                move || fs::remove_file(&target).expect("leaf removes"),
                operation,
            );
            if !fired {
                return Err("disappearance-before-open barrier did not fire".to_owned());
            }
            result
        }
        RaceClass::RegularReplacementAfterOpen => {
            let replacement = replacement_path(&target)?;
            let aside = target.with_extension("aside");
            let observed_replacement = target.clone();
            let (result, fired) = run_with_two_bound_read_barriers(
                BoundReadPrimitive::Read,
                ordinal,
                move || {
                    fs::rename(&target, &aside).expect("original moves aside");
                    fs::rename(&replacement, &target).expect("replacement installs");
                },
                BoundReadPrimitive::FinalNameObserve,
                ordinal,
                move || assert!(observed_replacement.exists(), "replacement remains named"),
                operation,
            );
            if fired != 2 {
                return Err(format!("replacement-after-open fired {fired} barriers"));
            }
            result
        }
        RaceClass::DisappearanceAfterOpen => {
            let (result, fired) = run_with_bound_read_barrier(
                BoundReadPrimitive::Read,
                ordinal,
                move || fs::remove_file(&target).expect("leaf removes"),
                operation,
            );
            if !fired {
                return Err("disappearance-after-open barrier did not fire".to_owned());
            }
            result
        }
    }
}

fn invoke_direct(root: &JournalRoot, leaf: Leaf) -> Result<(), String> {
    match leaf {
        Leaf::ConfigJournalJson => match read_journal_config_bound(root) {
            Err(ConfigLoadError::Corrupt { .. }) => Ok(()),
            other => Err(format!("expected config corruption, got {other:?}")),
        },
        _ => match load_committed_identity_bound(root) {
            Err(error) if committed_error_kind(&error) == expected_link_error(leaf) => Ok(()),
            Err(error) => Err(format!(
                "expected {}, got {}",
                expected_link_error(leaf),
                committed_error_kind(&error)
            )),
            Ok(_) => Err(format!(
                "expected {}, got success",
                expected_link_error(leaf)
            )),
        },
    }
}

fn invoke_endpoint(root: &Path, leaf: Leaf) -> Result<(), String> {
    match (leaf, bootstrap_mcp_endpoint_owner_identity(root)) {
        (Leaf::ConfigJournalJson, Err(McpEndpointBootstrapError::ConfigRead)) => Ok(()),
        (Leaf::ConfigJournalJson, Err(error)) => Err(format!("expected ConfigRead, got {error:?}")),
        (Leaf::ConfigJournalJson, Ok(_)) => Err("expected ConfigRead, got success".to_owned()),
        (_, Err(McpEndpointBootstrapError::Endpoint)) => Ok(()),
        (_, Err(error)) => Err(format!("expected Endpoint, got {error:?}")),
        (_, Ok(_)) => Err("expected Endpoint, got success".to_owned()),
    }
}

fn committed_error_kind(error: &CommittedIdentityError) -> &'static str {
    match error {
        CommittedIdentityError::CertificateRead { .. } => "certificate-read",
        CommittedIdentityError::PrivateKeyRead { .. } => "private-key-read",
        CommittedIdentityError::CertificatePem { .. } => "certificate-pem",
        CommittedIdentityError::Ca { .. } => "ca",
        CommittedIdentityError::StateRead { .. } => "state-read",
        CommittedIdentityError::StateMalformed { .. } => "state-malformed",
        CommittedIdentityError::StateInstanceMismatch { .. } => "state-instance-mismatch",
    }
}

fn expected_link_error(leaf: Leaf) -> &'static str {
    match leaf {
        Leaf::CaCertificatePem => "certificate-read",
        Leaf::CaPrivatePem => "private-key-read",
        Leaf::LinkStatePrimary | Leaf::CaStateFallback => "state-read",
        Leaf::ConfigJournalJson => unreachable!("config is not a link leaf"),
    }
}

fn prepared_root(leaf: Leaf) -> Result<tempfile::TempDir, String> {
    let root = tempfile::TempDir::new().map_err(|error| error.to_string())?;
    write_enabled_config(root.path())?;
    write_committed_identity(root.path(), leaf)?;
    Ok(root)
}

fn write_enabled_config(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join("config")).map_err(|error| error.to_string())?;
    fs::write(
        root.join("config/journal.json"),
        br#"{"mcp_endpoint":{"enabled":true}}"#,
    )
    .map_err(|error| error.to_string())
}

fn write_committed_identity(root: &Path, leaf: Leaf) -> Result<(), String> {
    let ca = generate_ca().map_err(|error| error.to_string())?;
    let instance_id = jid_from_spki(ca.spki_der()).map_err(|error| error.to_string())?;
    let directory = root.join("link/ca");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::write(directory.join("cert.pem"), ca.certificate_pem())
        .map_err(|error| error.to_string())?;
    fs::write(directory.join("private.pem"), ca.private_key_pem())
        .map_err(|error| error.to_string())?;

    if leaf != Leaf::CaStateFallback {
        write_state(&root.join("link/state.json"), &instance_id, "Primary Home")?;
    }
    if leaf == Leaf::LinkStatePrimary || leaf == Leaf::CaStateFallback {
        write_state(&directory.join("state.json"), &instance_id, "Fallback Home")?;
    }
    Ok(())
}

fn write_state(path: &Path, instance_id: &str, home_label: &str) -> Result<(), String> {
    fs::write(
        path,
        format!(r#"{{"instance_id":"{instance_id}","home_label":"{home_label}"}}"#),
    )
    .map_err(|error| error.to_string())
}

fn assert_state_precedence(root: &Path, leaf: Leaf) -> Result<(), String> {
    let expected = match leaf {
        Leaf::LinkStatePrimary => Some("Primary Home"),
        Leaf::CaStateFallback => Some("Fallback Home"),
        _ => None,
    };
    let Some(expected) = expected else {
        return Ok(());
    };
    let admitted = open_root(root)?;
    let identity = load_committed_identity_bound(&admitted).map_err(link_error)?;
    if identity.home_label() != expected {
        return Err(format!(
            "expected {expected:?} state precedence, got {:?}",
            identity.home_label()
        ));
    }
    Ok(())
}

fn leaf_path(root: &Path, leaf: Leaf) -> PathBuf {
    match leaf {
        Leaf::ConfigJournalJson => root.join("config/journal.json"),
        Leaf::CaCertificatePem => root.join("link/ca/cert.pem"),
        Leaf::CaPrivatePem => root.join("link/ca/private.pem"),
        Leaf::LinkStatePrimary => root.join("link/state.json"),
        Leaf::CaStateFallback => root.join("link/ca/state.json"),
    }
}

fn direct_ordinal(leaf: Leaf) -> usize {
    match leaf {
        Leaf::ConfigJournalJson | Leaf::CaCertificatePem => 1,
        Leaf::CaPrivatePem => 2,
        Leaf::LinkStatePrimary | Leaf::CaStateFallback => 3,
    }
}

fn endpoint_ordinal(leaf: Leaf) -> usize {
    match leaf {
        Leaf::ConfigJournalJson => 1,
        Leaf::CaCertificatePem => 2,
        Leaf::CaPrivatePem => 3,
        Leaf::LinkStatePrimary | Leaf::CaStateFallback => 4,
    }
}

fn open_root(path: &Path) -> Result<JournalRoot, String> {
    JournalRoot::open(path).map_err(|error| error.to_string())
}

fn open_directory(path: &Path) -> Result<OwnedFd, String> {
    openat(
        AT_FDCWD,
        path,
        OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|error| error.to_string())
}

fn replacement_path(target: &Path) -> Result<PathBuf, String> {
    let replacement = target.with_extension("replacement");
    let permissions = fs::metadata(target)
        .map_err(|error| error.to_string())?
        .permissions();
    fs::write(&replacement, b"replacement").map_err(|error| error.to_string())?;
    fs::set_permissions(&replacement, permissions).map_err(|error| error.to_string())?;
    Ok(replacement)
}

fn replace_with_socket(path: &Path) {
    fs::remove_file(path).expect("leaf removes");
    let listener = UnixListener::bind(path).expect("socket binds");
    drop(listener);
}

fn link_error(error: CommittedIdentityError) -> String {
    error.to_string()
}

fn assert_no_endpoint(root: &Path) -> Result<(), String> {
    if root.join("mcp-endpoint").exists() {
        Err("rejected leaf read created endpoint state".to_owned())
    } else {
        Ok(())
    }
}
