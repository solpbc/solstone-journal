use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
pub struct DeviceGeometry {
    pub free_bytes: Option<u64>,
    pub total_bytes: Option<u64>,
}

pub struct MeasurementCache {
    entry: Option<(Instant, Value)>,
    geometry: DeviceGeometry,
}
pub type SharedMeasurementCache = Arc<Mutex<MeasurementCache>>;
pub fn new() -> SharedMeasurementCache {
    with_geometry(DeviceGeometry {
        free_bytes: None,
        total_bytes: None,
    })
}
pub fn with_geometry(geometry: DeviceGeometry) -> SharedMeasurementCache {
    Arc::new(Mutex::new(MeasurementCache {
        entry: None,
        geometry,
    }))
}
pub fn snapshot(cache: &SharedMeasurementCache) -> Value {
    let mut cache = cache.lock().expect("measurement cache lock");
    if let Some((at, value)) = &cache.entry
        && at.elapsed() <= Duration::from_secs(60)
    {
        return value.clone();
    }
    // The Rust surface deliberately makes unavailable host geometry explicit rather
    // than allowing a zero total to panic the whole handler.
    let value = match cache.geometry.total_bytes {
        Some(0) => {
            json!({"free_bytes": cache.geometry.free_bytes, "total_bytes": 0, "suggested_defaults": Value::Null})
        }
        Some(total) => {
            let floor = (total / 10).max(20_000_000_000).min(total / 4);
            json!({"free_bytes": cache.geometry.free_bytes, "total_bytes": total, "suggested_defaults": {"budget_bytes": total / 2, "floor_bytes": floor}})
        }
        None => {
            json!({"free_bytes": Value::Null, "total_bytes": Value::Null, "suggested_defaults": Value::Null})
        }
    };
    cache.entry = Some((Instant::now(), value.clone()));
    value
}
pub fn invalidate(cache: &SharedMeasurementCache) {
    cache.lock().expect("measurement cache lock").entry = None;
}
