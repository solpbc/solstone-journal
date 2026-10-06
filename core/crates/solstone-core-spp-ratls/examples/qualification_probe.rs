// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Manual qualification tool, invoked by hand, not part of the journal, not a CI gate, the host comes from the caller.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use ring::rand::SecureRandom;
use solstone_core_spp_attest::PcrMode;
use solstone_core_spp_attest::nvgpu::{GpuProfile, GpuProfiles, ManifestSet, StatusMode};
use solstone_core_spp_ratls::qualification::{
    QualificationRequest, qualification_policy, run_qualification,
};
use solstone_core_spp_ratls::{
    ProductionCompositeVerifier, classify_nvattest_prerequisite, ensure_nvattest_installed,
};

struct RecordingVerifier {
    inner: ProductionCompositeVerifier,
    output: PathBuf,
}

impl solstone_core_spp_ratls::CompositeVerifier for RecordingVerifier {
    fn verify(
        &self,
        bundle: solstone_core_spp_attest::CpuBundle<'_>,
        input: solstone_core_spp_ratls::CompositeVerificationInput<'_>,
    ) -> Result<
        solstone_core_spp_ratls::CompositeVerdict,
        solstone_core_spp_ratls::CompositeVerificationError,
    > {
        let failed = || solstone_core_spp_ratls::CompositeVerificationError {
            reason_code: "qualification_capture_failed",
        };
        std::fs::create_dir_all(&self.output).map_err(|_| failed())?;
        // Explicitly unverified captures remain available when CPU admission
        // refuses an unexpected platform; only a returned verdict authenticates them.
        std::fs::write(self.output.join("unverified-quote.pcrs"), bundle.quote_pcrs)
            .map_err(|_| failed())?;
        std::fs::write(self.output.join("gpu-envelope.tlv"), input.envelope_tlv)
            .map_err(|_| failed())?;
        if let Some(proofs) = input.status_proofs {
            std::fs::write(self.output.join("status-proofs.der"), proofs).map_err(|_| failed())?;
        } else {
            match std::fs::remove_file(self.output.join("status-proofs.der")) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(failed()),
            }
        }
        let verdict = self.inner.verify(bundle, input);
        std::fs::write(
            self.output.join("composite-verdict.txt"),
            format!("{verdict:#?}\n"),
        )
        .map_err(|_| failed())?;
        verdict
    }
}

const USAGE: &str = "\
Usage: qualification_probe --host <HOST> [--port <PORT>] --pcr-mode <record|pin> [--pin <HEX>] --output-dir <DIR> [--model <MODEL>] [--credential-file <PATH>] [--nvattest-dir <DIR>] [--no-content] [--offline-status-profile]

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
  --request-json <PATH>   Send one custom chat JSON body after admission; save JSON status/body or bounded streaming HTTP wire bytes (no fixed chat/audio probes)\n  --no-content            Captures evidence and does not send chat or transcription
  --offline-status-profile Stages the packaged 595.71.05 manifests and OfflineSignedAge for --pin only; requires --pcr-mode pin, never changes production admission
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
    let mut request_json: Option<PathBuf> = None;
    let mut no_content = false;
    let mut offline_status_profile = false;

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
            "--request-json" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("error: missing value for --request-json");
                    std::process::exit(1);
                }
                request_json = Some(PathBuf::from(&args[i]));
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
            "--offline-status-profile" => {
                offline_status_profile = true;
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

    if offline_status_profile && pcr_mode != PcrMode::Pin {
        eprintln!("error: --offline-status-profile requires --pcr-mode pin and --pin");
        std::process::exit(1);
    }

    if request_json.is_some() && no_content {
        eprintln!("error: --request-json cannot be used with --no-content");
        std::process::exit(1);
    }
    let request_body = request_json.map(|path| {
        let bytes = std::fs::read(path).expect("request JSON must be readable");
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).expect("request must be JSON");
        assert!(value.is_object(), "request must be a JSON object");
        bytes
    });
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

    let staged_profile = if offline_status_profile {
        pin.as_ref().map(|pin| {
            GpuProfile::new(
                pin.clone(),
                ManifestSet::QUALIFIED_595_71_05,
                StatusMode::OfflineSignedAge,
            )
        })
    } else {
        None
    };
    let mut pins = BTreeSet::new();
    if let Some(p) = pin {
        pins.insert(p);
    }
    let policy = qualification_policy(pcr_mode, pins);
    let inner = match staged_profile {
        Some(profile) => ProductionCompositeVerifier::with_profiles(
            resolved_nvattest_dir.clone(),
            GpuProfiles::from_profiles(vec![profile]),
        ),
        None => ProductionCompositeVerifier::new(resolved_nvattest_dir.clone()),
    };
    let composite_verifier = RecordingVerifier {
        inner,
        output: output_dir.clone(),
    };

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

    if let Some(body) = request_body {
        if let Err(error) =
            run_custom_request(&request, &resolved_nvattest_dir, &composite_verifier, &body)
        {
            eprintln!("custom attested request failed: {error}");
            std::process::exit(1);
        }
        return;
    }
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

// The custom body is a manual qualification input. It never changes production policy.
fn run_custom_request(
    request: &QualificationRequest,
    nvattest_dir: &std::path::Path,
    verifier: &dyn solstone_core_spp_ratls::CompositeVerifier,
    body: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    use solstone_core_spp_ratls::{
        AdmissionClock, AttestedIo, RatlsEndpoint, SystemAdmissionClock,
        establish_attested_channel_with_clock, send_json_request,
    };
    use std::net::ToSocketAddrs;
    let addresses = (request.host.as_str(), request.port)
        .to_socket_addrs()?
        .collect::<BTreeSet<_>>();
    if addresses.len() != 1 {
        return Err("qualification target must resolve to one address".into());
    }
    let clock = SystemAdmissionClock;
    let mut channel = establish_attested_channel_with_clock(
        &RatlsEndpoint::new(&request.host, request.port),
        &request.owner_nonce,
        nvattest_dir,
        request.now,
        None,
        Some(&request.policy),
        None,
        verifier,
        Duration::from_secs(900),
        0,
        &clock,
    )?;
    let evidence = &channel.verified.evidence;
    for (name, bytes) in [
        ("akpub.pem", evidence.ak_public_key_pem.as_slice()),
        ("quote.msg", evidence.quote_message.as_slice()),
        ("quote.sig", evidence.quote_signature.as_slice()),
        ("quote.pcrs", evidence.quote_pcrs.as_slice()),
        ("hcl_report.bin", evidence.hcl_report.as_slice()),
    ] {
        std::fs::write(request.output_dir.join(name), bytes)?;
    }
    let nonce: String = evidence
        .owner_nonce
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    std::fs::write(request.output_dir.join("nonce.hex"), nonce)?;
    if !channel.status_permits_new_request(clock.now_system(), clock.now_monotonic()) {
        return Err("status no longer permits a new request".into());
    }
    channel.set_io_timeout(Some(Duration::from_secs(900)))?;
    if serde_json::from_slice::<serde_json::Value>(body)?
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        // The JSON response API deliberately refuses transfer encoding. The
        // manual streaming probe records bounded wire bytes for the caller's
        // HTTP/SSE decoder, after the same composite and admission checks.
        let wire = capture_streaming_response(&mut channel, request, body)?;
        std::fs::write(request.output_dir.join("http-wire-response.bin"), &wire)?;
        println!("streaming HTTP wire bytes: {}", wire.len());
        return Ok(());
    }
    let response = send_json_request(
        &mut channel,
        &format!("{}:{}", request.host, request.port),
        "/v1/chat/completions",
        request.credential.as_deref(),
        body,
        false,
    )?;
    std::fs::write(
        request.output_dir.join("http-response.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"status":response.status,"body":String::from_utf8(response.body)?}),
        )?,
    )?;
    println!("HTTP status: {}", response.status);
    Ok(())
}

fn capture_streaming_response(
    channel: &mut dyn solstone_core_spp_ratls::AttestedIo,
    request: &QualificationRequest,
    body: &[u8],
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    use std::io::Read;
    const MAX_WIRE_BYTES: u64 = 2 * 1024 * 1024;
    let authority = format!("{}:{}", request.host, request.port);
    let credential = request.credential.as_deref().unwrap_or_default();
    if authority.contains(['\r', '\n']) || credential.contains(['\r', '\n']) {
        return Err("qualification HTTP header contains a line break".into());
    }
    let head = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nAuthorization: Bearer {credential}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    channel.write_all(head.as_bytes())?;
    channel.write_all(body)?;
    channel.flush()?;
    let mut wire = Vec::new();
    channel.take(MAX_WIRE_BYTES + 1).read_to_end(&mut wire)?;
    if wire.len() as u64 > MAX_WIRE_BYTES {
        return Err("qualification streaming response exceeds capture limit".into());
    }
    Ok(wire)
}
