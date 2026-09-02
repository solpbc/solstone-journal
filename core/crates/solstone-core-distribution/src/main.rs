// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use solstone_core_distribution::acquire;
use solstone_core_distribution::ced_windows_source;
use solstone_core_distribution::cleanroom::{
    bind_loopback, plan_text_from_inventory_path, serve_directory, serve_generation_fixture,
    serve_root_from_args,
};
use solstone_core_distribution::discover_and_validate_inventory;
use solstone_core_distribution::onnx_windows_source;
use solstone_core_distribution::parakeet_windows_source;
use solstone_core_distribution::produce::{self, ProduceArgs};
use solstone_core_distribution::publish;

fn usage() -> &'static str {
    "usage: solstone-distribution <validate|produce|publish|sign|acquire|ced-windows|onnx-windows|parakeet-windows|cleanroom-plan|cleanroom-serve|cleanroom-generate-serve|help> [ARG]"
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("validate") => {
            let start = args
                .next()
                .map(PathBuf::from)
                .or_else(|| env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            match discover_and_validate_inventory(&start) {
                Ok(inventory) => {
                    println!(
                        "inventory ok: product={} entries={} denies={}",
                        inventory.product,
                        inventory.entry.len(),
                        inventory.deny.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("acquire") => {
            let rest = args.collect::<Vec<_>>();
            match acquire::run(&rest) {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("ced-windows") => {
            let rest = args.collect::<Vec<_>>();
            match ced_windows_source::run_cli(&rest) {
                Ok(line) => {
                    println!("{line}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("onnx-windows") => {
            let rest = args.collect::<Vec<_>>();
            match onnx_windows_source::run_cli(&rest) {
                Ok(line) => {
                    println!("{line}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("parakeet-windows") => {
            let rest = args.collect::<Vec<_>>();
            match parakeet_windows_source::run_cli(&rest) {
                Ok(line) => {
                    println!("{line}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("publish") => {
            let rest = args.collect::<Vec<_>>();
            match publish::run_cli(&rest) {
                Ok(report) => {
                    println!(
                        "published lane={} version={} dest={}",
                        report.lane,
                        report.version,
                        report.dest.display()
                    );
                    for path in report.objects {
                        println!("{}", path.display());
                    }
                    println!("{}", report.latest.display());
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("produce") => {
            let target = args.next();
            let dest = args.next();
            match (target, dest) {
                (Some(target), Some(dest)) => {
                    let start = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                    match produce::run(ProduceArgs {
                        target_id: target,
                        dest: PathBuf::from(dest),
                        start,
                    }) {
                        Ok(report) => {
                            println!(
                                "produced target={} commit={} lock_sha256={} onnx_wheel_sha256={} onnx_source={}",
                                report.target,
                                report.commit,
                                report.lock_sha256,
                                report.onnx_wheel_sha256,
                                report.onnx_source
                            );
                            for path in report.artifacts {
                                println!("{}", path.display());
                            }
                            ExitCode::SUCCESS
                        }
                        Err(error) => {
                            eprintln!("{error}");
                            ExitCode::from(2)
                        }
                    }
                }
                _ => {
                    eprintln!("{}", usage());
                    ExitCode::from(2)
                }
            }
        }
        Some("sign") => {
            solstone_core_distribution::cli_sign::run(&args.collect::<Vec<_>>(), usage())
        }
        Some("cleanroom-plan") => {
            let start = args
                .next()
                .map(PathBuf::from)
                .or_else(|| env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("."));
            let path = start.join("core/distribution/inventory.toml");
            let inventory_path = if path.is_file() { path } else { start.clone() };
            match plan_text_from_inventory_path(&inventory_path) {
                Ok(text) => {
                    print!("{text}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("cleanroom-serve") => {
            let rest = args.collect::<Vec<_>>();
            match serve_root_from_args(&rest) {
                Ok(root) => match bind_loopback() {
                    Ok((listener, port)) => {
                        println!("127.0.0.1:{port}");
                        let _ = std::io::Write::flush(&mut std::io::stdout());
                        match serve_directory(listener, &root) {
                            Ok(()) => ExitCode::SUCCESS,
                            Err(error) => {
                                eprintln!("{error}");
                                ExitCode::from(2)
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        ExitCode::from(2)
                    }
                },
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            }
        }
        Some("cleanroom-generate-serve") => {
            let evidence = args.next().map(PathBuf::from);
            let expected = args.next().map(PathBuf::from);
            match (evidence, expected) {
                (Some(evidence), Some(expected)) => match std::fs::read_to_string(&expected) {
                    Ok(expected) => match bind_loopback() {
                        Ok((listener, port)) => {
                            println!("127.0.0.1:{port}");
                            let _ = std::io::Write::flush(&mut std::io::stdout());
                            match serve_generation_fixture(listener, &evidence, &expected) {
                                Ok(()) => ExitCode::SUCCESS,
                                Err(error) => {
                                    eprintln!("{error}");
                                    ExitCode::from(2)
                                }
                            }
                        }
                        Err(error) => {
                            eprintln!("{error}");
                            ExitCode::from(2)
                        }
                    },
                    Err(error) => {
                        eprintln!("read expected fragment {}: {error}", expected.display());
                        ExitCode::from(2)
                    }
                },
                _ => {
                    eprintln!(
                        "usage: solstone-distribution cleanroom-generate-serve EVIDENCE EXPECTED_FRAGMENT_FILE"
                    );
                    ExitCode::from(2)
                }
            }
        }
        Some("help" | "--help" | "-h") | None => {
            println!("{}", usage());
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("unknown command {other:?}");
            println!("{}", usage());
            ExitCode::from(2)
        }
    }
}
