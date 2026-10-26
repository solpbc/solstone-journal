// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded snapshot node listing. A walk that does not account for every node
//! is a refusal, including a short or empty buffer that has no complete summary.

use serde_json::Value;

use super::ReadbackError;

const MAX_NODE_LISTING_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveNodeRecord {
    pub node_type: String,
    pub path: String,
    pub name: String,
    pub size: u64,
}

pub fn parse_bounded_nodes(bytes: &[u8]) -> Result<Vec<ArchiveNodeRecord>, ReadbackError> {
    if bytes.is_empty() || bytes.len() > MAX_NODE_LISTING_BYTES {
        return Err(ReadbackError::IncompleteDiscovery);
    }
    let incomplete = ReadbackError::IncompleteDiscovery;
    let mut nodes = Vec::new();
    let mut summary = None;
    for value in serde_json::Deserializer::from_slice(bytes).into_iter::<Value>() {
        let value = value.map_err(|_| incomplete)?;
        match value.get("message_type").and_then(Value::as_str) {
            Some("node") => {
                if summary.is_some() {
                    return Err(incomplete);
                }
                let node_type = required_text(&value, "type")?;
                let path = required_text(&value, "path")?;
                let name = required_text(&value, "name")?;
                let size = value
                    .get("size")
                    .and_then(Value::as_u64)
                    .ok_or(incomplete)?;
                nodes.push(ArchiveNodeRecord {
                    node_type,
                    path,
                    name,
                    size,
                });
            }
            Some("summary") => {
                if summary.is_some() {
                    return Err(incomplete);
                }
                let complete = value.get("complete").and_then(Value::as_bool);
                let count = value.get("node_count").and_then(Value::as_u64);
                summary = Some((complete, count));
            }
            _ => return Err(incomplete),
        }
    }
    match summary {
        Some((Some(true), Some(count))) if count == nodes.len() as u64 => Ok(nodes),
        _ => Err(incomplete),
    }
}

fn required_text(value: &Value, field: &str) -> Result<String, ReadbackError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
        .ok_or(ReadbackError::IncompleteDiscovery)
}

#[cfg(test)]
mod tests {
    use super::parse_bounded_nodes;
    use crate::archive_readback::ReadbackError;

    fn node(node_type: &str, path: &str, name: &str, size: u64) -> String {
        format!(
            r#"{{"message_type":"node","type":"{node_type}","path":"{path}","name":"{name}","size":{size}}}"#
        )
    }

    fn summary(count: u64) -> String {
        format!(r#"{{"message_type":"summary","complete":true,"node_count":{count}}}"#)
    }

    #[test]
    fn populated_and_complete_empty_listings_account_for_every_node() {
        let populated = format!(
            "{}\n{}\n{}\n",
            node("dir", "retained", "retained", 0),
            node("file", "retained/clip.m4a", "clip.m4a", 4),
            summary(2)
        );
        let nodes = parse_bounded_nodes(populated.as_bytes()).expect("complete listing");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[1].node_type, "file");
        assert_eq!(nodes[1].path, "retained/clip.m4a");
        assert_eq!(nodes[1].name, "clip.m4a");
        assert_eq!(nodes[1].size, 4);
        let empty = parse_bounded_nodes(summary(0).as_bytes()).expect("complete empty");
        assert!(empty.is_empty());
    }

    #[test]
    fn incomplete_walks_refuse_instead_of_returning_a_short_list() {
        let cases = [
            String::new(),
            node("file", "retained/clip.m4a", "clip.m4a", 4),
            format!("{}\n{}", node("file", "a", "a", 1), summary(0)),
            format!(
                "{}\n{}",
                node("file", "a", "a", 1),
                r#"{"message_type":"summary","complete":false,"node_count":1}"#
            ),
            format!(
                "{}\n{}",
                r#"{"message_type":"node","path":"a","name":"a","size":1}"#,
                summary(1)
            ),
            "not-json\n".to_owned(),
            format!(
                "{}\n{}\n",
                node("file", "a", "a", 1),
                r#"{"message_type":"exit","complete":true,"node_count":1}"#
            ),
        ];
        for case in cases {
            assert_eq!(
                parse_bounded_nodes(case.as_bytes()),
                Err(ReadbackError::IncompleteDiscovery)
            );
        }
    }
}
