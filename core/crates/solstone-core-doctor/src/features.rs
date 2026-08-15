// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native temporary twin of `solstone/think/features.py`; retire after the native implementation is complete.

use std::path::Path;

use crate::vocabulary::Platform;

pub struct Feature {
    pub name: &'static str,
    pub summary: &'static str,
    pub modules: &'static [(&'static str, &'static str)],
    pub apt: &'static [&'static str],
    pub brew: &'static [&'static str],
}
pub const FEATURES: &[Feature] = &[
    Feature {
        name: "pdf-import",
        summary: "PDF document extraction",
        modules: &[("pypdfium2", "pypdfium2"), ("PIL", "Pillow")],
        apt: &[],
        brew: &[],
    },
    Feature {
        name: "pdf-export",
        summary: "PDF export rendering",
        modules: &[("weasyprint", "weasyprint")],
        apt: &["libpango-1.0-0", "libpangoft2-1.0-0"],
        brew: &["pango"],
    },
];
pub fn find(name: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|feature| feature.name == name)
}
pub fn available(feature: &Feature, env: &Path) -> bool {
    feature
        .modules
        .iter()
        .all(|(module, distribution)| module_present(env, module, distribution))
}
pub fn hint(feature: &Feature, platform: Platform) -> String {
    let mut hint = format!("pip install 'solstone[{}]'", feature.name);
    let packages = if platform == Platform::Linux {
        feature.apt
    } else {
        feature.brew
    };
    if !packages.is_empty() {
        hint.push_str(&format!(
            " and {} install {}",
            if platform == Platform::Linux {
                "apt"
            } else {
                "brew"
            },
            packages.join(" ")
        ));
    }
    hint
}
fn module_present(env: &Path, module: &str, distribution: &str) -> bool {
    let Ok(libs) = std::fs::read_dir(env.join("lib")) else {
        return false;
    };
    libs.filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("python"))
        .any(|entry| {
            ["site-packages", "dist-packages"].iter().any(|site| {
                let root = entry.path().join(site);
                root.join(module).join("__init__.py").is_file()
                    || root.join(format!("{module}.py")).is_file()
                    || std::fs::read_dir(&root).ok().is_some_and(|entries| {
                        entries.filter_map(Result::ok).any(|entry| {
                            entry.file_name().to_string_lossy().starts_with(module)
                                && matches!(
                                    entry.path().extension().and_then(|value| value.to_str()),
                                    Some("so") | Some("dylib") | Some("pyd")
                                )
                        })
                    })
                    || std::fs::read_dir(&root).ok().is_some_and(|entries| {
                        entries.filter_map(Result::ok).any(|entry| {
                            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                            name.starts_with(&distribution.to_ascii_lowercase().replace('-', "_"))
                                && name.ends_with(".dist-info")
                                && entry.path().join("RECORD").is_file()
                        })
                    })
            })
        })
}
