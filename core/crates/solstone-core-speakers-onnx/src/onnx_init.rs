// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read the three tail initializers from the pinned WeSpeaker ONNX file.
//!
//! The conv prefix stays in the original session. These tensors are the
//! unchanged projection: `model.seg_1.weight`, `model.seg_1.bias`, and
//! `mean_vec`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::SpeakerOnnxError;

const WEIGHT_LEN: usize = 256 * 5120;
const VECTOR_LEN: usize = 256;

pub(crate) struct Projection {
    pub(crate) weight: Vec<f32>,
    pub(crate) bias: Vec<f32>,
    pub(crate) mean: Vec<f32>,
}

pub(crate) fn read_projection(path: &Path) -> Result<Projection, SpeakerOnnxError> {
    let mut file = File::open(path).map_err(|error| SpeakerOnnxError::InvalidModelIo {
        detail: format!("could not open speaker model: {error}"),
    })?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| SpeakerOnnxError::InvalidModelIo {
            detail: format!("could not read speaker model: {error}"),
        })?;
    let mut weight = None;
    let mut bias = None;
    let mut mean = None;
    let mut cursor = Cursor::new(&bytes);
    while cursor.remaining() {
        let (field, wire) = cursor.tag().map_err(invalid)?;
        if field == 7 && wire == 2 {
            let graph = cursor.bytes().map_err(invalid)?;
            read_graph(graph, &mut weight, &mut bias, &mut mean)?;
        } else {
            cursor.skip(wire).map_err(invalid)?;
        }
    }
    let weight = weight.ok_or_else(|| missing("model.seg_1.weight"))?;
    let bias = bias.ok_or_else(|| missing("model.seg_1.bias"))?;
    let mean = mean.ok_or_else(|| missing("mean_vec"))?;
    if weight.len() != WEIGHT_LEN || bias.len() != VECTOR_LEN || mean.len() != VECTOR_LEN {
        return Err(SpeakerOnnxError::InvalidModelIo {
            detail: format!(
                "projection shapes were {}, {}, {}",
                weight.len(),
                bias.len(),
                mean.len()
            ),
        });
    }
    Ok(Projection { weight, bias, mean })
}

fn read_graph(
    graph: &[u8],
    weight: &mut Option<Vec<f32>>,
    bias: &mut Option<Vec<f32>>,
    mean: &mut Option<Vec<f32>>,
) -> Result<(), SpeakerOnnxError> {
    let mut cursor = Cursor::new(graph);
    while cursor.remaining() {
        let (field, wire) = cursor.tag().map_err(invalid)?;
        if field == 5 && wire == 2 {
            let tensor = cursor.bytes().map_err(invalid)?;
            if let Some((name, values)) = tensor_f32(tensor).map_err(invalid)? {
                match name {
                    "model.seg_1.weight" => *weight = Some(values),
                    "model.seg_1.bias" => *bias = Some(values),
                    "mean_vec" => *mean = Some(values),
                    _ => {}
                }
            }
        } else {
            cursor.skip(wire).map_err(invalid)?;
        }
    }
    Ok(())
}

fn tensor_f32(bytes: &[u8]) -> Result<Option<(&str, Vec<f32>)>, String> {
    let mut cursor = Cursor::new(bytes);
    let mut dims = Vec::new();
    let mut data_type = None;
    let mut name = None;
    let mut raw = None;
    while cursor.remaining() {
        let (field, wire) = cursor.tag()?;
        match (field, wire) {
            (1, 0) => dims.push(cursor.varint()? as usize),
            (1, 2) => {
                let packed = cursor.bytes()?;
                let mut packed_cursor = Cursor::new(packed);
                while packed_cursor.remaining() {
                    dims.push(packed_cursor.varint()? as usize);
                }
            }
            (2, 0) => data_type = Some(cursor.varint()?),
            (8, 2) => {
                let text = cursor.bytes()?;
                name = Some(std::str::from_utf8(text).map_err(|error| error.to_string())?);
            }
            (9, 2) => raw = Some(cursor.bytes()?),
            _ => cursor.skip(wire)?,
        }
    }
    let Some(name) = name else {
        return Ok(None);
    };
    if !matches!(name, "model.seg_1.weight" | "model.seg_1.bias" | "mean_vec") {
        return Ok(None);
    }
    if data_type != Some(1) {
        return Err(format!("{name} is not float32"));
    }
    let raw = raw.ok_or_else(|| format!("{name} has no raw payload"))?;
    if raw.len() % 4 != 0 {
        return Err(format!("{name} payload is not a float sequence"));
    }
    let values = raw
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    let expect = dims
        .iter()
        .try_fold(1usize, |acc, dim| acc.checked_mul(*dim));
    if expect != Some(values.len()) {
        return Err(format!("{name} dims do not match its payload"));
    }
    Ok(Some((name, values)))
}

fn invalid(detail: String) -> SpeakerOnnxError {
    SpeakerOnnxError::InvalidModelIo { detail }
}

fn missing(name: &str) -> SpeakerOnnxError {
    SpeakerOnnxError::InvalidModelIo {
        detail: format!("speaker model is missing {name}"),
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    index: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, index: 0 }
    }

    fn remaining(&self) -> bool {
        self.index < self.data.len()
    }

    fn tag(&mut self) -> Result<(u64, u32), String> {
        let tag = self.varint()?;
        Ok((tag >> 3, (tag & 7) as u32))
    }

    fn varint(&mut self) -> Result<u64, String> {
        let mut value = 0_u64;
        let mut shift = 0;
        loop {
            if self.index >= self.data.len() {
                return Err("truncated protobuf varint".to_owned());
            }
            let byte = self.data[self.index];
            self.index += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
            shift += 7;
            if shift > 63 {
                return Err("protobuf varint is too wide".to_owned());
            }
        }
    }

    fn bytes(&mut self) -> Result<&'a [u8], String> {
        let len = usize::try_from(self.varint()?).map_err(|error| error.to_string())?;
        let end = self
            .index
            .checked_add(len)
            .ok_or_else(|| "protobuf length overflow".to_owned())?;
        if end > self.data.len() {
            return Err("truncated protobuf bytes".to_owned());
        }
        let slice = &self.data[self.index..end];
        self.index = end;
        Ok(slice)
    }

    fn skip(&mut self, wire: u32) -> Result<(), String> {
        match wire {
            0 => {
                self.varint()?;
            }
            1 => self.advance(8)?,
            5 => self.advance(4)?,
            2 => {
                self.bytes()?;
            }
            _ => return Err(format!("unsupported protobuf wire {wire}")),
        }
        Ok(())
    }

    fn advance(&mut self, count: usize) -> Result<(), String> {
        let end = self
            .index
            .checked_add(count)
            .ok_or_else(|| "protobuf length overflow".to_owned())?;
        if end > self.data.len() {
            return Err("truncated protobuf field".to_owned());
        }
        self.index = end;
        Ok(())
    }
}
