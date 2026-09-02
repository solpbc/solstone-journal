// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(unix)]
#[path = "local_end_to_end.rs"]
mod local_end_to_end;
#[cfg(unix)]
#[path = "local_lifecycle.rs"]
mod local_lifecycle;
#[cfg(unix)]
#[path = "local_probe.rs"]
mod local_probe;
#[cfg(unix)]
#[path = "local_truth.rs"]
mod local_truth;
#[cfg(unix)]
#[path = "parakeet_binary_probe.rs"]
mod parakeet_binary_probe;
#[cfg(any(unix, windows))]
#[path = "parakeet_end_to_end.rs"]
mod parakeet_end_to_end;
#[cfg(unix)]
#[path = "parakeet_stop.rs"]
mod parakeet_stop;
