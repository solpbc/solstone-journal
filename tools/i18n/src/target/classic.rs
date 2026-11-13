// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::fault::Fault;
use crate::target::{esm, ftl::generate_bundle_ftl, model::TargetBundle};
use std::{fs, path::Path};

/// Bundle the pinned local Fluent modules without eval, imports or a build-time JS dependency.
/// Each module keeps its own scope; only its named exports cross that boundary.
fn vendor_code(root: &Path) -> Result<String, Vec<Fault>> {
    let mut out = String::new();
    for name in [
        "types", "scope", "memoizer", "resolver", "builtins", "bundle", "resource",
    ] {
        let source = fs::read_to_string(root.join(format!("{name}.js")))
            .map_err(|_| vec![Fault::catalog("io-error")])?;
        let mut exports = Vec::new();
        out.push_str(&format!("const {name} = (() => {{\n"));
        for line in source.lines() {
            if let Some(import) = line.strip_prefix("import ") {
                let (names, from) = import
                    .split_once(" from ")
                    .ok_or_else(|| vec![Fault::catalog("classic-vendor-syntax")])?;
                let module = from
                    .strip_prefix("\"./")
                    .and_then(|s| s.strip_suffix(".js\";"))
                    .ok_or_else(|| vec![Fault::catalog("classic-vendor-syntax")])?;
                if !names.starts_with('{') || !names.ends_with('}') {
                    return Err(vec![Fault::catalog("classic-vendor-syntax")]);
                }
                out.push_str(&format!("const {names} = {module};\n"));
            } else if let Some(declaration) = line.strip_prefix("export ") {
                let rest = declaration
                    .strip_prefix("class ")
                    .or_else(|| declaration.strip_prefix("function "))
                    .ok_or_else(|| vec![Fault::catalog("classic-vendor-syntax")])?;
                let identifier = rest.split([' ', '(']).next().unwrap_or_default();
                exports.push(identifier.to_string());
                out.push_str(declaration);
                out.push('\n');
            } else {
                out.push_str(line);
                out.push('\n');
            }
        }
        out.push_str(&format!("return {{ {} }};\n}})();\n", exports.join(", ")));
    }
    out.push_str("const { FluentBundle } = bundle;\nconst { FluentResource } = resource;\n");
    Ok(out)
}

pub fn generate_classic_target(
    bundle: &TargetBundle,
    dest: &Path,
    vendor: &Path,
) -> Result<(), Vec<Fault>> {
    esm::validate_bundle(bundle)?;
    let dir = dest.join("classic").join(&bundle.id);
    fs::create_dir_all(&dir).map_err(|_| vec![Fault::catalog("io-error")])?;
    let mut code = String::from(
        "// SPDX-License-Identifier: AGPL-3.0-only\n// Copyright (c) 2026 sol pbc\n// Generated; regenerate from the catalog. Fluent runtime: see LICENSE.fluent.\n(() => {\n\"use strict\";\n",
    );
    code.push_str(&vendor_code(vendor)?);
    code.push_str("const CATALOGS = {\n");
    for locale in &bundle.locales {
        let ftl = generate_bundle_ftl(bundle, &locale.tag).map_err(|f| vec![f])?;
        code.push_str(&format!(
            "{}: {},\n",
            serde_json::to_string(&locale.tag).unwrap(),
            serde_json::to_string(&ftl).unwrap()
        ));
    }
    code.push_str("};\n");
    let api = esm::generate_esm_api_code(bundle)?;
    for line in api.lines() {
        if !line.starts_with("import ") {
            code.push_str(line.strip_prefix("export ").unwrap_or(line));
            code.push('\n');
        }
    }
    code.push_str(&format!("globalThis.SolstoneI18nBundles ??= Object.create(null);\nglobalThis.SolstoneI18nBundles[{}] = Object.freeze({{render, integer, cardinal, messageIds: Object.freeze(Object.keys(MESSAGES))}});\n}})();\n", serde_json::to_string(&bundle.id).unwrap()));
    fs::write(dir.join("api.js"), ascii_javascript(&code))
        .map_err(|_| vec![Fault::catalog("io-error")])?;
    fs::copy(vendor.join("LICENSE"), dir.join("LICENSE.fluent"))
        .map_err(|_| vec![Fault::catalog("io-error")])?;
    Ok(())
}

// Safari's script admission requires ASCII source; escaping UTF-16 code units
// preserves strings and regular expressions, including astral characters.
fn ascii_javascript(source: &str) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    for ch in source.chars() {
        if ch.is_ascii() {
            output.push(ch);
        } else {
            for unit in ch.encode_utf16(&mut [0; 2]) {
                let _ = write!(output, "\\u{unit:04x}");
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    #[test]
    fn ascii_source_preserves_bmp_and_astral_code_units() {
        assert_eq!(
            super::ascii_javascript("abc\n · 🪨"),
            "abc\n \\u00b7 \\ud83e\\udea8"
        );
    }
}
