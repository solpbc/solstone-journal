// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Shared Windows clang-cl host-tool closure for ARM source recipes.
//! Each dependency retains responsibility for its source, flags and outputs.

use crate::controlled_build::InputIdentityEntry;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkDeny {
    pub label: String,
    pub program: String,
    pub enabled: bool,
    pub direction: String,
    pub action: String,
    pub profile: String,
    pub protocol: String,
    pub local_address: Vec<String>,
    pub remote_address: Vec<String>,
    pub local_port: Vec<String>,
    pub remote_port: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildValidation {
    pub schema: String,
    pub work_root: String,
    pub tool_paths: BTreeMap<String, String>,
    pub network_denies: Vec<NetworkDeny>,
    pub cache: String,
    pub build_log: String,
    pub install_manifest: String,
    pub python_path_configuration: String,
}

pub(crate) fn windows_path(value: &str) -> Result<String, String> {
    let folded = value.replace('\\', "/").to_ascii_lowercase();
    let b = folded.as_bytes();
    if b.len() < 4
        || !b[0].is_ascii_alphabetic()
        || b[1..3] != *b":/"
        || folded[3..]
            .split('/')
            .any(|p| p.is_empty() || matches!(p, "." | "..") || p.contains([':', '\0']))
    {
        return Err("OpenMP capture requires ordinary absolute Windows paths".into());
    }
    Ok(folded)
}

pub(crate) fn parse_cache(text: &str) -> Result<BTreeMap<&str, &str>, String> {
    let mut cache = BTreeMap::new();
    for line in text
        .lines()
        .filter(|l| !l.starts_with(['#', '/']) && !l.is_empty())
    {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.split_once(':').map_or(key, |(key, _)| key);
        if cache.insert(key, value).is_some() {
            return Err("duplicate OpenMP cache key".into());
        }
    }
    Ok(cache)
}

pub(crate) struct ToolCensus<'a> {
    pub archive_tools: &'a [InputIdentityEntry],
    pub system_tools: &'a [InputIdentityEntry],
    pub network_denied_tools: &'a [String],
}

pub(crate) fn pinned_tools() -> Vec<InputIdentityEntry> {
    serde_json::from_slice(include_bytes!("openmp_windows_tools.json"))
        .expect("committed Windows clang host-tool census")
}

pub(crate) fn validate_tool_census(census: &ToolCensus<'_>) -> Result<(), String> {
    if census.archive_tools != pinned_tools().as_slice() {
        return Err("Windows clang archive-tool identities differ".into());
    }
    let mut tools = BTreeSet::new();
    for tool in census.system_tools {
        let Some((prefix, name)) = tool.label.split_once('/') else {
            return Err("invalid OpenMP system-tool label".into());
        };
        if !matches!(prefix, "msvc" | "sdk" | "windows")
            || !name.ends_with(".exe")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || tool.size == 0
            || !valid_digest(&tool.sha256)
            || !tools.insert(tool.label.clone())
        {
            return Err("invalid or duplicate OpenMP system tool".into());
        }
    }
    for required in [
        "msvc/link.exe",
        "msvc/lib.exe",
        "msvc/nmake.exe",
        "sdk/rc.exe",
        "sdk/mt.exe",
        "windows/cmd.exe",
        "windows/powershell.exe",
        "windows/git.exe",
        "windows/recorder.exe",
    ] {
        if !tools.contains(required) {
            return Err(format!("OpenMP system-tool census is missing {required}"));
        }
    }
    tools.extend(census.archive_tools.iter().map(|t| t.label.clone()));
    if census.network_denied_tools != tools.into_iter().collect::<Vec<_>>().as_slice() {
        return Err("OpenMP outbound denial does not cover its complete tool census".into());
    }
    Ok(())
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn validate_host_capture(
    census: &ToolCensus<'_>,
    validation: &BuildValidation,
) -> Result<(), String> {
    validate_tool_census(census)?;
    let work = windows_path(&validation.work_root)?;
    let cache = parse_cache(&validation.cache)?;
    for key in [
        "CMAKE_C_COMPILER",
        "CMAKE_CXX_COMPILER",
        "CMAKE_ASM_COMPILER",
    ] {
        let actual = windows_path(cache.get(key).ok_or("OpenMP cache compiler is missing")?)?;
        if actual != format!("{work}/llvm/bin/clang-cl.exe") {
            return Err("OpenMP cache uses a foreign compiler".into());
        }
    }
    if windows_path(
        cache
            .get("Python3_EXECUTABLE")
            .ok_or("OpenMP Python path missing")?,
    )? != format!("{work}/python/python.exe")
    {
        return Err("OpenMP cache uses a foreign Python interpreter".into());
    }
    let labels: BTreeSet<_> = census
        .archive_tools
        .iter()
        .chain(census.system_tools)
        .map(|tool| tool.label.as_str())
        .collect();
    if validation
        .tool_paths
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>()
        != labels
        || validation.network_denies.len() != labels.len()
    {
        return Err("OpenMP tool paths or firewall-rule population differ".into());
    }
    if windows_path(
        cache
            .get("CMAKE_MAKE_PROGRAM")
            .ok_or("OpenMP NMake path missing")?,
    )? != windows_path(&validation.tool_paths["msvc/nmake.exe"])?
    {
        return Err("OpenMP cache uses a foreign make program".into());
    }
    let known_paths: BTreeSet<_> = validation
        .tool_paths
        .values()
        .map(|p| windows_path(p))
        .collect::<Result<_, _>>()?;
    for key in [
        "CMAKE_LINKER",
        "CMAKE_AR",
        "CMAKE_MT",
        "CMAKE_RC_COMPILER",
        "CMAKE_C_COMPILER_AR",
        "CMAKE_CXX_COMPILER_AR",
        "CMAKE_NM",
        "CMAKE_OBJCOPY",
        "CMAKE_DLLTOOL",
    ] {
        if let Some(path) = cache.get(key)
            && !path.is_empty()
            && !path.ends_with("-NOTFOUND")
            && !known_paths.contains(&windows_path(path)?)
        {
            return Err(format!("OpenMP cache uses an uncensused tool at {key}"));
        }
    }
    for tool in census.archive_tools {
        if windows_path(&validation.tool_paths[&tool.label])?
            != format!("{work}/{}", tool.label.to_ascii_lowercase())
        {
            return Err("OpenMP archive tool path differs from its extracted root".into());
        }
    }
    for (rule, label) in validation
        .network_denies
        .iter()
        .zip(census.network_denied_tools)
    {
        if rule.label != *label
            || !rule.enabled
            || rule.direction != "Outbound"
            || rule.action != "Block"
            || rule.profile != "Any"
            || rule.protocol != "Any"
            || rule.local_address != ["Any"]
            || rule.remote_address != ["Any"]
            || rule.local_port != ["Any"]
            || rule.remote_port != ["Any"]
            || windows_path(&rule.program)? != windows_path(&validation.tool_paths[label])?
        {
            return Err("OpenMP firewall rule differs from the complete outbound deny".into());
        }
    }
    Ok(())
}
