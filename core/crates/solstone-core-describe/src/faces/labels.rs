// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{FaceDetection, FaceLabel};
use crate::RawOccurrence;
use serde_json::Value;
use std::collections::BTreeMap;

fn normalize(text: &str) -> String {
    let mut depth = 0;
    let mut clean = String::new();
    for c in text
        .split('|')
        .next()
        .unwrap_or("")
        .trim()
        .trim_end_matches("...")
        .trim_end_matches('…')
        .to_lowercase()
        .chars()
    {
        match c {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ if depth == 0 && (c.is_ascii_alphanumeric() || c.is_whitespace()) => clean.push(c),
            _ => {}
        }
    }
    clean.split_whitespace().collect::<Vec<_>>().join(" ")
}
// SequenceMatcher's longest contiguous matching blocks (no junk for short labels).
fn matching(a: &[u8], b: &[u8]) -> usize {
    let (mut ai, mut bi, mut best) = (0, 0, 0);
    let mut prev = vec![0; b.len() + 1];
    for (i, x) in a.iter().enumerate() {
        let mut next = vec![0; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            if x == y {
                next[j + 1] = prev[j] + 1;
                if next[j + 1] > best {
                    best = next[j + 1];
                    ai = i + 1 - best;
                    bi = j + 1 - best;
                }
            }
        }
        prev = next;
    }
    if best == 0 {
        0
    } else {
        best + matching(&a[..ai], &b[..bi]) + matching(&a[ai + best..], &b[bi + best..])
    }
}
fn score(text: &str, name: &str) -> f32 {
    let a = normalize(text);
    let b = normalize(name);
    if a.is_empty() || b.is_empty() {
        return 0.;
    }
    if a == b || (b.len() >= 4 && a.starts_with(&b)) || (a.len() >= 5 && b.starts_with(&a)) {
        return 1.;
    }
    2. * matching(a.as_bytes(), b.as_bytes()) as f32 / (a.len() + b.len()) as f32
}
fn participant_box(p: &Value) -> Option<[f32; 4]> {
    let array = p.get("box_2d")?.as_array()?;
    if array.len() != 4 {
        return None;
    }
    let mut b = [0.; 4];
    for (i, v) in array.iter().enumerate() {
        b[i] = v.as_f64()?.clamp(0., 1000.) as f32;
    }
    (b[2] > b[0] && b[3] > b[1]).then_some(b)
}
pub(super) fn in_box(face: &FaceDetection, parts: &[Value], w: u32, h: u32) -> Option<usize> {
    let cx = (face.bbox[0] + face.bbox[2] / 2.) * 1000. / w as f32;
    let cy = (face.bbox[1] + face.bbox[3] / 2.) * 1000. / h as f32;
    parts.iter().position(|p| {
        participant_box(p)
            .is_some_and(|[y0, x0, y1, x1]| x0 <= cx && cx <= x1 && y0 <= cy && cy <= y1)
    })
}
pub(super) fn meeting_region(parts: &[Value], w: u32, h: u32) -> [u32; 4] {
    let boxes = parts.iter().filter_map(participant_box).collect::<Vec<_>>();
    if boxes.is_empty() {
        return [0, 0, w, h];
    }
    let mut b = [1000_f32, 1000., 0., 0.];
    for v in boxes {
        b[0] = b[0].min(v[0]);
        b[1] = b[1].min(v[1]);
        b[2] = b[2].max(v[2]);
        b[3] = b[3].max(v[3]);
    }
    let py = (b[2] - b[0]) * 0.15 + 20.;
    let px = (b[3] - b[1]) * 0.15 + 20.;
    let x = ((b[1] - px).max(0.) * w as f32 / 1000.) as u32;
    let y = ((b[0] - py).max(0.) * h as f32 / 1000.) as u32;
    let x1 = ((b[3] + px).min(1000.) * w as f32 / 1000.) as u32;
    let y1 = ((b[2] + py).min(1000.) * h as f32 / 1000.) as u32;
    [x.min(w - 1), y.min(h - 1), (x1 - x).max(1), (y1 - y).max(1)]
}
pub(super) fn assign(
    faces: &[FaceDetection],
    parts: &[Value],
    texts: &[RawOccurrence],
) -> BTreeMap<usize, FaceLabel> {
    let mut labels = Vec::new();
    for (idx, p) in parts.iter().enumerate() {
        let Some(name) = p.get("name").and_then(Value::as_str) else {
            continue;
        };
        let best = texts
            .iter()
            .filter(|t| !t.geometry.is_empty())
            .map(|t| (score(&t.text, name), t))
            .reduce(|best, current| if current.0 > best.0 { current } else { best });
        let Some((s, t)) = best.filter(|(s, _)| *s >= 0.82) else {
            continue;
        };
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for [x, y] in &t.geometry {
            b[0] = b[0].min(*x);
            b[1] = b[1].min(*y);
            b[2] = b[2].max(*x);
            b[3] = b[3].max(*y);
        }
        labels.push(FaceLabel {
            text: t.text.clone(),
            bbox: b,
            participant_idx: idx,
            participant_name: name.to_owned(),
            match_score: s,
        });
    }
    let mut assigned: BTreeMap<usize, (usize, FaceLabel)> = BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        let [x, y, w, h] = f.bbox;
        let cx = x + w / 2.;
        let chin = y + h;
        let nearest = labels
            .iter()
            .filter_map(|l| {
                let gap = l.bbox[1] - chin;
                (gap >= -0.25 * h
                    && gap <= 6. * h
                    && cx >= l.bbox[0] - 1.2 * w
                    && cx <= l.bbox[0] + 3.2 * (gap + 2. * h) + w)
                    .then_some((gap, l))
            })
            .min_by(|a, b| {
                a.0.total_cmp(&b.0)
                    .then(a.1.participant_idx.cmp(&b.1.participant_idx))
            });
        if let Some((_, l)) = nearest {
            let entry = assigned.entry(l.participant_idx).or_insert((i, l.clone()));
            if w > faces[entry.0].bbox[2] {
                *entry = (i, l.clone());
            }
        }
    }
    assigned
        .into_values()
        .filter(|(i, _)| faces[*i].bbox[2] >= 32.)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn face(x: f32, y: f32, w: f32, h: f32) -> FaceDetection {
        FaceDetection {
            bbox: [x, y, w, h],
            score: 0.95,
            landmarks: [0.; 10],
        }
    }
    #[test]
    fn labels_normalize_and_match_prefix_and_sequence() {
        assert_eq!(score("Alex Rivera (Guest) | Team", "Alex Rivera"), 1.);
        assert_eq!(score("Alex Riv…", "Alex Rivera"), 1.);
        assert!(score("Alex Rviera", "Alex Rivera") >= 0.82);
        assert_eq!(score("", "Alex"), 0.);
    }
    #[test]
    fn participant_boxes_use_yx_order_and_native_dimensions() {
        let parts = vec![json!({"box_2d":[100,500,500,900]})];
        assert_eq!(
            in_box(&face(1100., 200., 80., 80.), &parts, 2000, 1000),
            Some(0)
        );
        assert_eq!(
            in_box(&face(200., 600., 80., 80.), &parts, 2000, 1000),
            None
        );
    }
    #[test]
    fn equal_label_matches_keep_the_first_ocr_occurrence() {
        let parts = vec![json!({"name":"Alex Rivera"})];
        let text = |y| RawOccurrence {
            line_order: 0,
            text: "Alex Rivera".into(),
            confidence: Some(1.),
            geometry: vec![[100., y], [190., y + 20.]],
            recurrence: 1,
        };
        let assigned = assign(
            &[face(110., 120., 40., 60.)],
            &parts,
            &[text(200.), text(800.)],
        );
        assert_eq!(assigned.len(), 1);
        assert_eq!(assigned[&0].bbox[1], 200.);
    }

    #[test]
    fn nearest_label_gets_only_the_widest_face() {
        let parts = vec![json!({"name":"Alex Rivera"})];
        let text = RawOccurrence {
            line_order: 0,
            text: "Alex Rivera".into(),
            confidence: Some(1.),
            geometry: vec![[100., 200.], [190., 220.]],
            recurrence: 1,
        };
        let faces = [face(110., 120., 40., 60.), face(130., 100., 60., 80.)];
        let assigned = assign(&faces, &parts, &[text]);
        assert_eq!(assigned.len(), 1);
        assert!(assigned.contains_key(&1));
    }
}
