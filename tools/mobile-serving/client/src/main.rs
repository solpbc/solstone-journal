// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
//! Synthetic physical qualification driver; one JSON command and reply per line.
use rustls::{
    ClientConfig,
    pki_types::{PrivateKeyDer, ServerName, pem::PemObject},
};
use serde_json::{Value, json};
use spl_core::{
    frame::{FLAG_CLOSE, FLAG_DATA, FLAG_OPEN, Frame, FrameDecoder, FrameDialer},
    mux::ResponseAssembler,
};
use spl_transport::{credential::Credential, tls};
use std::{
    io::{self, BufRead, Write},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, client::TlsStream};

type Error = Box<dyn std::error::Error + Send + Sync>;
type Stream = TlsStream<TcpStream>;
fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("missing {key}").into())
}
fn credentials(v: &Value) -> Result<Credential, Error> {
    Ok(serde_json::from_slice(&std::fs::read(field(
        v,
        "credential",
    )?)?)?)
}
fn config(v: &Value) -> Result<Arc<ClientConfig>, Error> {
    let c = credentials(v)?;
    let mode = v["auth"].as_str().unwrap_or("paired");
    if mode == "missing" {
        return Ok(Arc::new(tls::pairing_config(&c.ca_fp_prefix)?));
    }
    let (cert, key) = if mode == "invalid" {
        let signed = rcgen::generate_simple_self_signed(vec!["synthetic.invalid".into()])?;
        (signed.cert.pem(), signed.key_pair.serialize_pem())
    } else if mode == "paired" {
        (c.client_cert_pem, c.client_key_pem)
    } else {
        return Err("unknown auth mode".into());
    };
    Ok(Arc::new(tls::mtls_config(
        &c.ca_fp_prefix,
        tls::parse_certs(&cert)?,
        PrivateKeyDer::from_pem_slice(key.as_bytes())?,
    )?))
}
async fn connect(v: &Value) -> Result<Stream, Error> {
    if !matches!(v["carrier"].as_str(), None | Some("direct")) {
        return Err("held and fault carriers require direct mode".into());
    }
    let port = u16::try_from(v["port"].as_u64().ok_or("missing port")?)?;
    let tcp = TcpStream::connect((field(v, "host")?, port)).await?;
    tcp.set_nodelay(true)?;
    Ok(TlsConnector::from(config(v)?)
        .connect(ServerName::try_from("spl.local")?, tcp)
        .await?)
}
fn response(r: spl_core::http::HttpResponse, v: &Value) -> Value {
    // Exact-original checks need the received bytes, not a lossy text view.
    if v["body_encoding"].as_str() == Some("hex") {
        let hex: String = r.body.iter().map(|b| format!("{b:02x}")).collect();
        return json!({"status":r.status,"body_hex":hex});
    }
    json!({"status":r.status,"body":String::from_utf8_lossy(&r.body)})
}
// Fault and held-carrier controls use the same SPL framing/assembly library.
// Bound small synthetic requests fit in the initial stream window.
async fn exchange(stream: &mut Stream, id: u32, v: &Value) -> Result<Value, Error> {
    let body = body(v)?;
    if body.len() > 512 * 1024 {
        return Err("fault-injection body exceeds 512 KiB".into());
    }
    let headers = headers(v);
    let mut bytes = spl_core::http::build_request_head(
        field(v, "method")?,
        field(v, "uri")?,
        &headers,
        body.len(),
    );
    let loss = v["loss"].as_str().unwrap_or("");
    bytes.extend_from_slice(if loss == "before" {
        &body[..body.len() / 2]
    } else {
        &body
    });
    for (index, chunk) in bytes.chunks(32 * 1024).enumerate() {
        let flags = FLAG_DATA | if index == 0 { FLAG_OPEN } else { 0 };
        stream
            .write_all(&Frame::new(id, flags, chunk.to_vec()).encode()?)
            .await?;
    }
    if loss == "before" {
        stream.flush().await?;
        return Ok(
            json!({"loss":"before","sent_body_bytes":body.len()/2,"complete_body_bytes":body.len()}),
        );
    }
    stream
        .write_all(&Frame::new(id, FLAG_CLOSE, vec![]).encode()?)
        .await?;
    stream.flush().await?;
    let mut assembler = ResponseAssembler::with_cap(id, 4 * 1024 * 1024);
    let mut decoder = FrameDecoder::new();
    let mut buffer = [0u8; 65536];
    while !assembler.is_closed() {
        let n = stream.read(&mut buffer).await?;
        if n == 0 {
            return Err("carrier closed before response".into());
        }
        if loss == "after" {
            decoder.feed(&buffer[..n]);
            for frame in decoder.drain()? {
                if frame.stream_id == id && frame.flags & FLAG_DATA != 0 {
                    return Ok(
                        json!({"loss":"after","response_data_observed":true,"response_discarded":true}),
                    );
                }
                if let Some(pong) = frame.control_pong() {
                    stream.write_all(&pong.encode()?).await?;
                }
            }
            continue;
        }
        let output = assembler.feed(&buffer[..n])?;
        for frame in output.pongs.into_iter().chain(output.emit_frames) {
            stream.write_all(&frame).await?;
        }
        if let Some(error) = output.terminal_error {
            return Err(error.into());
        }
    }
    Ok(response(assembler.into_response()?, v))
}
fn body(v: &Value) -> Result<Vec<u8>, Error> {
    if let Some(file) = v["body_file"].as_str() {
        Ok(std::fs::read(file)?)
    } else {
        Ok(v["body"].as_str().unwrap_or("").as_bytes().to_vec())
    }
}
fn headers(v: &Value) -> Vec<(String, String)> {
    let mut headers = vec![(
        "content-type".into(),
        v["content_type"]
            .as_str()
            .unwrap_or("application/json")
            .into(),
    )];
    if let Some(version) = v["protocol_version"].as_str() {
        headers.push(("x-solstone-protocol-version".into(), version.into()));
    }
    headers
}
async fn command(
    v: &Value,
    held: &mut Option<Stream>,
    dialer: &mut FrameDialer,
) -> Result<Value, Error> {
    match field(v, "op")? {
        "pair" => {
            let link = field(v, "link")?;
            // Require the chosen carrier before dialing. Relay qualification
            // cannot silently use direct endpoints from a pairing link.
            match (
                v["carrier"].as_str().unwrap_or("direct"),
                spl_core::pairlink::parse(link)?,
            ) {
                ("direct", spl_core::pairlink::ParsedPairLink::Direct(_))
                | ("relay", spl_core::pairlink::ParsedPairLink::Relay(_)) => {}
                _ => return Err("pair link does not match selected carrier".into()),
            }
            let credential = spl_transport::pairing::pair_from_link(
                link,
                field(v, "label")?,
                &Default::default(),
            )
            .await?;
            let path = Path::new(field(v, "credential")?);
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            options
                .open(path)?
                .write_all(&serde_json::to_vec(&credential)?)?;
            Ok(
                json!({"paired":true,"instance_id":credential.instance_id,"endpoints":credential.endpoints}),
            )
        }
        "request" => {
            if v["carrier"].as_str() == Some("relay") {
                let c = credentials(v)?;
                let result = spl_transport::relay::request_once_relay(
                    config(v)?,
                    c.relay_origin.as_deref().ok_or("missing relay origin")?,
                    &c.instance_id,
                    c.device_token
                        .as_deref()
                        .ok_or("missing relay device token")?,
                    field(v, "method")?,
                    field(v, "uri")?,
                    &headers(v),
                    &body(v)?,
                )
                .await?;
                return Ok(response(result, v));
            }
            if !matches!(v["carrier"].as_str(), None | Some("direct")) {
                return Err("unknown carrier".into());
            }
            let port = u16::try_from(v["port"].as_u64().ok_or("missing port")?)?;
            let result = spl_transport::connection::request_once(
                config(v)?,
                field(v, "host")?,
                port,
                field(v, "method")?,
                field(v, "uri")?,
                &headers(v),
                &body(v)?,
            )
            .await?;
            Ok(response(result, v))
        }
        "connect" => {
            *held = Some(connect(v).await?);
            *dialer = FrameDialer::default();
            Ok(json!({"connected":true}))
        }
        "held" => {
            if !matches!(v["carrier"].as_str(), None | Some("direct")) {
                return Err("held and fault carriers require direct mode".into());
            }
            exchange(
                held.as_mut().ok_or("no held carrier")?,
                dialer.allocate(),
                v,
            )
            .await
        }
        "disconnect" => {
            held.take();
            Ok(json!({"disconnected":true}))
        }
        "closed" => {
            let stream = held.as_mut().ok_or("no held carrier")?;
            let mut decoder = FrameDecoder::new();
            let mut buffer = [0u8; 65536];
            loop {
                match stream.read(&mut buffer).await {
                    Ok(0) => return Ok(json!({"closed":true,"reason":"eof"})),
                    Err(error) => return Ok(json!({"closed":true,"reason":error.to_string()})),
                    Ok(n) => {
                        decoder.feed(&buffer[..n]);
                        for frame in decoder.drain()? {
                            if let Some(pong) = frame.control_pong() {
                                stream.write_all(&pong.encode()?).await?;
                            }
                        }
                    }
                }
            }
        }
        "loss" => {
            let mut stream = connect(v).await?;
            exchange(&mut stream, 1, v).await
        }
        _ => Err("unknown operation".into()),
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let mut held = None;
    let mut dialer = FrameDialer::default();
    for line in io::stdin().lock().lines() {
        let result = async {
            let v: Value = serde_json::from_str(&line?)?;
            command(&v, &mut held, &mut dialer).await
        };
        let reply = match tokio::time::timeout(Duration::from_secs(30), result).await {
            Ok(Ok(value)) => json!({"ok":true,"value":value}),
            Ok(Err(error)) => json!({"ok":false,"error":error.to_string()}),
            Err(_) => json!({"ok":false,"error":"qualification request deadline"}),
        };
        println!("{reply}");
        io::stdout().flush().unwrap();
    }
}
