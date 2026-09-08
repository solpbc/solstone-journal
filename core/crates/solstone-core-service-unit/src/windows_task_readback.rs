// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Strict readback of the installed Windows service profile.

use std::collections::BTreeMap;

use quick_xml::{Reader, events::Event};

use super::windows_action::{
    WindowsServiceAction, decode_windows_task_arguments, xml_text_is_valid,
};
use super::windows_task::{WindowsTaskInput, render_windows_task_xml};

const MAX_XML_BYTES: usize = 128 * 1024;
const TASK_NAMESPACE: &str = "http://schemas.microsoft.com/windows/2004/02/mit/task";

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WindowsTaskDefinition {
    pub principal_sid: String,
    pub command: String,
    pub working_directory: String,
    pub action: WindowsServiceAction,
}

/// UTF-16LE with a BOM, matching the task's XML declaration.
pub fn encode_windows_task_xml(xml: &str) -> Result<Vec<u8>, &'static str> {
    if !xml_text_is_valid(xml) || xml.len() > MAX_XML_BYTES / 2 {
        return Err("invalid Windows task XML text or size");
    }
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
    Ok(bytes)
}

/// Read saved task artifacts losslessly. No replacement decoding or implicit
/// UTF-8 fallback can turn malformed guard text into an unguarded artifact.
pub fn decode_windows_task_xml(bytes: &[u8]) -> Result<String, &'static str> {
    if bytes.len() > MAX_XML_BYTES
        || !bytes.starts_with(&[0xff, 0xfe])
        || !bytes.len().is_multiple_of(2)
    {
        return Err("Windows task artifact is not bounded UTF-16LE with BOM");
    }
    let words: Vec<_> = bytes[2..]
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let xml = String::from_utf16(&words).map_err(|_| "Windows task artifact has invalid UTF-16")?;
    if !xml_text_is_valid(&xml) {
        return Err("Windows task artifact has invalid XML text");
    }
    Ok(xml)
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Node {
    text: String,
    attributes: BTreeMap<String, String>,
}

fn elements(xml: &str) -> Result<BTreeMap<String, Node>, &'static str> {
    if xml.len() > MAX_XML_BYTES || !xml_text_is_valid(xml) {
        return Err("invalid Windows task XML size or text");
    }
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut nodes = BTreeMap::new();
    let mut stack = Vec::<String>::new();
    let mut roots = 0;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| "malformed Windows task XML")?;
        let empty = matches!(event, Event::Empty(_));
        match event {
            Event::Start(tag) | Event::Empty(tag) => {
                let name = std::str::from_utf8(tag.name().as_ref())
                    .map_err(|_| "invalid XML name")?
                    .to_owned();
                if name.contains(':') || stack.len() >= 12 || nodes.len() >= 96 {
                    return Err("unsupported Windows task XML structure");
                }
                if stack.is_empty() {
                    roots += 1;
                }
                stack.push(name);
                let mut node = Node::default();
                for attr in tag.attributes() {
                    let attr = attr.map_err(|_| "invalid task attribute")?;
                    let key = std::str::from_utf8(attr.key.as_ref())
                        .map_err(|_| "invalid task attribute name")?
                        .to_owned();
                    let value = attr
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Explicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|_| "invalid task attribute value")?
                        .into_owned();
                    if node.attributes.insert(key, value).is_some() {
                        return Err("duplicate task attribute");
                    }
                }
                if nodes.insert(stack.join("/"), node).is_some() {
                    return Err("duplicate task element");
                }
                if empty {
                    stack.pop();
                }
            }
            Event::End(_) => {
                if stack.pop().is_none() {
                    return Err("unbalanced task XML");
                }
            }
            Event::Text(text) => {
                let text = text.decode().map_err(|_| "invalid task XML text")?;
                if stack.is_empty() {
                    if !text.trim().is_empty() {
                        return Err("text outside task XML");
                    }
                } else {
                    nodes
                        .get_mut(&stack.join("/"))
                        .ok_or("missing task element")?
                        .text
                        .push_str(&text);
                }
            }
            Event::GeneralRef(reference) => {
                let name = reference
                    .decode()
                    .map_err(|_| "invalid task XML reference")?;
                let character = match name.as_ref() {
                    "amp" => '&',
                    "lt" => '<',
                    "gt" => '>',
                    "quot" => '"',
                    "apos" => '\'',
                    name if name.starts_with("#x") => u32::from_str_radix(&name[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or("invalid numeric XML reference")?,
                    name if name.starts_with('#') => name[1..]
                        .parse::<u32>()
                        .ok()
                        .and_then(char::from_u32)
                        .ok_or("invalid numeric XML reference")?,
                    _ => return Err("unknown task XML reference"),
                };
                if !xml_text_is_valid(&character.to_string()) {
                    return Err("invalid referenced XML character");
                }
                nodes
                    .get_mut(&stack.join("/"))
                    .ok_or("task reference outside element")?
                    .text
                    .push(character);
            }
            Event::Decl(_) if roots == 0 => {}
            Event::Comment(_) => {}
            Event::Eof => break,
            _ => return Err("unsupported task XML construct"),
        }
    }
    if roots != 1 || !stack.is_empty() {
        return Err("invalid task XML root");
    }
    let root = nodes.get_mut("Task").ok_or("missing task root")?;
    if root.attributes.get("xmlns").map(String::as_str) != Some(TASK_NAMESPACE)
        || !matches!(
            root.attributes.get("version").map(String::as_str),
            Some("1.3" | "1.4")
        )
    {
        return Err("unexpected task XML namespace or version");
    }
    // Scheduler versions may raise the schema version while preserving the profile.
    root.attributes
        .insert("version".to_owned(), "1.3".to_owned());
    let containers: Vec<_> = nodes
        .keys()
        .filter(|key| {
            nodes
                .keys()
                .any(|other| other.starts_with(&format!("{key}/")))
        })
        .cloned()
        .collect();
    for key in containers {
        let node = nodes.get_mut(&key).ok_or("missing task container")?;
        if !node.text.trim().is_empty() {
            return Err("mixed task XML content");
        }
        node.text.clear();
    }
    Ok(nodes)
}

/// Validate the complete fixed task profile, including action and all four
/// guard fields. Callers must separately compare against the loaded binding
/// and verify the scheduler task/folder security descriptors before mutation.
pub fn parse_windows_task_xml(xml: &str) -> Result<WindowsTaskDefinition, &'static str> {
    let mut nodes = elements(xml)?;
    let text = |path: &str| {
        nodes
            .get(path)
            .map(|node| node.text.clone())
            .ok_or("missing required Windows task field")
    };
    let definition = WindowsTaskDefinition {
        principal_sid: text("Task/Principals/Principal/UserId")?,
        command: text("Task/Actions/Exec/Command")?,
        working_directory: text("Task/Actions/Exec/WorkingDirectory")?,
        action: WindowsServiceAction::parse(&decode_windows_task_arguments(&text(
            "Task/Actions/Exec/Arguments",
        )?)?)?,
    };
    let expected_xml = render_windows_task_xml(&WindowsTaskInput {
        principal_sid: &definition.principal_sid,
        command: &definition.command,
        working_directory: &definition.working_directory,
        action: &definition.action,
    })?;
    let mut expected = elements(&expected_xml)?;
    // Whitespace/quoting normalization of Arguments is safe only after parsing
    // the exact action grammar, with no additional flags or executable action.
    nodes
        .get_mut("Task/Actions/Exec/Arguments")
        .ok_or("missing task arguments")?
        .text = expected
        .get("Task/Actions/Exec/Arguments")
        .ok_or("missing expected arguments")?
        .text
        .clone();
    // Scheduler-owned registration metadata does not change launch semantics.
    for field in ["Date", "Author", "URI"] {
        if let Some(node) = nodes.remove(&format!("Task/RegistrationInfo/{field}"))
            && !node.attributes.is_empty()
        {
            return Err("unexpected registration metadata attributes");
        }
    }
    for (path, value) in [
        ("Task/Triggers/LogonTrigger/Enabled", "true"),
        ("Task/Settings/DisallowStartOnRemoteAppSession", "false"),
    ] {
        if let Some(node) = nodes.remove(path)
            && (!node.attributes.is_empty() || node.text != value)
        {
            return Err("unexpected task scheduling setting");
        }
        expected.remove(path);
    }
    if nodes != expected {
        return Err("Windows task definition differs from the managed profile");
    }
    Ok(definition)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_installation_identity::{Generation, InstallationId, NamespaceName};

    fn xml() -> String {
        let action = WindowsServiceAction {
            port: 6123,
            journal: "C:\\Users\\Zoë\\Journal & notes\\".to_owned(),
            guard: solstone_core_installation_identity::GuardFields {
                namespace: NamespaceName::parse(
                    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                )
                .unwrap(),
                id: InstallationId::parse("0123456789abcdef0123456789abcdef").unwrap(),
                generation: Generation::new(7).unwrap(),
                journal_token: solstone_core_installation_identity::journal_token_from_path(
                    std::path::Path::new(if cfg!(windows) {
                        "C:\\journal"
                    } else {
                        "/journal"
                    }),
                )
                .unwrap(),
            },
        };
        render_windows_task_xml(&WindowsTaskInput {
            principal_sid: "S-1-5-21-1-2-3-1001",
            command: "C:\\Program Files\\Solstone\\journal.exe",
            working_directory: &action.journal,
            action: &action,
        })
        .unwrap()
    }

    #[test]
    fn utf16_saved_artifact_retains_complete_action() {
        let source = xml();
        let bytes = encode_windows_task_xml(&source).unwrap();
        assert_eq!(&bytes[..2], &[0xff, 0xfe]);
        let parsed = parse_windows_task_xml(&decode_windows_task_xml(&bytes).unwrap()).unwrap();
        assert_eq!(parsed.action.port, 6123);
        assert_eq!(parsed.action.guard.generation, Generation::new(7).unwrap());
        assert_eq!(parsed.action.journal, "C:\\Users\\Zoë\\Journal & notes\\");
        assert!(decode_windows_task_xml(source.as_bytes()).is_err());
        assert!(decode_windows_task_xml(&[0xff, 0xfe, 0x00, 0xd8]).is_err());
        assert!(decode_windows_task_xml(&[0xff, 0xfe, 0]).is_err());
    }

    #[test]
    fn refuses_duplicate_actions_triggers_wrong_namespace_and_privilege() {
        let source = xml();
        for altered in [
            source.replace(
                "</Actions>",
                "<Exec><Command>evil.exe</Command></Exec></Actions>",
            ),
            source.replace("</Triggers>", "<BootTrigger/></Triggers>"),
            source.replace("LeastPrivilege", "HighestAvailable"),
            source.replace(TASK_NAMESPACE, "urn:foreign"),
            source.replace("<LogonTrigger>", "<LogonTrigger xmlns=\"urn:foreign\">"),
            source.replace("<Priority>4</Priority>", "<Priority>7</Priority>"),
            source.replace("<Count>10</Count>", "<Count>999</Count>"),
            source.replace("</Task>", "</Task><Task/>"),
            source.replace("<WorkingDirectory>", "<WorkingDirectory>foreign"),
            source.replace("--installation-id", "--installation-namespace"),
        ] {
            assert!(
                parse_windows_task_xml(&altered).is_err(),
                "accepted altered task: {altered}"
            );
        }
    }

    #[test]
    fn requires_unified_engine_and_exact_exec_identity() {
        let source = xml();
        assert!(parse_windows_task_xml(&source).is_ok());
        for altered in [
            source.replace(
                "<UseUnifiedSchedulingEngine>true</UseUnifiedSchedulingEngine>",
                "",
            ),
            source.replace(
                "<UseUnifiedSchedulingEngine>true",
                "<UseUnifiedSchedulingEngine>false",
            ),
            source.replace("id=\"journal-supervisor\"", "id=\"other-action\""),
            source.replace(" id=\"journal-supervisor\"", ""),
            source.replace("version=\"1.3\"", "version=\"1.2\""),
        ] {
            assert!(parse_windows_task_xml(&altered).is_err());
        }
    }

    #[test]
    fn accepts_scheduler_metadata_without_weakening_action_validation() {
        let source = xml().replace("<RegistrationInfo>", "<RegistrationInfo><URI>\\solstone-owner\\install</URI><Date>2026-09-07T00:00:00</Date>")
            .replace("version=\"1.3\"", "version=\"1.4\"")
            .replace("<LogonTrigger>", "<LogonTrigger><Enabled>true</Enabled>");
        assert!(parse_windows_task_xml(&source).is_ok());
    }
}
