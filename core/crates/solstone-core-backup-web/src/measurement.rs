use serde_json::{Value, json};
use std::{
    path::Path,
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
pub fn new(journal_root: &Path) -> SharedMeasurementCache {
    cache(device_geometry(journal_root))
}

#[cfg(unix)]
fn device_geometry(journal_root: &Path) -> DeviceGeometry {
    let Some(stats) = nix::sys::statvfs::statvfs(journal_root).ok() else {
        return DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        };
    };
    let fragment_size = Some(stats.fragment_size())
        .filter(|size| *size > 0)
        .or(Some(stats.block_size()))
        .filter(|size| *size > 0);
    let Some(fragment_size) = fragment_size else {
        return DeviceGeometry {
            free_bytes: None,
            total_bytes: None,
        };
    };
    // `statvfs`'s BLOCK COUNTS are u64 on Linux and u32 on Darwin (the sizes
    // are u64 on both), so these products only compile on Linux without a
    // widening cast -- and this one crate's break took the WHOLE solstone-core
    // build down on macOS, not just its own tests. The allow is required
    // because the lint is platform-blind: on Linux it sees u64 -> u64 and
    // cannot see that the same cast is load-bearing on Darwin. Same class as
    // `solstone-core-import-sources::archive`, `solstone-core::check` and
    // `solstone-core-local::install::fit_report::free_bytes`; the first two
    // escape the lint through their expression shape, which is luck rather
    // than a pattern to copy.
    #[allow(clippy::unnecessary_cast)]
    let free_blocks = stats.blocks_free() as u64;
    #[allow(clippy::unnecessary_cast)]
    let total_blocks = stats.blocks() as u64;
    DeviceGeometry {
        free_bytes: free_blocks.checked_mul(fragment_size),
        total_bytes: total_blocks.checked_mul(fragment_size),
    }
}

#[cfg(windows)]
fn device_geometry(journal_root: &Path) -> DeviceGeometry {
    DeviceGeometry {
        free_bytes: solstone_core_offload::measurement::device_free_bytes(journal_root).ok(),
        total_bytes: solstone_core_offload::measurement::device_total_bytes(journal_root).ok(),
    }
}

#[cfg(not(any(unix, windows)))]
fn device_geometry(_journal_root: &Path) -> DeviceGeometry {
    DeviceGeometry {
        free_bytes: None,
        total_bytes: None,
    }
}

#[cfg(test)]
pub fn with_geometry(geometry: DeviceGeometry) -> SharedMeasurementCache {
    cache(geometry)
}

fn cache(geometry: DeviceGeometry) -> SharedMeasurementCache {
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
