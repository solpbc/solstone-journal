// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Developer/operator entry point; does not run describe or call a provider.

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn main() -> std::process::ExitCode {
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 3 && args.len() != 4 {
        eprintln!(
            "usage: solstone-face-pass JOURNAL RAW SCREEN_JSONL [PAYLOAD_EXECUTABLE]\nrequires faces.enabled=true; writes private faces sidecars, with no provider calls"
        );
        return std::process::ExitCode::from(2);
    }
    let start = std::time::Instant::now();
    let result = if let Some(executable) = args.get(3) {
        solstone_core_describe::faces::run_segment_using_payload(
            std::path::Path::new(&args[0]),
            std::path::Path::new(&args[1]),
            std::path::Path::new(&args[2]),
            std::path::Path::new(executable),
            &|| false,
        )
    } else {
        solstone_core_describe::faces::run_segment(
            std::path::Path::new(&args[0]),
            std::path::Path::new(&args[1]),
            std::path::Path::new(&args[2]),
            &|| false,
        )
    };
    match result {
        Ok(stats) => {
            println!(
                "{}",
                serde_json::json!({"stats":stats,"wall_seconds":start.elapsed().as_secs_f64()})
            );
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("face pass failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
#[cfg(any(target_os = "android", target_os = "ios"))]
fn main() {
    eprintln!("face pass is available on desktop journals only");
    std::process::exit(2);
}
