// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde_json::{Value, json};

use crate::{clock::Clock, segments::day_segment_counts};

pub fn load_master(root: &Path) -> Result<Value, std::io::Error> {
    let path = root.join("timeline.json");
    if !path.is_file() {
        return Ok(json!({}));
    }
    let text = std::fs::read_to_string(path)?;
    serde_json::from_str(&text).map_err(std::io::Error::other)
}

pub fn overview(root: &Path, clock: &Clock) -> Result<Value, std::io::Error> {
    let master = load_master(root)?;
    let counts = day_segment_counts(root, None);
    let today = clock.now().date();
    let months = coverage_months(&master, &counts, today).into_iter().map(|ym| {
        let month = master.get("months").and_then(Value::as_object).and_then(|months| months.get(&ym)).cloned().unwrap_or_else(|| json!({}));
        json!({"ym": ym, "year": ym[0..4].parse::<i32>().unwrap_or_default(), "month_num": ym[4..6].parse::<u32>().unwrap_or_default(), "days_in_month": days_in_month(&ym), "first_weekday": first_weekday(&ym), "day_count": month.get("day_count").and_then(Value::as_u64).unwrap_or(0), "days_with_data": days_with_data(&month)})
    }).collect::<Vec<_>>();
    Ok(
        json!({"now": clock.now().format("%Y-%m-%dT%H:%M:%S").to_string(), "today": today.format("%Y%m%d").to_string(), "generated_at": master.get("generated_at"), "model": master.get("model"), "data_through": rollup_watermark(&master), "months": months}),
    )
}

pub fn month(root: &Path, ym: &str) -> Result<Option<Value>, std::io::Error> {
    let master = load_master(root)?;
    let Some(month) = master
        .get("months")
        .and_then(Value::as_object)
        .and_then(|months| months.get(ym))
    else {
        return Ok(None);
    };
    let days = month.get("days").and_then(Value::as_object).map(|days| days.iter().map(|(day, value)| (day.clone(), json!({"day": day, "generated_at": value.get("generated_at"), "model": value.get("model"), "day_top": value.get("day_top").cloned().unwrap_or_else(|| json!([])), "day_rationale": value.get("day_rationale").and_then(Value::as_str).unwrap_or_default()}))).collect::<BTreeMap<_, _>>()).unwrap_or_default();
    Ok(Some(
        json!({"ym": ym, "generated_at": master.get("generated_at"), "model": master.get("model"), "day_count": month.get("day_count").and_then(Value::as_u64).unwrap_or(0), "days_with_data": days_with_data(month), "days": days}),
    ))
}

pub fn day_rollup(root: &Path, day: &str) -> Result<Value, std::io::Error> {
    let master = load_master(root)?;
    let data = master
        .get("months")
        .and_then(Value::as_object)
        .and_then(|months| months.get(&day[..6]))
        .and_then(|month| month.get("days"))
        .and_then(Value::as_object)
        .and_then(|days| days.get(day));
    Ok(
        json!({"generated_at": data.and_then(|data| data.get("generated_at")), "model": data.and_then(|data| data.get("model")), "day_top": data.and_then(|data| data.get("day_top")).cloned().unwrap_or_else(|| json!([])), "day_rationale": data.and_then(|data| data.get("day_rationale")).and_then(Value::as_str).unwrap_or_default(), "hours": data.and_then(|data| data.get("hours")).cloned().unwrap_or_else(|| json!({}))}),
    )
}

pub fn rollup_watermark(master: &Value) -> Option<String> {
    master
        .get("months")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .flat_map(|(_, month)| days_with_data(month))
        .max()
}

pub fn coverage_months(
    master: &Value,
    counts: &BTreeMap<String, usize>,
    today: NaiveDate,
) -> Vec<String> {
    let mut keys = master
        .get("months")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|(ym, _)| ym.len() == 6 && ym.bytes().all(|byte| byte.is_ascii_digit()))
        .map(|(ym, _)| ym.clone())
        .collect::<Vec<_>>();
    keys.extend(
        counts
            .keys()
            .filter(|day| day.len() == 8 && day.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|day| day[..6].to_owned()),
    );
    if keys.is_empty() {
        return vec![today.format("%Y%m").to_string()];
    }
    keys.sort();
    keys.dedup();
    month_span(
        &keys[0],
        std::cmp::max(
            keys.last().expect("nonempty"),
            &today.format("%Y%m").to_string(),
        ),
    )
}

fn days_with_data(month: &Value) -> Vec<String> {
    if let Some(days) = month.get("days").and_then(Value::as_object) {
        let mut values = days.keys().cloned().collect::<Vec<_>>();
        values.sort();
        return values;
    }
    let mut values = month
        .get("days_with_data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    values.sort();
    values
}

fn month_span(start: &str, end: &str) -> Vec<String> {
    let mut year = start[0..4].parse::<i32>().unwrap_or_default();
    let mut month = start[4..6].parse::<u32>().unwrap_or_default();
    let end_year = end[0..4].parse::<i32>().unwrap_or_default();
    let end_month = end[4..6].parse::<u32>().unwrap_or_default();
    let mut months = Vec::new();
    while (year, month) <= (end_year, end_month) {
        months.push(format!("{year:04}{month:02}"));
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    months
}

fn days_in_month(ym: &str) -> u32 {
    let year = ym[0..4].parse::<i32>().unwrap_or_default();
    let month = ym[4..6].parse::<u32>().unwrap_or(1);
    let start = NaiveDate::from_ymd_opt(year, month, 1)
        .unwrap_or_else(|| NaiveDate::from_ymd_opt(1970, 1, 1).expect("fixed date"));
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .expect("valid month");
    (next - start).num_days() as u32
}
fn first_weekday(ym: &str) -> u32 {
    NaiveDate::from_ymd_opt(
        ym[0..4].parse().unwrap_or(1970),
        ym[4..6].parse().unwrap_or(1),
        1,
    )
    .map_or(3, |date| date.weekday().num_days_from_monday())
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::{
        routes,
        test_support::{fixed_clock, phase_root, write},
    };

    #[tokio::test]
    async fn a1_days_with_data_is_sorted_for_month_and_overview() {
        let root = phase_root("populated");
        let mut master: Value = serde_json::from_str(
            &std::fs::read_to_string(root.path().join("timeline.json")).expect("master"),
        )
        .expect("JSON");
        let days = master["months"]["202605"]["days"]
            .as_object_mut()
            .expect("days");
        let original = days.remove("20260510").expect("day");
        days.insert("20260520".to_owned(), original.clone());
        days.insert("20260510".to_owned(), original);
        master["months"]["202605"]["days_with_data"] = serde_json::json!(["20260520", "20260510"]);
        write(
            &root.path().join("timeline.json"),
            &serde_json::to_string(&master).expect("JSON"),
        );
        for path in [
            "/app/timeline/api/month/202605",
            "/app/timeline/api/overview",
        ] {
            let response = routes(root.path().to_path_buf(), fixed_clock())
                .oneshot(Request::get(path).body(Body::empty()).expect("request"))
                .await
                .expect("response");
            let body: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), usize::MAX)
                    .await
                    .expect("body"),
            )
            .expect("JSON");
            let values = if path.contains("month") {
                body["days_with_data"].as_array().expect("days")
            } else {
                body["months"]
                    .as_array()
                    .expect("months")
                    .iter()
                    .find(|month| month["ym"] == "202605")
                    .expect("month")["days_with_data"]
                    .as_array()
                    .expect("days")
            };
            assert_eq!(
                values,
                &[serde_json::json!("20260510"), serde_json::json!("20260520")]
            );
        }
    }
}
