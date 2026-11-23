// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-side policy for stored object detections.
//!
//! Describe and depict store the detector's full output at its run threshold.
//! This module is the one place that decides which of those objects are
//! reliable enough to show or search. The floors and suppressions come from
//! a judged evaluation on real journal frames: device classes on a screen
//! capture are the screen itself, and below 0.4 the remaining classes are
//! close to a coin flip.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use solstone_core_window_catalog::{ENGLISH, strings, text};

const DEVICE_CLASSES: &[&str] = &["laptop", "tv", "cell phone"];
const PERSON_CLASS: &str = "person";
const PERSON_MIN_SCORE: f64 = 0.4;
const ALLOWED_CLASSES: &[&str] = &["car", "bird", "tie", "bottle", "cup", "truck", "bowl"];
const ALLOWED_MIN_SCORE: f64 = 0.4;

/// One detection that passed the read-side policy.
#[derive(Clone, Debug, PartialEq)]
pub struct QualifiedObject {
    pub class_name: String,
    pub score: f64,
    /// `[x_min, y_min, x_max, y_max]` in the detector's image pixels.
    pub bbox: Option<[f64; 4]>,
}

/// Objects in a stored `detections` block that pass the read-side policy, in
/// stored order.
pub fn qualified_objects(detections: &Value) -> Vec<QualifiedObject> {
    let screen = detections.get("source").and_then(Value::as_str) == Some("screen");
    detections
        .get("objects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|object| {
            let class_name = object.get("class_name").and_then(Value::as_str)?;
            let score = object.get("score").and_then(Value::as_f64).unwrap_or(0.0);
            let device = DEVICE_CLASSES.contains(&class_name);
            let floor = if screen && device {
                return None;
            } else if class_name == PERSON_CLASS {
                PERSON_MIN_SCORE
            } else if device || ALLOWED_CLASSES.contains(&class_name) {
                ALLOWED_MIN_SCORE
            } else {
                return None;
            };
            (score >= floor).then(|| QualifiedObject {
                class_name: class_name.to_owned(),
                score,
                bbox: bbox(object.get("bbox")),
            })
        })
        .collect()
}

fn bbox(value: Option<&Value>) -> Option<[f64; 4]> {
    let values = value?.as_array()?;
    if values.len() != 4 {
        return None;
    }
    let mut out = [0.0; 4];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = value.as_f64().filter(|value| value.is_finite())?;
    }
    Some(out)
}

/// Class counts, most frequent first, then by name.
pub fn object_counts(objects: &[QualifiedObject]) -> Vec<(String, usize)> {
    let mut counts = BTreeMap::<&str, usize>::new();
    for object in objects {
        *counts.entry(object.class_name.as_str()).or_default() += 1;
    }
    let mut ordered = counts
        .into_iter()
        .map(|(name, count)| (name.to_owned(), count))
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    ordered
}

fn class_label(class_name: &str, locale: &str) -> String {
    let id = match class_name {
        "person" => "journal.window.content.objects.class.person",
        "car" => "journal.window.content.objects.class.car",
        "bird" => "journal.window.content.objects.class.bird",
        "tie" => "journal.window.content.objects.class.tie",
        "bottle" => "journal.window.content.objects.class.bottle",
        "cup" => "journal.window.content.objects.class.cup",
        "truck" => "journal.window.content.objects.class.truck",
        "bowl" => "journal.window.content.objects.class.bowl",
        "laptop" => "journal.window.content.objects.class.laptop",
        "tv" => "journal.window.content.objects.class.tv",
        "cell phone" => "journal.window.content.objects.class.cell.phone",
        _ => return class_name.to_owned(),
    };
    text(locale, id)
}

/// `person ×3, car`, or `None` when nothing qualified.
pub fn objects_summary(objects: &[QualifiedObject]) -> Option<String> {
    objects_summary_for_locale(objects, ENGLISH)
}

/// Localized object summary for owner display.
pub fn objects_summary_for_locale(objects: &[QualifiedObject], locale: &str) -> Option<String> {
    let counts = object_counts(objects);
    (!counts.is_empty()).then(|| {
        counts
            .iter()
            .map(|(name, count)| {
                let localized = class_label(name, locale);
                if *count == 1 {
                    localized
                } else {
                    strings(
                        locale,
                        "journal.window.content.objects.count",
                        [("name", localized.as_str()), ("count", &count.to_string())],
                    )
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
}

/// The `**Objects:**` line for a stored `detections` block.
pub fn objects_line(detections: &Value) -> Option<String> {
    objects_line_for_locale(detections, ENGLISH)
}

/// The localized `**Objects:**` line for a stored `detections` block.
pub fn objects_line_for_locale(detections: &Value, locale: &str) -> Option<String> {
    objects_summary_for_locale(&qualified_objects(detections), locale).map(|summary| {
        strings(
            locale,
            "journal.window.content.objects.line",
            [("summary", summary.as_str())],
        )
    })
}

/// Qualified objects with their boxes and the detector's image size, for a
/// viewer that draws them over the frame. `None` when nothing qualified.
pub fn objects_overlay(detections: &Value) -> Option<Value> {
    objects_overlay_for_locale(detections, ENGLISH)
}

/// Localized overlay carrying translated class labels for owner display.
pub fn objects_overlay_for_locale(detections: &Value, locale: &str) -> Option<Value> {
    let objects = qualified_objects(detections);
    if objects.is_empty() {
        return None;
    }
    let image = detections.get("image").and_then(Value::as_object);
    let dimension = |key: &str| {
        image
            .and_then(|image| image.get(key))
            .and_then(Value::as_f64)
            .filter(|value| value.is_finite() && *value > 0.0)
    };
    let items = objects
        .into_iter()
        .filter_map(|object| {
            Some(json!({
                "label": class_label(&object.class_name, locale),
                "score": object.score,
                "bbox": object.bbox?,
            }))
        })
        .collect::<Vec<_>>();
    Some(Value::Object(Map::from_iter([
        (
            "image".to_owned(),
            json!({"width": dimension("width"), "height": dimension("height")}),
        ),
        ("items".to_owned(), Value::Array(items)),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(source: &str, objects: Value) -> Value {
        json!({
            "engine": "rf-detr.cpp",
            "source": source,
            "image": {"width": 1920, "height": 1080},
            "objects": objects,
        })
    }

    #[test]
    fn screen_policy_suppresses_devices_and_applies_floors() {
        let detections = block(
            "screen",
            json!([
                {"class_name": "person", "score": 0.91, "bbox": [1.0, 2.0, 3.0, 4.0]},
                {"class_name": "person", "score": 0.39},
                {"class_name": "laptop", "score": 0.99},
                {"class_name": "tv", "score": 0.8},
                {"class_name": "car", "score": 0.4},
                {"class_name": "bird", "score": 0.35},
                {"class_name": "hot dog", "score": 0.9},
                {"class_name": "person", "score": 0.5}
            ]),
        );
        let kept = qualified_objects(&detections);
        let names = kept
            .iter()
            .map(|o| o.class_name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["person", "car", "person"]);
        assert_eq!(kept[0].bbox, Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(kept[1].bbox, None);
        assert_eq!(
            objects_line(&detections).as_deref(),
            Some("**Objects:** person ×2, car")
        );
    }

    #[test]
    fn stills_keep_devices_above_the_floor() {
        let detections = block(
            "still",
            json!([
                {"class_name": "laptop", "score": 0.7},
                {"class_name": "cell phone", "score": 0.2},
                {"class_name": "cup", "score": 0.45}
            ]),
        );
        assert_eq!(
            objects_line(&detections).as_deref(),
            Some("**Objects:** cup, laptop")
        );
    }

    #[test]
    fn nothing_qualified_gives_no_line_or_overlay() {
        let detections = block("screen", json!([{"class_name": "laptop", "score": 0.9}]));
        assert_eq!(objects_line(&detections), None);
        assert_eq!(objects_overlay(&detections), None);
        assert_eq!(objects_line(&json!({"source": "screen"})), None);
        assert_eq!(objects_line(&Value::Null), None);
    }

    #[test]
    fn overlay_carries_boxes_and_image_size() {
        let detections = block(
            "screen",
            json!([
                {"class_name": "person", "score": 0.8, "bbox": [10.0, 20.0, 110.0, 220.0]},
                {"class_name": "person", "score": 0.6, "bbox": [1.0, 2.0]}
            ]),
        );
        assert_eq!(
            objects_overlay(&detections),
            Some(json!({
                "image": {"width": 1920.0, "height": 1080.0},
                "items": [{"label": "person", "score": 0.8, "bbox": [10.0, 20.0, 110.0, 220.0]}]
            }))
        );
    }
}
