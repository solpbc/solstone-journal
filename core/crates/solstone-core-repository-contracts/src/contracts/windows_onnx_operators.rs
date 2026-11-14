// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::{fs, path::Path};

type Operator = (String, u64, String);
type Operators = BTreeSet<Operator>;

// Read only the ONNX protobuf metadata needed by the upstream reduced-build
// generator. Tensor payloads are skipped as borrowed slices, never decoded.
#[derive(Clone, Copy)]
enum Value<'a> {
    Integer(u64),
    Bytes(&'a [u8]),
    Fixed,
}

fn varint(bytes: &[u8], index: &mut usize) -> Result<u64, String> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*index).ok_or("truncated protobuf varint")?;
        *index += 1;
        if shift == 63 && byte > 1 {
            return Err("overflowing protobuf varint".into());
        }
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized protobuf varint".into())
}

fn fields(bytes: &[u8]) -> Result<Vec<(u64, Value<'_>)>, String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let tag = varint(bytes, &mut index)?;
        if tag >> 3 == 0 {
            return Err("zero protobuf field number".into());
        }
        let value = match tag & 7 {
            0 => Value::Integer(varint(bytes, &mut index)?),
            wire @ (1 | 2 | 5) => {
                let len = match wire {
                    1 => 8,
                    5 => 4,
                    _ => usize::try_from(varint(bytes, &mut index)?)
                        .map_err(|_| "protobuf field too large")?,
                };
                let end = index.checked_add(len).ok_or("protobuf length overflow")?;
                let payload = bytes.get(index..end).ok_or("truncated protobuf field")?;
                index = end;
                if wire == 2 {
                    Value::Bytes(payload)
                } else {
                    Value::Fixed
                }
            }
            _ => return Err("unsupported protobuf wire type".into()),
        };
        out.push((tag >> 3, value));
    }
    Ok(out)
}

fn text(value: Value<'_>) -> Result<&str, String> {
    match value {
        Value::Bytes(bytes) => std::str::from_utf8(bytes).map_err(|e| e.to_string()),
        _ => Err("expected protobuf string".into()),
    }
}

fn domain(name: &str) -> &str {
    if name.is_empty() { "ai.onnx" } else { name }
}

fn graph_ops(
    bytes: &[u8],
    imports: &BTreeMap<String, u64>,
    ops: &mut Operators,
) -> Result<(), String> {
    for (field, value) in fields(bytes)? {
        if let (1, Value::Bytes(node)) = (field, value) {
            let node = fields(node)?;
            let mut name = "";
            let mut op_domain = "ai.onnx";
            for &(field, value) in &node {
                match field {
                    4 => name = text(value)?,
                    7 => op_domain = domain(text(value)?),
                    _ => (),
                }
            }
            if name.is_empty() {
                return Err("ONNX node has no operator type".into());
            }
            // The controlled build disables contrib and ML operators. A new
            // domain needs a runtime decision, not an extra config line.
            if op_domain != "ai.onnx" {
                return Err(format!("unsupported ONNX operator domain: {op_domain}"));
            }
            let version = *imports
                .get(op_domain)
                .ok_or("operator has no opset import")?;
            ops.insert((op_domain.into(), version, name.into()));
            for (field, value) in node {
                if let (5, Value::Bytes(attribute)) = (field, value) {
                    for (field, value) in fields(attribute)? {
                        if let (6 | 11, Value::Bytes(graph)) = (field, value) {
                            graph_ops(graph, imports, ops)?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn model_ops(bytes: &[u8]) -> Result<Operators, String> {
    let model = fields(bytes)?;
    let mut imports = BTreeMap::new();
    for &(field, value) in &model {
        if let (8, Value::Bytes(opset)) = (field, value) {
            let mut name = "ai.onnx";
            let mut version = None;
            for (field, value) in fields(opset)? {
                match (field, value) {
                    (1, value) => name = domain(text(value)?),
                    (2, Value::Integer(value)) => version = Some(value),
                    _ => (),
                }
            }
            let version = version
                .filter(|v| *v > 0)
                .ok_or("missing ONNX opset version")?;
            if imports.insert(name.into(), version).is_some() {
                return Err("duplicate ONNX opset import".into());
            }
        }
    }
    let mut ops = Operators::new();
    let mut graphs = 0;
    for (field, value) in model {
        match (field, value) {
            (7, Value::Bytes(graph)) => {
                graphs += 1;
                graph_ops(graph, &imports, &mut ops)?;
            }
            (25, _) => {
                return Err("ONNX local functions need reduced-build generator review".into());
            }
            _ => (),
        }
    }
    if graphs != 1 || ops.is_empty() {
        return Err("expected one nonempty ONNX inference graph".into());
    }
    Ok(ops)
}

fn config_ops(config: &str) -> Operators {
    let mut ops = Operators::new();
    for line in config
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
    {
        let parts: Vec<_> = line.split(';').collect();
        assert_eq!(parts.len(), 3, "unexpected reduced config entry: {line}");
        let version = parts[1].parse().expect("reduced config opset");
        for op in parts[2].split(',') {
            assert!(!op.is_empty(), "empty reduced config operator");
            ops.insert((domain(parts[0]).into(), version, op.into()));
        }
    }
    ops
}

fn windows_models(inventory: &str) -> BTreeSet<String> {
    let inventory = inventory
        .parse::<toml_edit::DocumentMut>()
        .expect("parse inventory");
    inventory["entry"]
        .as_array_of_tables()
        .expect("inventory entries")
        .iter()
        .filter(|entry| entry.get("kind").and_then(toml_edit::Item::as_str) == Some("model-asset"))
        .filter(|entry| {
            entry["targets"]
                .as_array()
                .expect("model targets")
                .iter()
                .any(|target| target.as_str().is_some_and(|s| s.starts_with("windows-")))
        })
        .map(|entry| entry["source"].as_str().expect("model source").to_owned())
        .filter(|source| {
            Path::new(source)
                .extension()
                .is_some_and(|ext| ext == "onnx")
        })
        .collect()
}

#[test]
fn every_windows_onnx_model_is_covered_by_the_reduced_runtime() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let inventory = fs::read_to_string(root.join("core/distribution/inventory.toml")).unwrap();
    let config =
        fs::read_to_string(root.join("core/distribution/onnx-windows-required-operators.config"))
            .unwrap();
    let carried = config_ops(&config);
    let models = windows_models(&inventory);
    assert!(
        !models.is_empty(),
        "Windows model selection must be nonempty"
    );
    for model in models {
        let required = model_ops(&fs::read(root.join(&model)).unwrap())
            .unwrap_or_else(|e| panic!("{model}: {e}"));
        let missing: Vec<_> = required.difference(&carried).collect();
        assert!(
            missing.is_empty(),
            "{model}: Windows reduced runtime lacks {missing:?}"
        );
    }
}

#[test]
fn parser_checks_nested_graphs_and_removed_operator() {
    // Model opset 11; If with a nested Resize. No tensor/runtime dependency.
    let nested = [10, 8, 34, 6, b'R', b'e', b's', b'i', b'z', b'e'];
    let mut attribute = vec![50, nested.len() as u8];
    attribute.extend(nested);
    let mut node = vec![34, 2, b'I', b'f', 42, attribute.len() as u8];
    node.extend(attribute);
    let mut graph = vec![10, node.len() as u8];
    graph.extend(node);
    let mut model = vec![66, 2, 16, 11, 58, graph.len() as u8];
    model.extend(graph);
    let required = model_ops(&model).unwrap();
    let config = config_ops("ai.onnx;11;If,Resize\n");
    assert!(required.is_subset(&config));
    let removed = config_ops("ai.onnx;11;If\n");
    assert_eq!(
        required.difference(&removed).cloned().collect::<Vec<_>>(),
        vec![("ai.onnx".into(), 11, "Resize".into())]
    );
    assert!(!required.is_subset(&config_ops("ai.onnx;18;If,Resize\n")));
    assert!(model_ops(&model[..model.len() - 1]).is_err());
    assert!(fields(&[10, 255]).is_err());
    assert!(fields(&[128; 10]).is_err());
}
