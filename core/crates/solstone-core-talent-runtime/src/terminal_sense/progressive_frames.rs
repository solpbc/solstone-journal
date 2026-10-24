// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Value, json};

pub(crate) fn clock(base_ms: i64, offset: f64) -> String {
    let value = (base_ms + (offset * 1000.0).round_ties_even() as i64).rem_euclid(86_400_000);
    let hour = value / 3_600_000;
    let minute = (value / 60_000) % 60;
    let second = (value / 1_000) % 60;
    let millis = value % 1_000;
    if millis != 0 {
        format!("{hour:02}:{minute:02}:{second:02}.{millis:03}")
    } else {
        format!("{hour:02}:{minute:02}:{second:02}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PrefixExtension {
    pub start_line: usize,
    pub old_line_count: usize,
    pub new_line_count: usize,
    pub old_prefix_utf8_bytes: usize,
}

impl PrefixExtension {
    pub(crate) fn to_json(&self) -> Value {
        json!({
            "start_line": self.start_line,
            "old_line_count": self.old_line_count,
            "new_line_count": self.new_line_count,
            "old_prefix_utf8_bytes": self.old_prefix_utf8_bytes,
        })
    }

    pub(crate) fn from_json(val: &Value) -> Option<Self> {
        Some(Self {
            start_line: val.get("start_line")?.as_u64()? as usize,
            old_line_count: val.get("old_line_count")?.as_u64()? as usize,
            new_line_count: val.get("new_line_count")?.as_u64()? as usize,
            old_prefix_utf8_bytes: val.get("old_prefix_utf8_bytes")?.as_u64()? as usize,
        })
    }
}

pub(crate) fn extension(old: &str, new: &str) -> Option<PrefixExtension> {
    let a: Vec<&str> = old.split('\n').collect();
    let b: Vec<&str> = new.split('\n').collect();
    let mut lo = 0;
    while lo < a.len().min(b.len()) && a[lo] == b[lo] {
        lo += 1;
    }
    let mut hi = 0;
    while hi < (a.len() - lo).min(b.len() - lo) && a[a.len() - hi - 1] == b[b.len() - hi - 1] {
        hi += 1;
    }
    let a_end = if hi > 0 { a.len() - hi } else { a.len() };
    let b_end = if hi > 0 { b.len() - hi } else { b.len() };
    let aa = &a[lo..a_end];
    let bb = &b[lo..b_end];
    let x = aa.join("\n");
    let y = bb.join("\n");
    if x.is_empty() || !y.starts_with(&x) || y.len() <= x.len() {
        return None;
    }
    Some(PrefixExtension {
        start_line: lo,
        old_line_count: aa.len(),
        new_line_count: bb.len(),
        old_prefix_utf8_bytes: x.len(),
    })
}

pub(crate) fn previous(current: &str, change: &PrefixExtension) -> String {
    let lines: Vec<&str> = current.split('\n').collect();
    let lo = change.start_line;
    let end = lo + change.new_line_count;
    let block = lines[lo..end].join("\n");
    let old_bytes = &block.as_bytes()[..change.old_prefix_utf8_bytes];
    let old_str = std::str::from_utf8(old_bytes).expect("valid utf8");
    let old: Vec<&str> = old_str.split('\n').collect();
    assert_eq!(old.len(), change.old_line_count);

    let mut result = Vec::new();
    result.extend_from_slice(&lines[..lo]);
    result.extend_from_slice(&old);
    result.extend_from_slice(&lines[end..]);
    result.join("\n")
}

fn metadata(frame: &Value) -> Value {
    let mut val = frame.clone();
    if let Some(panes) = val
        .get_mut("content")
        .and_then(|c| c.get_mut("tmux"))
        .and_then(|t| t.get_mut("panes"))
        .and_then(Value::as_array_mut)
    {
        for p in panes {
            if let Some(obj) = p.as_object_mut() {
                obj.shift_remove("content");
            }
        }
    }
    val
}

#[derive(Clone, Debug)]
pub(crate) struct Group {
    pub latest: Value,
    pub earlier: Vec<Value>,
}

pub(crate) fn groups(frames: &[Value]) -> Vec<Group> {
    let mut result: Vec<Group> = Vec::new();
    for frame in frames {
        let mut change = None;
        if let Some(last_group) = result.last_mut() {
            let prior = &last_group.latest;
            let a_panes = prior
                .get("content")
                .and_then(|c| c.get("tmux"))
                .and_then(|t| t.get("panes"))
                .and_then(Value::as_array);
            let b_panes = frame
                .get("content")
                .and_then(|c| c.get("tmux"))
                .and_then(|t| t.get("panes"))
                .and_then(Value::as_array);
            if let (Some(a), Some(b)) = (a_panes, b_panes)
                && a.len() == 1
                && b.len() == 1
            {
                let meta_prior = metadata(prior);
                let meta_frame = metadata(frame);
                if meta_prior["content"] == meta_frame["content"] {
                    let text_a = a[0]["content"].as_str().unwrap_or("");
                    let text_b = b[0]["content"].as_str().unwrap_or("");
                    change = extension(text_a, text_b);
                    if let Some(ref ch) = change {
                        assert_eq!(previous(text_b, ch), text_a);
                    }
                }
            }
        }

        if let Some(ch) = change {
            let prior = result.last().unwrap().latest.clone();
            let mut fields = prior;
            if let Some(obj) = fields.as_object_mut() {
                obj.shift_remove("content");
            }
            result.last_mut().unwrap().earlier.push(json!({
                "metadata": fields,
                "prefix_extension": ch.to_json(),
            }));
            result.last_mut().unwrap().latest = frame.clone();
        } else {
            result.push(Group {
                latest: frame.clone(),
                earlier: Vec::new(),
            });
        }
    }
    result
}

pub(crate) fn expand(grouped: &[Group]) -> Vec<Value> {
    let mut restored = Vec::new();
    for group in grouped {
        let mut current = group.latest.clone();
        let mut old = vec![current.clone()];
        for item in group.earlier.iter().rev() {
            let ext = PrefixExtension::from_json(&item["prefix_extension"]).unwrap();
            let cur_content = current["content"]["tmux"]["panes"][0]["content"]
                .as_str()
                .unwrap();
            let prev_content = previous(cur_content, &ext);
            let same_content = current["content"].clone();
            current = item["metadata"].clone();
            current["content"] = same_content;
            current["content"]["tmux"]["panes"][0]["content"] = json!(prev_content);
            old.push(current.clone());
        }
        old.reverse();
        restored.extend(old);
    }
    restored
}
