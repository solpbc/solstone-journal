// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Manual qualification tool, invoked by hand, not part of the journal, not a CI gate, the host comes from the caller.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ring::rand::SecureRandom;
use solstone_core_spp_attest::PcrMode;
use solstone_core_spp_ratls::qualification::{
    QualificationRequest, qualification_policy, run_qualification,
};
use solstone_core_spp_ratls::{
    ProductionCompositeVerifier, classify_nvattest_prerequisite, ensure_nvattest_installed,
};

const USAGE: &str = "\
Usage: qualification_probe --host <HOST> [--port <PORT>] --pcr-mode <record|pin> [--pin <HEX>] --output-dir <DIR> [--model <MODEL>] [--credential-file <PATH>] [--nvattest-dir <DIR>] [--no-content]

Manual qualification tool for SPP RA-TLS endpoints.

Options:
  --host <HOST>           Target host (required, no default host)
  --port <PORT>           Target port (defaults to 9443)
  --pcr-mode <MODE>       Attestation mode: record or pin (required)
  --pin <HEX>             Expected PCR SHA-256 fingerprint (64 hex characters; required with pin, forbidden with record)
  --output-dir <DIR>      Output directory for evidence files (required)
  --model <MODEL>         Model name for qualification chat probe (required unless --no-content)
  --credential-file <PATH> File holding the owner credential sent as the bearer (required unless --no-content)
  --nvattest-dir <DIR>    Path to nvattest directory (defaults to SPP_NVATTEST_DIR env var)
  --no-content            Captures evidence and does not send chat or transcription
  --help                  Show this help message and exit
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "--help") {
        print!("{USAGE}");
        std::process::exit(0);
    }

    let mut host: Option<String> = None;
    let mut port: u16 = 9443;
    let mut pcr_mode: Option<PcrMode> = None;
    let mut pin: Option<String> = None;
    let mut output_dir: Option<PathBuf> = None;
    let mut model: Option<String> = None;
    let mut credential_file: Option<PathBuf> = None;
    let mut nvattest_dir: Option<PathBuf> = None;
    let mut no_content = false;

    let mut seen_flags = BTreeSet::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if !arg.starts_with("--") {
            eprintln!("error: unexpected positional argument '{arg}'");
            std::process::exit(1);
        }

        let flag_name = arg.as_str();
        if !seen_flags.insert(flag_name.to_string()) {
            eprintln!("error: duplicate flag '{flag_name}'");
            std::process::exit(1);
        }

        match flag_name {
            "--host" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --host");
                    std::process::exit(1);
                }
                host = Some(args[i].clone());
            }
            "--port" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --port");
                    std::process::exit(1);
                }
                match args[i].parse::<u16>() {
                    Ok(p) if p > 0 => port = p,
                    _ => {
                        eprintln!("error: invalid port '{}'", args[i]);
                        std::process::exit(1);
                    }
                }
            }
            "--pcr-mode" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --pcr-mode");
                    std::process::exit(1);
                }
                match args[i].as_str() {
                    "record" => pcr_mode = Some(PcrMode::Record),
                    "pin" => pcr_mode = Some(PcrMode::Pin),
                    other => {
                        eprintln!(
                            "error: invalid --pcr-mode '{other}', expected 'record' or 'pin'"
                        );
                        std::process::exit(1);
                    }
                }
            }
            "--pin" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --pin");
                    std::process::exit(1);
                }
                pin = Some(args[i].clone());
            }
            "--output-dir" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --output-dir");
                    std::process::exit(1);
                }
                output_dir = Some(PathBuf::from(&args[i]));
            }
            "--model" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --model");
                    std::process::exit(1);
                }
                model = Some(args[i].clone());
            }
            "--credential-file" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --credential-file");
                    std::process::exit(1);
                }
                credential_file = Some(PathBuf::from(&args[i]));
            }
            "--nvattest-dir" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --nvattest-dir");
                    std::process::exit(1);
                }
                nvattest_dir = Some(PathBuf::from(&args[i]));
            }
            "--no-content" => {
                no_content = true;
            }
            unknown => {
                eprintln!("error: unknown flag '{unknown}'");
                std::process::exit(1);
            }
        }
        i += 1;
    }

    let host = match host {
        Some(h) if !h.trim().is_empty() => h,
        _ => {
            eprintln!("error: --host is required");
            std::process::exit(1);
        }
    };

    let pcr_mode = match pcr_mode {
        Some(m) => m,
        None => {
            eprintln!("error: --pcr-mode is required");
            std::process::exit(1);
        }
    };

    let output_dir = match output_dir {
        Some(dir) => dir,
        None => {
            eprintln!("error: --output-dir is required");
            std::process::exit(1);
        }
    };

    match pcr_mode {
        PcrMode::Record => {
            if pin.is_some() {
                eprintln!("error: --pin is forbidden with --pcr-mode record");
                std::process::exit(1);
            }
        }
        PcrMode::Pin => match &pin {
            None => {
                eprintln!("error: --pin is required with --pcr-mode pin");
                std::process::exit(1);
            }
            Some(p) => {
                if p.len() != 64 || !p.chars().all(|c| c.is_ascii_hexdigit()) {
                    eprintln!("error: --pin must be exactly 64 hex characters");
                    std::process::exit(1);
                }
            }
        },
        _ => unreachable!(),
    }

    let content = !no_content;
    if content && model.is_none() {
        eprintln!("error: --model is required unless --no-content is specified");
        std::process::exit(1);
    }
    let credential = match (content, credential_file) {
        (false, _) => None,
        (true, None) => {
            eprintln!("error: --credential-file is required unless --no-content is specified");
            std::process::exit(1);
        }
        (true, Some(path)) => match std::fs::read_to_string(&path) {
            Ok(text) if !text.trim().is_empty() => Some(text.trim().to_owned()),
            _ => {
                eprintln!("error: --credential-file is unreadable or empty");
                std::process::exit(1);
            }
        },
    };

    let resolved_nvattest_dir = match nvattest_dir {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => match std::env::var("SPP_NVATTEST_DIR") {
            Ok(val) if !val.trim().is_empty() => PathBuf::from(val.trim()),
            _ => {
                eprintln!("error: nvattest_dir_missing");
                std::process::exit(1);
            }
        },
    };

    let ensure_status = ensure_nvattest_installed(&resolved_nvattest_dir);
    if let Some(failure) = classify_nvattest_prerequisite(ensure_status) {
        eprintln!("{}", failure.reason_code);
        std::process::exit(1);
    }

    let mut owner_nonce = [0u8; 32];
    if ring::rand::SystemRandom::new()
        .fill(&mut owner_nonce)
        .is_err()
    {
        eprintln!("confidential attestation rejected (nonce_generation_failed)");
        std::process::exit(1);
    }

    let mut pins = BTreeSet::new();
    if let Some(p) = pin {
        pins.insert(p);
    }
    let policy = qualification_policy(pcr_mode, pins);
    let composite_verifier = ProductionCompositeVerifier::new(resolved_nvattest_dir.clone());

    let request = QualificationRequest {
        host,
        port,
        policy,
        output_dir,
        model,
        credential,
        content,
        owner_nonce,
        now: SystemTime::now(),
        socket_timeout: Duration::from_secs(120),
    };

    match run_qualification(&request, &resolved_nvattest_dir, &composite_verifier) {
        Ok(success) => {
            if let Some(hex) = success.pcr_sha256 {
                println!("pcr-sha256: {hex}");
            }
            if content {
                println!("chat succeeded");
                println!("transcription succeeded");
            }
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
