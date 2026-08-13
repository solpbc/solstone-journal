// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use nix::sys::statvfs::statvfs;
use solstone_core_check::{
    CheckInputs, DiskInput, MemoryInput, NvidiaInput, PlatformInput, VulkanInput,
    build_check_report, exit_code, human_output, json_output,
};
use solstone_core_local::{detect_gpus, gpu_probe_ok, probe_nvidia_gpu};

pub(super) fn run(json: bool) -> u8 {
    let report = build_check_report(&host_inputs());
    if json {
        print!("{}", json_output(&report));
    } else {
        print!("{}", human_output(&report));
    }
    exit_code(&report)
}

#[derive(Debug, PartialEq, Eq)]
struct HostPlatform {
    os: String,
    os_version: String,
    arch: String,
}
fn command_text(program: &str, args: &[&str]) -> Option<String> {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}
fn host_platform() -> HostPlatform {
    let raw_os = std::env::consts::OS;
    let os = match raw_os {
        "macos" => "Darwin",
        "linux" => "Linux",
        other => other,
    };
    let arch = if raw_os == "macos" && std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        std::env::consts::ARCH
    };
    let os_version = if raw_os == "macos" {
        command_text("/usr/bin/sw_vers", &["-productVersion"])
            .or_else(|| command_text("sw_vers", &["-productVersion"]))
    } else {
        command_text("uname", &["-r"])
    }
    .unwrap_or_default();
    HostPlatform {
        os: os.into(),
        os_version,
        arch: arch.into(),
    }
}
#[cfg(target_os = "linux")]
fn meminfo() -> Option<(u64, u64)> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = None;
    let mut available = None;
    for line in text.lines() {
        let (key, value) = line.split_once(':')?;
        let kib = value.split_whitespace().next()?.parse::<u64>().ok()?;
        match key {
            "MemTotal" => total = Some(kib * 1024),
            "MemAvailable" => available = Some(kib * 1024),
            _ => {}
        }
    }
    Some((total?, available?))
}
fn memory() -> MemoryInput {
    #[cfg(target_os = "linux")]
    {
        if let Some((total, available)) = meminfo() {
            return MemoryInput {
                total_bytes: (total > 0).then_some(total),
                available_bytes: (total > 0 && available > 0 && available <= total)
                    .then_some(available),
            };
        }
        MemoryInput {
            total_bytes: None,
            available_bytes: None,
        }
    }
    #[cfg(target_os = "macos")]
    {
        let total = command_text("/usr/sbin/sysctl", &["-n", "hw.memsize"])
            .and_then(|text| text.parse().ok()); // psutil's macOS available model uses host_statistics64 inactive + free. vm_stat is a separate snapshot approximation; this branch only yields a warning.
        let available = command_text("/usr/bin/vm_stat", &[]).and_then(|text| {
            let page = text
                .split("page size of ")
                .nth(1)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()?;
            let mut count = 0;
            for line in text.lines() {
                if line.starts_with("Pages free") || line.starts_with("Pages inactive") {
                    count += line
                        .split(':')
                        .nth(1)?
                        .trim()
                        .trim_end_matches('.')
                        .parse::<u64>()
                        .ok()?;
                }
            }
            Some(count * page)
        });
        MemoryInput {
            total_bytes: total,
            available_bytes: available,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    MemoryInput {
        total_bytes: None,
        available_bytes: None,
    }
}
fn free_disk(path: &Path) -> Result<u64, String> {
    let mut root = path.to_path_buf();
    while !root.exists() && root != root.parent().unwrap_or(&root) {
        root = root.parent().unwrap_or(&root).to_path_buf();
    }
    let stat = statvfs(&root).map_err(|error| error.to_string())?;
    // Cast explicitly: `statvfs`'s field widths differ by platform (u64 on Linux,
    // u32 on Darwin), so multiplying them unconverted only compiles on Linux.
    // `solstone-core-local::install::fit_report::free_bytes` already does this.
    let blocks = stat.blocks_available() as u64;
    let fragment_size = stat.fragment_size() as u64;
    blocks
        .checked_mul(fragment_size)
        .ok_or_else(|| "free space overflow".into())
}
fn render_nodes_present_but_inaccessible(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    let nodes = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("renderD"))
        })
        .collect::<Vec<_>>();
    !nodes.is_empty()
        && nodes.iter().all(|path| {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .is_err()
        })
}
fn host_inputs() -> CheckInputs {
    let host = host_platform();
    let journal = super::resolve_process_journal_path()
        .map(|line| line.path)
        .unwrap_or_else(|_| PathBuf::from("journal"));
    let disk = free_disk(&journal)
        .map(|free_bytes| DiskInput::Ok { free_bytes })
        .unwrap_or_else(|message| DiskInput::Error { message });
    let nvidia = probe_nvidia_gpu();
    let (tiering_memory_mib, memory_source) = if let Some(vram) = nvidia.vram_mib {
        (Some(vram), "nvidia_vram")
    } else if let Some(unified) = nvidia.unified_memory_mib {
        (Some(unified), "system_available")
    } else {
        (None, "unavailable")
    };
    let devices = detect_gpus();
    let probe_ok = gpu_probe_ok();
    CheckInputs {
        platform: PlatformInput {
            os: host.os,
            os_version: host.os_version,
            arch: host.arch,
        },
        memory: memory(),
        disk,
        journal_path: journal.display().to_string(),
        nvidia: NvidiaInput {
            detected: nvidia.detected,
            vram_mib: nvidia.vram_mib,
            tiering_memory_mib,
            memory_source: memory_source.into(),
        },
        vulkan: VulkanInput { probe_ok, devices },
        render_nodes_present_but_inaccessible: render_nodes_present_but_inaccessible(Path::new(
            "/dev/dri",
        )),
        gpu_evaluation_error: None,
        version: env!("CARGO_PKG_VERSION").into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn real_host_platform_is_mapped_at_the_consumption_site() {
        let platform = host_platform();
        #[cfg(target_os = "linux")]
        assert_eq!((&platform.os[..], &platform.arch[..]), ("Linux", "x86_64"));
        #[cfg(target_os = "macos")]
        assert_eq!((&platform.os[..], &platform.arch[..]), ("Darwin", "arm64"));
    }
    #[test]
    fn render_nodes_use_constructed_root() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            !render_nodes_present_but_inaccessible(directory.path()),
            "no render nodes"
        );
        let accessible = directory.path().join("renderD128");
        fs::write(&accessible, b"").unwrap();
        assert!(
            !render_nodes_present_but_inaccessible(directory.path()),
            "one R/W node"
        );
        #[cfg(unix)]
        {
            fs::set_permissions(&accessible, fs::Permissions::from_mode(0o000)).unwrap();
            if fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&accessible)
                .is_ok()
            {
                eprintln!("skipping inaccessible-mode assertion: runner bypasses mode bits");
            } else {
                assert!(
                    render_nodes_present_but_inaccessible(directory.path()),
                    "render nodes exist but none is R/W accessible"
                );
            }
        }
    }
    #[test]
    fn binary_version_is_passed_to_renderer() {
        let report = build_check_report(&host_inputs());
        let payload: serde_json::Value = serde_json::from_str(&json_output(&report)).unwrap();
        assert_eq!(payload["version"], env!("CARGO_PKG_VERSION"));
    }
}
