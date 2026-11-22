// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The schema-1 install record is also read by earlier installers. Keep its
//! bytes intact and attach optional presentation only to the exact generation
//! the journal wrote. The companion stores no copy of its strings.

use super::InstallStatus;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_cli_copy::MessageDetail;
use solstone_core_journal_io::{
    AtomicWriteOptions, JournalRoot, atomic_replace,
    media::{FileReadVersion, MediaDirectory},
};
use std::{ffi::OsStr, io::Read, ops::Range, path::Path};

const LIMIT: u64 = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    version: u8,
    generation: FileReadVersion,
    sha256: String,
    message: SavedMessage,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedMessage {
    id: String,
    arguments: Vec<Range<usize>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cause: Option<Box<NamedCause>>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedCause {
    slot: String,
    message: SavedMessage,
}

impl SavedMessage {
    fn capture(detail: &MessageDetail, source: &str) -> Option<Self> {
        let context = detail.context_fields();
        let cardinal = detail.cardinal_fields();
        let (id, fields) = if let Some((message, _, _)) = context {
            (message.id(), message.fields())
        } else if let Some((message, _)) = cardinal {
            (message.id(), message.fields())
        } else {
            detail.catalog_fields()?
        };
        let shape = arguments(id)?;
        let mut args = Vec::new();
        let mut cause = None;
        for name in shape {
            let value = if let Some((_, slot, inner)) = context
                && slot == *name
            {
                let text = inner.to_string();
                cause = Self::capture(inner, &text).map(|message| {
                    Box::new(NamedCause {
                        slot: slot.to_owned(),
                        message,
                    })
                });
                text
            } else if id == "journal.cli.install.archive.size" && *name == "quantity" {
                fields
                    .iter()
                    .find(|(field, _)| *field == "expected")?
                    .1
                    .clone()
            } else if let Some(count) =
                cardinal.and_then(|(_, counts)| counts.iter().find(|(field, _)| field == name))
            {
                count.1.to_string()
            } else {
                fields.iter().find(|(field, _)| field == name)?.1.clone()
            };
            let start = source.find(&value)?;
            args.push(start..start + value.len());
        }
        let saved = Self {
            id: id.to_owned(),
            arguments: args,
            cause,
        };
        (saved.render(source, None, 0)? == source).then_some(saved)
    }

    fn render(&self, source: &str, locale: Option<&str>, depth: usize) -> Option<String> {
        let names = arguments(&self.id)?;
        if depth > 4 || names.len() != self.arguments.len() {
            return None;
        }
        let mut fields = Vec::new();
        for (name, range) in names.iter().zip(&self.arguments) {
            let literal = source.get(range.clone())?;
            let value = if let Some(cause) = &self.cause
                && cause.slot == *name
            {
                if cause.message.render(literal, None, depth + 1)? != literal {
                    return None;
                }
                cause.message.render(literal, locale, depth + 1)?
            } else {
                literal.to_owned()
            };
            fields.push((*name, value));
        }
        let value = |key| {
            fields
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, text)| text)
        };
        // These source records predate the human singular forms.
        if locale.is_none() {
            match self.id.as_str() {
                "journal.cli.install.hardware.memory.minimum" => {
                    return Some(format!(
                        "device-local VRAM below 6 GiB minimum: {} bytes",
                        value("available_bytes")?
                    ));
                }
                "journal.cli.install.archive.size" => {
                    let expected = value("expected")?;
                    if value("quantity")? != expected {
                        return None;
                    }
                    return Some(format!(
                        "download size mismatch: expected {expected} bytes, got {}",
                        value("actual")?
                    ));
                }
                _ => {}
            }
        }
        let locale = locale.unwrap_or(solstone_core_cli_copy::ENGLISH);
        if self.id.starts_with("journal.cli.") {
            let args: Vec<_> = fields
                .iter()
                .map(|(name, value)| {
                    let arg = match (self.id.as_str(), *name) {
                        ("journal.cli.install.hardware.memory.minimum", "available_bytes")
                        | ("journal.cli.install.archive.size", "quantity") => {
                            let count: u64 = value.parse().ok()?;
                            solstone_core_cli_copy::Value::Cardinal(
                                i64::try_from(count).unwrap_or(i64::MAX),
                            )
                        }
                        _ => solstone_core_cli_copy::Value::String(value),
                    };
                    Some((*name, arg))
                })
                .collect::<Option<Vec<_>>>()?;
            solstone_core_cli_copy::try_render_for(locale, &self.id, &args)
        } else {
            let args: Vec<_> = fields
                .iter()
                .map(|(name, value)| (*name, solstone_core_window_catalog::Value::String(value)))
                .collect();
            Some(solstone_core_window_catalog::render(
                locale, &self.id, &args,
            ))
        }
    }
}

fn directory(journal: &Path) -> Option<std::sync::Arc<MediaDirectory>> {
    MediaDirectory::open(
        JournalRoot::open(journal).ok()?.into(),
        Path::new("health/providers"),
    )
    .ok()
}

fn receipt_name(provider: &str) -> String {
    format!("{provider}.json.display")
}

fn status_generation(
    directory: &MediaDirectory,
    status: &InstallStatus,
) -> Option<(FileReadVersion, String)> {
    directory
        .read_versioned(
            OsStr::new(&format!("{}.json", status.provider)),
            |file, version| {
                let mut bytes = Vec::new();
                file.take(1024 * 1024 + 1).read_to_end(&mut bytes).ok()?;
                if bytes.len() > 1024 * 1024
                    || &serde_json::from_slice::<InstallStatus>(&bytes).ok()? != status
                {
                    return None;
                }
                Some((version, format!("{:x}", Sha256::digest(bytes))))
            },
        )
        .ok()?
}

/// Called under the same status lock as the authoritative atomic replacement.
/// Failed optional publication can only leave a mismatched generation.
pub(super) fn publish(journal: &Path, status: &InstallStatus, detail: Option<&MessageDetail>) {
    let path = journal
        .join("health/providers")
        .join(receipt_name(&status.provider));
    let message = status
        .install_error
        .as_deref()
        .zip(detail)
        .and_then(|(source, detail)| SavedMessage::capture(detail, source));
    let Some(message) = message else {
        let _ = std::fs::remove_file(path);
        return;
    };
    let Some((generation, sha256)) = directory(journal).and_then(|d| status_generation(&d, status))
    else {
        return;
    };
    let receipt = Receipt {
        version: 1,
        generation,
        sha256,
        message,
    };
    if let Ok(bytes) = serde_json::to_vec(&receipt)
        && bytes.len() as u64 <= LIMIT
    {
        let _ = atomic_replace(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) });
    }
}

/// Select known writer wording at the presentation boundary. Historical,
/// missing, modified and foreign details retain their original bytes.
pub fn display_error(journal: &Path, status: &InstallStatus, locale: &str) -> Option<String> {
    let source = status.install_error.as_deref()?;
    let locale = solstone_core_cli_copy::packaged_locale(Some(locale));
    if locale == solstone_core_cli_copy::ENGLISH
        || status.install_state != "failed"
        || !super::PROVIDERS.contains(&status.provider.as_str())
    {
        return Some(source.to_owned());
    }
    let projected = (|| {
        let directory = directory(journal)?;
        let receipt: Receipt = directory
            .read_versioned(OsStr::new(&receipt_name(&status.provider)), |file, _| {
                let mut bytes = Vec::new();
                file.take(LIMIT + 1).read_to_end(&mut bytes).ok()?;
                if bytes.len() as u64 > LIMIT {
                    return None;
                }
                serde_json::from_slice(&bytes).ok()
            })
            .ok()??;
        let (generation, sha256) = status_generation(&directory, status)?;
        if receipt.version != 1
            || receipt.generation != generation
            || receipt.sha256 != sha256
            || receipt.message.render(source, None, 0)? != source
        {
            return None;
        }
        receipt.message.render(source, Some(locale), 0)
    })();
    Some(projected.unwrap_or_else(|| source.to_owned()))
}

// Only identities emitted by the installer may describe a stored failure.
fn arguments(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "journal.cli.install.status.malformed.providers" => &["listed"],
        "journal.cli.install.lease.held"
        | "journal.cli.install.status.malformed.invalid.provider" => &["provider"],
        "journal.cli.install.status.malformed.state" => &["state"],
        "journal.cli.install.archive.path"
        | "journal.cli.install.coreml.sentinel.not.ready"
        | "journal.cli.install.mlx.directory.explicit"
        | "journal.cli.install.mlx.directory.invalid"
        | "journal.cli.install.mlx.directory.override"
        | "journal.cli.install.mlx.helper.explicit"
        | "journal.cli.install.mlx.helper.invalid"
        | "journal.cli.install.mlx.helper.override"
        | "journal.cli.install.staging.bundle.occupied"
        | "journal.cli.install.status.malformed.file"
        | "journal.cli.install.windows.digest.absent"
        | "journal.cli.install.windows.executable.location"
        | "journal.cli.journal.helper.missing.mac"
        | "journal.cli.journal.helper.missing.other"
        | "journal.cli.journal.helper.mode.mac"
        | "journal.cli.journal.helper.mode.other" => &["path"],
        "journal.cli.install.backend.metal"
        | "journal.cli.install.backend.unavailable"
        | "journal.cli.install.component.packaged"
        | "journal.cli.install.component.unfetched"
        | "journal.cli.install.coreml.asset.incomplete"
        | "journal.cli.install.coreml.attempt.changed"
        | "journal.cli.install.coreml.catalog.conflict"
        | "journal.cli.install.coreml.catalog.empty"
        | "journal.cli.install.coreml.sentinel.removed"
        | "journal.cli.install.cuda.integrity"
        | "journal.cli.install.disk.insufficient"
        | "journal.cli.install.fingerprint.artifact"
        | "journal.cli.install.fingerprint.backend"
        | "journal.cli.install.fingerprint.missing"
        | "journal.cli.install.fingerprint.runtime"
        | "journal.cli.install.hardware.apple.memory"
        | "journal.cli.install.hardware.memory.unknown"
        | "journal.cli.install.hardware.no.hardware"
        | "journal.cli.install.hardware.probe.required"
        | "journal.cli.install.lease.regular"
        | "journal.cli.install.mlx.directory.unset"
        | "journal.cli.install.mlx.helper.missing"
        | "journal.cli.install.mlx.override.failed"
        | "journal.cli.install.mlx.pins.empty"
        | "journal.cli.install.mlx.pins.missing"
        | "journal.cli.install.mlx.platform"
        | "journal.cli.install.runtime.unavailable"
        | "journal.cli.install.staging.binary.parent"
        | "journal.cli.install.staging.target.name"
        | "journal.cli.install.staging.target.parent"
        | "journal.cli.install.status.conflict.different.attempt.while.install.in.flight"
        | "journal.cli.install.status.conflict.illegal.install.status.transition"
        | "journal.cli.install.status.conflict.install.attempt.id.changed"
        | "journal.cli.install.status.conflict.install.attempt.is.no.longer.in.flight"
        | "journal.cli.install.status.conflict.install.progress.can.only.be.bumped.for"
        | "journal.cli.install.status.conflict.install.target.fingerprint.changed"
        | "journal.cli.install.status.conflict.interrupted.attempt.id.does.not.match"
        | "journal.cli.install.status.conflict.interrupted.target.fingerprint.does.not.match"
        | "journal.cli.install.status.conflict.new.in.flight.attempt.reused.attempt.id"
        | "journal.cli.install.status.conflict.non.idle.install.status.requires.attempt.id"
        | "journal.cli.install.status.conflict.only.in.flight.installs.can.be.interrupted"
        | "journal.cli.install.status.conflict.stale.install.status.revision"
        | "journal.cli.install.status.malformed.initial.install.attempt.state.must.be.in"
        | "journal.cli.install.status.malformed.owner.must.be.an.object.or.null"
        | "journal.cli.install.terminal.write"
        | "journal.cli.install.windows.executable.parent"
        | "journal.cli.install.windows.package.root"
        | "journal.cli.install.windows.package.runtime"
        | "journal.cli.install.windows.target.unsupported"
        | "journal.cli.transcribe.error.coreml.publication.busy"
        | "journal.cli.transcribe.error.process.command"
        | "journal.cli.transcribe.error.process.explicit"
        | "journal.cli.transcribe.error.process.output"
        | "journal.cli.transcribe.error.process.unowned.reason"
        | "journal.setup.cancel.admission"
        | "journal.setup.cancel.busy"
        | "journal.setup.cancel.changed"
        | "journal.setup.cancel.identity"
        | "journal.setup.installer.control.timed.out"
        | "journal.setup.installer.control.unavailable"
        | "journal.setup.installer.directory.unavailable"
        | "journal.setup.installer.identity.unavailable"
        | "journal.setup.installer.job.authority"
        | "journal.setup.installer.lease.busy"
        | "journal.setup.installer.process.identity.invalid"
        | "journal.setup.installer.process.identity.unavailable"
        | "journal.setup.installer.process.observation.unavailable"
        | "journal.setup.installer.registry.unavailable"
        | "journal.setup.installer.still.running"
        | "journal.setup.installer.stopped"
        | "journal.setup.installer.timed.out" => &[],
        "journal.cli.install.coreml.publication.lock"
        | "journal.cli.install.mlx.directory.read"
        | "journal.cli.install.mlx.entry.read"
        | "journal.cli.install.mlx.type.read"
        | "journal.cli.install.status.lock"
        | "journal.cli.install.windows.executable.read"
        | "journal.cli.journal.helper.inspect"
        | "journal.cli.transcribe.error.process.log"
        | "journal.cli.transcribe.error.process.spawn"
        | "journal.cli.transcribe.error.process.terminate"
        | "journal.setup.installer.cleanup.failed"
        | "journal.setup.installer.job.cleanup" => &["error"],
        "journal.cli.install.coreml.platform" | "journal.cli.install.speech.platform" => {
            &["os_name", "arch"]
        }
        "journal.cli.install.coreml.cache.directory" => &["installed", "configured"],
        "journal.cli.install.coreml.catalog.mismatch" => &["found", "expected"],
        "journal.cli.install.coreml.asset.missing"
        | "journal.cli.install.mlx.file.missing"
        | "journal.cli.install.mlx.file.regular"
        | "journal.cli.install.mlx.input.extra" => &["file"],
        "journal.cli.install.coreml.asset.digest" | "journal.cli.install.coreml.asset.size" => {
            &["file", "expected", "found"]
        }
        "journal.cli.install.coreml.asset.read"
        | "journal.cli.install.mlx.digest.read"
        | "journal.cli.install.mlx.metadata.read" => &["file", "error"],
        "journal.cli.install.coreml.publish.restore" => &["error", "restore"],
        "journal.cli.install.hardware.memory.minimum" => &["available_bytes"],
        "journal.cli.install.metal.platform"
        | "journal.cli.install.pin.missing"
        | "journal.cli.install.registry.platform"
        | "journal.cli.install.windows.platform"
        | "journal.cli.install.windows.target.different" => &["key"],
        "journal.cli.install.registry.artifact.ambiguous"
        | "journal.cli.install.registry.artifact.missing" => &["unit"],
        "journal.cli.install.archive.binary" => &["binary_name"],
        "journal.cli.install.archive.digest" => &["expected", "actual"],
        "journal.cli.install.archive.host" => &["host"],
        "journal.cli.install.archive.scheme" => &["host", "scheme"],
        "journal.cli.install.archive.redirect" => &["limit"],
        "journal.cli.install.archive.userinfo" => &["authority"],
        "journal.cli.install.archive.origin" => &["host", "message"],
        "journal.cli.install.archive.download" => &["message"],
        "journal.cli.install.archive.size" => &["quantity", "expected", "actual"],
        "journal.cli.install.member.cuda.arch.key" => &["artifact_key"],
        "journal.cli.install.member.cuda.arch" => &["arch"],
        "journal.cli.install.member.cuda.missing"
        | "journal.cli.install.member.cuda.regular"
        | "journal.cli.install.member.link.broken"
        | "journal.cli.install.member.link.cycle"
        | "journal.cli.install.member.link.missing"
        | "journal.cli.install.member.link.regular"
        | "journal.cli.install.member.link.type"
        | "journal.cli.install.member.link.unreadable"
        | "journal.cli.install.member.regular.missing"
        | "journal.cli.install.member.regular.type" => &["name"],
        "journal.cli.install.member.link.read" => &["name", "error"],
        "journal.cli.install.member.link.target" => &["name", "target", "found"],
        "journal.cli.install.mlx.file.digest" | "journal.cli.install.mlx.file.size" => {
            &["file", "expected", "actual"]
        }
        "journal.setup.installer.exited" => &["code"],
        "journal.setup.installer.tree.not.reaped" => &["reason", "survivors"],
        "journal.cli.backup.config.lock.timeout" => &["path", "seconds"],
        "journal.cli.transcribe.error.process.admission" => &["reason"],
        "journal.cli.transcribe.error.process.capability" => &["needed"],
        "journal.cli.transcribe.error.process.confirm" => &["pid", "error"],
        "journal.cli.transcribe.error.process.identity" => &["pid"],
        "journal.cli.transcribe.installation.coded" => &["code", "detail"],
        "journal.window.facets.awareness.response.detail.unreadable.settings" => &["settings_path"],
        _ => return None,
    })
}
