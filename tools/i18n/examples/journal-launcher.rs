// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Generate the POSIX launcher messages used when the native executable is unavailable.

use i18n_catalog::catalog::LoadedCatalog;
use i18n_catalog::model::ElementKind;
use i18n_catalog::vocab::Channel;
use std::fmt::Write;
use std::path::Path;
use std::{env, fs};

const LAUNCHER: &str = "scripts/root-launchers/solstone";
const BEGIN: &str = "# Begin generated launcher messages\n";
const END: &str = "\n# End generated launcher messages";

fn quoted(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn compile(channel: Channel) -> Result<String, String> {
    let catalog = LoadedCatalog::load_and_validate(Path::new("i18n/cli"), channel, None)
        .map_err(|faults| format!("{faults:?}"))?;
    let locales = catalog
        .registry
        .get_eligible_locales(channel, &["journal-cli".to_owned()], None)
        .map_err(|fault| fault.to_string())?;
    let english = &catalog.config.source_locale;
    let mut output = String::from("write_launcher_error() {\n");
    if locales.len() > 1 {
        output.push_str("    case ${SOLSTONE_LOCALE-} in\n");
        for tag in locales.iter().filter(|tag| *tag != english) {
            let mut pattern = String::new();
            for c in tag.chars() {
                if c.is_ascii_alphabetic() {
                    write!(
                        pattern,
                        "[{}{}]",
                        c.to_ascii_lowercase(),
                        c.to_ascii_uppercase()
                    )
                    .unwrap();
                } else if c == '-' || c.is_ascii_digit() {
                    pattern.push(c);
                } else {
                    return Err(format!("unexpected locale tag {tag}"));
                }
            }
            writeln!(output, "        {pattern}) language={} ;;", quoted(tag)).unwrap();
        }
        writeln!(
            output,
            "        *) language={} ;;\n    esac",
            quoted(english)
        )
        .unwrap();
    } else {
        writeln!(output, "    language={}", quoted(english)).unwrap();
    }
    output.push_str("    case \"$language:$1:$2\" in\n");
    for tag in locales {
        for reason in ["cycle", "dangling", "missing", "executable"] {
            for platform in ["mac", "unix"] {
                let id = format!("journal.cli.sol.launcher.{reason}.{platform}");
                let meta = catalog
                    .metadata
                    .get(&id)
                    .ok_or_else(|| format!("missing {id}"))?;
                let message = catalog.translations[&tag]
                    .get(&id)
                    .ok_or_else(|| format!("missing {tag} {id}"))?;
                let [branch] = message.branches.as_slice() else {
                    return Err(format!("expected plain text for {id}"));
                };
                let mut word = String::new();
                let mut literal = String::new();
                for element in &branch.pattern {
                    match element.kind {
                        ElementKind::Text => literal.push_str(&element.value),
                        ElementKind::Slot => {
                            let slot = meta
                                .protected_slots
                                .iter()
                                .find(|slot| slot.name == element.value)
                                .ok_or_else(|| format!("unknown protected slot in {id}"))?;
                            literal.push_str(&slot.value);
                        }
                        ElementKind::Arg if element.value == "path" => {
                            word.push_str(&quoted(&literal));
                            literal.clear();
                            word.push_str("\"$3\"");
                        }
                        _ => return Err(format!("unsupported launcher argument in {id}")),
                    }
                }
                word.push_str(&quoted(&literal));
                let key = quoted(&format!("{tag}:{platform}:{reason}"));
                writeln!(output, "        {key}) printf '%s\\n' {word} >&2 ;;").unwrap();
            }
        }
    }
    output.push_str("    esac\n}");
    Ok(output)
}

fn main() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    let (channel, check) = match args.as_slice() {
        [channel, rest @ ..] if rest.is_empty() || rest == ["--check"] => {
            let channel = match channel.as_str() {
                "production" => Channel::Production,
                "development" => Channel::Development,
                _ => return Err("unknown launcher catalog channel".into()),
            };
            (channel, !rest.is_empty())
        }
        _ => return Err("expected production|development [--check]".into()),
    };
    let original = fs::read_to_string(LAUNCHER).map_err(|error| error.to_string())?;
    let (prefix, body) = original
        .split_once(BEGIN)
        .ok_or("missing launcher message start")?;
    let (_, suffix) = body.split_once(END).ok_or("missing launcher message end")?;
    let generated = format!("{prefix}{BEGIN}{}{END}{suffix}", compile(channel)?);
    if original != generated {
        if check {
            return Err("Launcher messages are stale. Run make journal-cli-catalog.".into());
        }
        fs::write(LAUNCHER, generated).map_err(|error| error.to_string())?;
    }
    Ok(())
}
