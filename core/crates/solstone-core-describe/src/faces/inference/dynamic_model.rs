// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Narrow ONNX protobuf shape edit for the digest-verified YuNet graph.
//! Model(7) -> Graph(11/12) -> ValueInfo(2) -> Type(1) -> Tensor(2) -> Shape(1).
//! Initializers and nodes are copied byte for byte. Nothing is written to disk.

use super::PassResult;

fn varint(bytes: &[u8], index: &mut usize) -> PassResult<u64> {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*index).ok_or("truncated ONNX field")?;
        *index += 1;
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    Err("oversized ONNX varint".into())
}
fn put(mut value: u64, out: &mut Vec<u8>) {
    while value >= 128 {
        out.push((value as u8 & 127) | 128);
        value >>= 7;
    }
    out.push(value as u8);
}

fn rewrite(
    bytes: &[u8],
    mut change: impl FnMut(u64, &[u8]) -> PassResult<Option<Vec<u8>>>,
) -> PassResult<Vec<u8>> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        let tag = varint(bytes, &mut index)?;
        let field = tag >> 3;
        let wire = tag & 7;
        if wire == 2 {
            let length =
                usize::try_from(varint(bytes, &mut index)?).map_err(|_| "ONNX field too large")?;
            let end = index
                .checked_add(length)
                .filter(|end| *end <= bytes.len())
                .ok_or("truncated ONNX payload")?;
            if let Some(replacement) = change(field, &bytes[index..end])? {
                if !replacement.is_empty() {
                    put(tag, &mut out);
                    put(replacement.len() as u64, &mut out);
                    out.extend(replacement);
                }
            } else {
                out.extend_from_slice(&bytes[start..end]);
            }
            index = end;
        } else {
            match wire {
                0 => {
                    varint(bytes, &mut index)?;
                }
                1 => index += 8,
                5 => index += 4,
                _ => return Err("unsupported ONNX wire type".into()),
            }
            out.extend_from_slice(bytes.get(start..index).ok_or("truncated ONNX value")?);
        }
    }
    Ok(out)
}
fn shape(bytes: &[u8], input: bool) -> PassResult<Vec<u8>> {
    let mut dimension = 0;
    rewrite(bytes, |field, _| {
        if field != 1 {
            return Ok(None);
        }
        let i = dimension;
        dimension += 1;
        if (input && i >= 2) || (!input && i == 1) {
            let name = format!("face_dim_{i}");
            let mut dim = vec![18];
            put(name.len() as u64, &mut dim);
            dim.extend(name.bytes());
            Ok(Some(dim))
        } else {
            Ok(None)
        }
    })
}
fn value_info(bytes: &[u8], input: bool) -> PassResult<Vec<u8>> {
    rewrite(bytes, |field, bytes| {
        if field == 2 {
            rewrite(bytes, |field, bytes| {
                if field == 1 {
                    rewrite(bytes, |field, bytes| {
                        if field == 2 {
                            shape(bytes, input).map(Some)
                        } else {
                            Ok(None)
                        }
                    })
                    .map(Some)
                } else {
                    Ok(None)
                }
            })
            .map(Some)
        } else {
            Ok(None)
        }
    })
}
pub(super) fn yunet(bytes: &[u8]) -> PassResult<Vec<u8>> {
    let mut found = false;
    let output = rewrite(bytes, |field, bytes| {
        if field == 7 {
            found = true;
            rewrite(bytes, |field, bytes| {
                if field == 11 || field == 12 {
                    value_info(bytes, field == 11).map(Some)
                } else if field == 13 {
                    Ok(Some(Vec::new()))
                } else {
                    Ok(None)
                }
            })
            .map(Some)
        } else {
            Ok(None)
        }
    })?;
    if !found {
        return Err("YuNet graph missing".into());
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shape_edit_preserves_weights_and_static_batch_channels() {
        let static_dim = |value: u8| vec![10, 2, 8, value];
        let mut shape_bytes = Vec::new();
        for v in [1, 3, 64, 64] {
            shape_bytes.extend(static_dim(v));
        }
        let patched = shape(&shape_bytes, true).unwrap();
        assert!(patched.starts_with(&[10, 2, 8, 1, 10, 2, 8, 3]));
        assert!(
            std::str::from_utf8(&patched)
                .unwrap()
                .contains("face_dim_2")
        );
        // Unknown graph fields, including node and initializer bytes, pass through.
        assert_eq!(
            rewrite(&[42, 3, 1, 2, 3], |_, _| Ok(None)).unwrap(),
            [42, 3, 1, 2, 3]
        );
    }
    #[test]
    fn malformed_model_is_refused() {
        assert!(yunet(&[58, 20, 1]).is_err());
        assert!(yunet(&[8, 1]).is_err());
    }
}
