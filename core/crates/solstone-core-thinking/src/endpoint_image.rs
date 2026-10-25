// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Probing whether a configured local endpoint can read image input.

use std::collections::BTreeSet;
use std::io::Cursor;
use std::path::Path;

use base64::Engine;
use image::{ImageFormat, Rgb, RgbImage};
use solstone_core_generate::{
    ClientError, ContentPart, GenerateRequest, GenerateResponse, OneShotClient,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hue {
    Orange,
    Purple,
    Pink,
    Yellow,
    Green,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageCheck {
    Read,
    NotRead,
    Unchecked { reason: CheckReason },
}

impl ImageCheck {
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::Read => serde_json::json!({"image_check": {"status": "read"}}),
            Self::NotRead => serde_json::json!({"image_check": {"status": "not_read"}}),
            Self::Unchecked { reason } => serde_json::json!({
                "image_check": {
                    "status": "unchecked",
                    "reason": reason.as_wire(),
                }
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckReason {
    Unreachable,
    Timeout,
    Busy,
    Key,
    Rejected,
    Unusable,
    Other,
    Unavailable,
}

impl CheckReason {
    pub const fn as_wire(self) -> &'static str {
        match self {
            Self::Unreachable => "unreachable",
            Self::Timeout => "timeout",
            Self::Busy => "busy",
            Self::Key => "key",
            Self::Rejected => "rejected",
            Self::Unusable => "unusable",
            Self::Other => "other",
            Self::Unavailable => "unavailable",
        }
    }
}

pub const fn rgb_for_hue(hue: Hue) -> [u8; 3] {
    match hue {
        Hue::Orange => [255, 140, 0],
        Hue::Purple => [130, 40, 160],
        Hue::Pink => [255, 130, 180],
        Hue::Yellow => [250, 220, 0],
        Hue::Green => [20, 160, 40],
    }
}

pub const fn hue_from_byte(byte: u8) -> Hue {
    match byte % 5 {
        0 => Hue::Orange,
        1 => Hue::Purple,
        2 => Hue::Pink,
        3 => Hue::Yellow,
        _ => Hue::Green,
    }
}

/// Encode a flat single-color 448x448 RGB PNG test picture.
/// The picture is entirely generated in memory from a synthetic hue so no personal
/// or journal image data ever leaves the local machine during the check.
pub fn encode_png(hue: Hue) -> Vec<u8> {
    let [r, g, b] = rgb_for_hue(hue);
    let img = RgbImage::from_pixel(448, 448, Rgb([r, g, b]));
    let mut buffer = Cursor::new(Vec::new());
    img.write_to(&mut buffer, ImageFormat::Png)
        .expect("in-memory PNG encoding succeeds");
    buffer.into_inner()
}

pub fn image_request(png_standard_base64: &str) -> GenerateRequest {
    GenerateRequest {
        id: None,
        context: "settings.local.endpoint_image".to_owned(),
        contents: vec![
            ContentPart::Image {
                mime_type: "image/png".to_owned(),
                data: png_standard_base64.to_owned(),
            },
            ContentPart::Text {
                text: "What color is this image? Answer with one word.".to_owned(),
            },
        ],
        system_instruction: None,
        temperature: 0.0,
        max_output_tokens: 512,
        timeout_s: Some(20.0),
        json_output: false,
        json_schema: None,
        attempt_index: 0,
        exclusive_admission: false,
        transport_retries: Some(0),
    }
}

pub trait ImageProbe {
    fn probe(
        &mut self,
        journal: &Path,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ClientError>;
}

impl ImageProbe for OneShotClient {
    fn probe(
        &mut self,
        journal: &Path,
        request: &GenerateRequest,
    ) -> Result<GenerateResponse, ClientError> {
        // The parent process environment is not altered: the provider override and
        // the journal root are set on the child command only. Variables the parent
        // already carries still reach the child.
        let client = self
            .clone()
            .with_env(
                solstone_core_generate_wire::overrides::PROVIDER_OVERRIDE_ENV,
                "local",
            )
            .with_env("SOLSTONE_JOURNAL", journal.to_str().unwrap_or_default());
        client.execute(request)
    }
}

#[allow(clippy::result_unit_err)]
pub fn check_endpoint_images(journal: &Path) -> Result<ImageCheck, ()> {
    let mut client = OneShotClient::sibling().map_err(|_| ())?;
    let mut byte = [0u8; 1];
    if getrandom::fill(&mut byte).is_err() {
        return Ok(ImageCheck::Unchecked {
            reason: CheckReason::Unavailable,
        });
    }
    let hue = hue_from_byte(byte[0]);
    Ok(check_with_hue(&mut client, journal, hue))
}

pub fn check_with_hue<P: ImageProbe>(probe: &mut P, journal: &Path, hue: Hue) -> ImageCheck {
    // The child learns its journal from an environment variable, which can only
    // carry valid UTF-8. A root it cannot carry must not silently fall back to
    // another journal, so the check is not made.
    if journal.to_str().is_none() {
        return ImageCheck::Unchecked {
            reason: CheckReason::Unavailable,
        };
    }
    let png_bytes = encode_png(hue);
    let base64_data = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    let request = image_request(&base64_data);
    let response = probe.probe(journal, &request);
    classify_image_reply(hue, response)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ColorFamily {
    Red,
    Orange,
    Yellow,
    Green,
    Blue,
    Purple,
    Pink,
    Brown,
    Black,
    White,
    Gray,
}

fn word_to_family(word: &str) -> Option<ColorFamily> {
    match word {
        "red" => Some(ColorFamily::Red),
        "orange" => Some(ColorFamily::Orange),
        "yellow" => Some(ColorFamily::Yellow),
        "green" => Some(ColorFamily::Green),
        "blue" => Some(ColorFamily::Blue),
        "purple" | "violet" => Some(ColorFamily::Purple),
        "pink" | "magenta" => Some(ColorFamily::Pink),
        "brown" => Some(ColorFamily::Brown),
        "black" => Some(ColorFamily::Black),
        "white" => Some(ColorFamily::White),
        "gray" | "grey" => Some(ColorFamily::Gray),
        _ => None,
    }
}

fn hue_to_family(hue: Hue) -> ColorFamily {
    match hue {
        Hue::Orange => ColorFamily::Orange,
        Hue::Purple => ColorFamily::Purple,
        Hue::Pink => ColorFamily::Pink,
        Hue::Yellow => ColorFamily::Yellow,
        Hue::Green => ColorFamily::Green,
    }
}

pub fn classify_image_reply(
    sent: Hue,
    response: Result<GenerateResponse, ClientError>,
) -> ImageCheck {
    let generate_response = match response {
        Ok(res) => res,
        Err(_) => {
            return ImageCheck::Unchecked {
                reason: CheckReason::Unavailable,
            };
        }
    };

    match generate_response {
        GenerateResponse::Generated(generated) => {
            if generated.finish_reason == "max_tokens" {
                return ImageCheck::Unchecked {
                    reason: CheckReason::Unusable,
                };
            }

            // Lowercase first so the closing tag is found in any case, then keep only
            // what follows the last one.
            let whole = generated.text.to_lowercase();
            let lower = match whole.rfind("</think>") {
                Some(index) => &whole[index + "</think>".len()..],
                None => whole.as_str(),
            };
            let mut detected_families = BTreeSet::new();
            for word in lower.split(|c: char| !c.is_alphabetic()) {
                if let Some(family) = word_to_family(word) {
                    detected_families.insert(family);
                }
            }

            let target_family = hue_to_family(sent);
            if detected_families.len() == 1 && detected_families.contains(&target_family) {
                ImageCheck::Read
            } else {
                ImageCheck::NotRead
            }
        }
        GenerateResponse::Refused(refusal) => {
            let reason = match refusal.reason_code.as_ref().map(|code| code.as_wire()) {
                Some("local_endpoint_unreachable") => CheckReason::Unreachable,
                Some("local_capacity_exhausted") => CheckReason::Timeout,
                Some("local_queue_timeout") => CheckReason::Busy,
                Some("provider_key_invalid" | "provider_key_missing") => CheckReason::Key,
                Some("provider_request_rejected" | "context_window_exceeded") => {
                    CheckReason::Rejected
                }
                Some("thinking_consumed_budget" | "provider_response_invalid") => {
                    CheckReason::Unusable
                }
                None => CheckReason::Unusable,
                _ => CheckReason::Other,
            };
            ImageCheck::Unchecked { reason }
        }
        GenerateResponse::Cancelled => ImageCheck::Unchecked {
            reason: CheckReason::Other,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_generate::{
        CapturedStream, ChildStatus, GeneratedResponse, InvalidResponseFailure, ProcessFailure,
        ProtocolFailure, ReasonCode, ReasonCodeValue, RefusalReason, RefusedResponse,
        UnexpectedChildFailure, UnknownReasonCode,
    };
    use solstone_core_local::HttpResponse;
    use std::cell::Cell;
    use std::rc::Rc;

    struct InMemoryProbe<F>(F);
    impl<F> InMemoryProbe<F>
    where
        F: FnMut(&Path, &GenerateRequest) -> Result<GenerateResponse, ClientError>,
    {
        fn new(f: F) -> Self {
            Self(f)
        }
    }

    impl<F> ImageProbe for InMemoryProbe<F>
    where
        F: FnMut(&Path, &GenerateRequest) -> Result<GenerateResponse, ClientError>,
    {
        fn probe(
            &mut self,
            journal: &Path,
            request: &GenerateRequest,
        ) -> Result<GenerateResponse, ClientError> {
            (self.0)(journal, request)
        }
    }

    fn generated_response(
        text: &str,
        finish_reason: &str,
    ) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: text.to_owned(),
            model: "test-model".to_owned(),
            usage: serde_json::Value::Null,
            finish_reason: finish_reason.to_owned(),
            thinking: None,
            schema_validation: None,
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: vec![],
        })))
    }

    fn reason_code_value(code: &str) -> ReasonCodeValue {
        match ReasonCode::new(code) {
            Ok(known) => ReasonCodeValue::Known(known),
            Err(_) => ReasonCodeValue::Unknown(UnknownReasonCode {
                received: code.to_owned(),
                canonical: ReasonCode::new("unknown").expect("unknown is a contract code"),
            }),
        }
    }

    fn refused_response(
        reason_code: Option<&str>,
        detail: &str,
    ) -> Result<GenerateResponse, ClientError> {
        Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::Unknown,
            reason_code: reason_code.map(reason_code_value),
            retryable: false,
            blocking: false,
            reset_at_ms: None,
            provider: None,
            detail: detail.to_owned(),
        }))
    }

    #[test]
    fn classification_table_driven_hues() {
        let hues = [
            (
                Hue::Orange,
                "orange",
                vec![
                    "red", "yellow", "green", "blue", "purple", "violet", "pink", "magenta",
                    "brown", "black", "white", "gray", "grey",
                ],
            ),
            (
                Hue::Purple,
                "purple",
                vec![
                    "red", "orange", "yellow", "green", "blue", "pink", "magenta", "brown",
                    "black", "white", "gray", "grey",
                ],
            ),
            (
                Hue::Pink,
                "pink",
                vec![
                    "red", "orange", "yellow", "green", "blue", "purple", "violet", "brown",
                    "black", "white", "gray", "grey",
                ],
            ),
            (
                Hue::Yellow,
                "yellow",
                vec![
                    "red", "orange", "green", "blue", "purple", "violet", "pink", "magenta",
                    "brown", "black", "white", "gray", "grey",
                ],
            ),
            (
                Hue::Green,
                "green",
                vec![
                    "red", "orange", "yellow", "blue", "purple", "violet", "pink", "magenta",
                    "brown", "black", "white", "gray", "grey",
                ],
            ),
        ];

        for (hue, own_name, others) in hues {
            assert_eq!(
                classify_image_reply(hue, generated_response(own_name, "stop")),
                ImageCheck::Read,
                "sent {hue:?} text {own_name} should be Read"
            );
            if hue == Hue::Purple {
                assert_eq!(
                    classify_image_reply(hue, generated_response("violet", "stop")),
                    ImageCheck::Read
                );
            }
            if hue == Hue::Pink {
                assert_eq!(
                    classify_image_reply(hue, generated_response("magenta", "stop")),
                    ImageCheck::Read
                );
            }
            for other in others {
                assert_eq!(
                    classify_image_reply(hue, generated_response(other, "stop")),
                    ImageCheck::NotRead,
                    "sent {hue:?} text {other} should be NotRead"
                );
            }
        }
    }

    #[test]
    fn classification_literals() {
        assert_eq!(
            classify_image_reply(
                Hue::Orange,
                generated_response("It's solid orange.", "stop")
            ),
            ImageCheck::Read
        );
        assert_eq!(
            classify_image_reply(
                Hue::Orange,
                generated_response("Orange, as colored", "stop")
            ),
            ImageCheck::Read
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("orange orange", "stop")),
            ImageCheck::Read
        );
        assert_eq!(
            classify_image_reply(Hue::Purple, generated_response("purple violet", "stop")),
            ImageCheck::Read
        );
        assert_eq!(
            classify_image_reply(Hue::Purple, generated_response("violet pink", "stop")),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("grey gray", "stop")),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(
                Hue::Orange,
                generated_response("I referred to it already", "stop")
            ),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("reddish", "stop")),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(Hue::Yellow, generated_response("yellow-orange", "stop")),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("yellow-orange", "stop")),
            ImageCheck::NotRead
        );
        for phrase in [
            "gold",
            "salmon",
            "",
            "no color here",
            "I'm sorry, I can't see an image",
        ] {
            assert_eq!(
                classify_image_reply(Hue::Orange, generated_response(phrase, "stop")),
                ImageCheck::NotRead,
                "phrase {phrase:?} should be NotRead"
            );
        }
        assert_eq!(
            classify_image_reply(
                Hue::Orange,
                generated_response("purple</think> orange", "stop")
            ),
            ImageCheck::Read
        );
        assert_eq!(
            classify_image_reply(
                Hue::Orange,
                generated_response("orange</think> purple", "stop")
            ),
            ImageCheck::NotRead
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("orange", "max_tokens")),
            ImageCheck::Unchecked {
                reason: CheckReason::Unusable
            }
        );
        assert_eq!(
            classify_image_reply(Hue::Orange, generated_response("", "stop")),
            ImageCheck::NotRead
        );
    }

    #[test]
    fn classification_refusals_and_reasons() {
        let cases = [
            ("local_endpoint_unreachable", CheckReason::Unreachable),
            ("local_capacity_exhausted", CheckReason::Timeout),
            ("local_queue_timeout", CheckReason::Busy),
            ("provider_key_invalid", CheckReason::Key),
            ("provider_key_missing", CheckReason::Key),
            ("provider_request_rejected", CheckReason::Rejected),
            ("context_window_exceeded", CheckReason::Rejected),
            ("thinking_consumed_budget", CheckReason::Unusable),
            ("provider_response_invalid", CheckReason::Unusable),
            ("context_budget_exceeded", CheckReason::Other),
            ("unrecognized_wire_code_123", CheckReason::Other),
        ];

        for (wire, expected) in cases {
            assert_eq!(
                classify_image_reply(
                    Hue::Orange,
                    refused_response(
                        Some(wire),
                        "secret-credential-value http://endpoint.example:9/v1"
                    )
                ),
                ImageCheck::Unchecked { reason: expected }
            );
        }

        assert_eq!(
            classify_image_reply(Hue::Orange, refused_response(None, "no reason code")),
            ImageCheck::Unchecked {
                reason: CheckReason::Unusable
            }
        );

        let unknown_code = ReasonCodeValue::Unknown(solstone_core_generate::UnknownReasonCode {
            received: "synthetic_unknown_reason".to_owned(),
            canonical: solstone_core_generate::ReasonCode::new("unknown").unwrap(),
        });
        let refused_unknown = Ok(GenerateResponse::Refused(RefusedResponse {
            id: None,
            reason: RefusalReason::Unknown,
            reason_code: Some(unknown_code),
            retryable: false,
            blocking: false,
            reset_at_ms: None,
            provider: None,
            detail: "detail".to_owned(),
        }));
        assert_eq!(
            classify_image_reply(Hue::Orange, refused_unknown),
            ImageCheck::Unchecked {
                reason: CheckReason::Other
            }
        );
    }

    #[test]
    fn classification_client_errors_and_secret_hygiene() {
        let secret = "secret-credential-value";
        let url = "http://endpoint.example:9/v1";

        let errors = vec![
            ClientError::Resolve(format!("{secret} {url}")),
            ClientError::Io {
                primary: format!("{secret} {url}"),
                cleanup: None,
            },
            ClientError::Protocol(Box::new(ProtocolFailure {
                error: solstone_core_generate::ProtocolError {
                    id: None,
                    reason: "local_endpoint_unreachable".to_owned(),
                    detail: format!("{secret} {url}"),
                },
                status: ChildStatus {
                    exit_code: Some(1),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
            ClientError::UnexpectedChild(Box::new(UnexpectedChildFailure {
                status: ChildStatus {
                    exit_code: Some(1),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
            ClientError::InvalidResponse(Box::new(InvalidResponseFailure {
                detail: format!("{secret} {url}"),
                status: ChildStatus {
                    exit_code: Some(1),
                    signal: None,
                },
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
            ClientError::ProcessIo(Box::new(ProcessFailure {
                primary: format!("{secret} {url}"),
                cleanup: Vec::new(),
                status: Some(ChildStatus {
                    exit_code: Some(1),
                    signal: None,
                }),
                stdout: CapturedStream::empty(),
                stderr: CapturedStream::empty(),
                stdin_closed_early: false,
            })),
            ClientError::Decode(format!("{secret} {url}")),
        ];

        for err in errors {
            let classified = classify_image_reply(Hue::Orange, Err(err));
            assert_eq!(
                classified,
                ImageCheck::Unchecked {
                    reason: CheckReason::Unavailable
                }
            );
            let json_str = serde_json::to_string(&classified.to_json()).unwrap();
            assert!(!json_str.contains(secret));
            assert!(!json_str.contains(url));
        }

        let refused = refused_response(Some("provider_key_invalid"), &format!("{secret} {url}"));
        let classified_refused = classify_image_reply(Hue::Orange, refused);
        let refused_json = serde_json::to_string(&classified_refused.to_json()).unwrap();
        assert!(!refused_json.contains(secret));
        assert!(!refused_json.contains(url));
    }

    #[test]
    fn png_encoding_properties() {
        for hue in [Hue::Orange, Hue::Purple, Hue::Pink, Hue::Yellow, Hue::Green] {
            let png_bytes = encode_png(hue);
            let img = image::load_from_memory(&png_bytes).expect("PNG decodes");
            assert_eq!(img.width(), 448);
            assert_eq!(img.height(), 448);
            assert_eq!(img.color(), image::ColorType::Rgb8);
            let rgb = img.to_rgb8();
            let expected_pixel = Rgb(rgb_for_hue(hue));
            for pixel in rgb.pixels() {
                assert_eq!(*pixel, expected_pixel);
            }
        }

        assert_eq!(hue_from_byte(0), Hue::Orange);
        assert_eq!(hue_from_byte(1), Hue::Purple);
        assert_eq!(hue_from_byte(2), Hue::Pink);
        assert_eq!(hue_from_byte(3), Hue::Yellow);
        assert_eq!(hue_from_byte(4), Hue::Green);

        for b in 0..=255u8 {
            let hue = hue_from_byte(b);
            let rgb = rgb_for_hue(hue);
            assert!(
                rgb == rgb_for_hue(Hue::Orange)
                    || rgb == rgb_for_hue(Hue::Purple)
                    || rgb == rgb_for_hue(Hue::Pink)
                    || rgb == rgb_for_hue(Hue::Yellow)
                    || rgb == rgb_for_hue(Hue::Green)
            );
        }
    }

    #[test]
    fn getrandom_production_source_draws() {
        let mut distinct_hues = BTreeSet::new();
        for _ in 0..200 {
            let mut byte = [0u8; 1];
            getrandom::fill(&mut byte).expect("getrandom succeeds");
            let hue = hue_from_byte(byte[0]);
            let rgb = rgb_for_hue(hue);
            assert!(
                rgb == rgb_for_hue(Hue::Orange)
                    || rgb == rgb_for_hue(Hue::Purple)
                    || rgb == rgb_for_hue(Hue::Pink)
                    || rgb == rgb_for_hue(Hue::Yellow)
                    || rgb == rgb_for_hue(Hue::Green)
            );
            distinct_hues.insert(rgb);
        }
        assert!(
            distinct_hues.len() >= 3,
            "200 draws must yield at least 3 distinct hues, got {}",
            distinct_hues.len()
        );
    }

    /// Name the color a flat test picture shows, from literal values and not from
    /// the production table, so a picture painted with the wrong hue is caught.
    fn name_of_painted_pixel(pixel: [u8; 3]) -> &'static str {
        match pixel {
            [255, 140, 0] => "orange",
            [130, 40, 160] => "purple",
            [255, 130, 180] => "pink",
            [250, 220, 0] => "yellow",
            [20, 160, 40] => "green",
            _ => "unrecognized",
        }
    }

    #[test]
    fn the_color_painted_is_the_color_judged() {
        for hue in [Hue::Orange, Hue::Purple, Hue::Pink, Hue::Yellow, Hue::Green] {
            let mut probe = InMemoryProbe::new(|_, request| {
                let data = request
                    .contents
                    .iter()
                    .find_map(|part| match part {
                        ContentPart::Image { data, .. } => Some(data.clone()),
                        _ => None,
                    })
                    .expect("the request carries an image");
                let png = base64::engine::general_purpose::STANDARD
                    .decode(data)
                    .expect("standard base64");
                let img = image::load_from_memory(&png)
                    .expect("PNG decodes")
                    .to_rgb8();
                let name = name_of_painted_pixel(img.get_pixel(0, 0).0);
                generated_response(&format!("It's solid {name}."), "stop")
            });
            assert_eq!(
                check_with_hue(&mut probe, Path::new("/tmp/journal"), hue),
                ImageCheck::Read,
                "a model that names the color actually painted must read for {hue:?}"
            );
        }
    }

    #[test]
    fn closing_think_tag_is_found_in_any_case_and_the_last_one_wins() {
        for (text, hue, want) in [
            (
                "<THINK>maybe orange</THINK> purple",
                Hue::Purple,
                ImageCheck::Read,
            ),
            (
                "</think>orange</think>purple",
                Hue::Purple,
                ImageCheck::Read,
            ),
            ("orange</think>", Hue::Orange, ImageCheck::NotRead),
        ] {
            assert_eq!(
                classify_image_reply(hue, generated_response(text, "stop")),
                want,
                "{text}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_journal_root_the_child_cannot_carry_is_not_checked() {
        use std::os::unix::ffi::OsStrExt;
        let root = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/journal-\xff"));
        let mut probe = InMemoryProbe::new(|_, _| panic!("the child must not be asked"));
        assert_eq!(
            check_with_hue(&mut probe, root, Hue::Orange),
            ImageCheck::Unchecked {
                reason: CheckReason::Unavailable
            }
        );
    }

    #[test]
    fn check_with_hue_end_to_end_in_memory() {
        let dummy_journal = Path::new("/var/tmp/dummy-journal");

        let mut probe_always_orange =
            InMemoryProbe::new(|_journal: &Path, _req: &GenerateRequest| {
                generated_response("orange", "stop")
            });
        assert_eq!(
            check_with_hue(&mut probe_always_orange, dummy_journal, Hue::Orange),
            ImageCheck::Read
        );
        assert_eq!(
            check_with_hue(&mut probe_always_orange, dummy_journal, Hue::Purple),
            ImageCheck::NotRead
        );

        let cell = Rc::new(Cell::new(Hue::Orange));
        let cell_clone = cell.clone();
        let mut probe_matching =
            InMemoryProbe::new(move |_journal: &Path, _req: &GenerateRequest| {
                let name = match cell_clone.get() {
                    Hue::Orange => "orange",
                    Hue::Purple => "purple",
                    Hue::Pink => "pink",
                    Hue::Yellow => "yellow",
                    Hue::Green => "green",
                };
                generated_response(name, "stop")
            });

        cell.set(Hue::Orange);
        assert_eq!(
            check_with_hue(&mut probe_matching, dummy_journal, Hue::Orange),
            ImageCheck::Read
        );
        cell.set(Hue::Purple);
        assert_eq!(
            check_with_hue(&mut probe_matching, dummy_journal, Hue::Purple),
            ImageCheck::Read
        );

        let mut probe_blue = InMemoryProbe::new(|_journal: &Path, _req: &GenerateRequest| {
            generated_response("blue", "stop")
        });
        assert_eq!(
            check_with_hue(&mut probe_blue, dummy_journal, Hue::Orange),
            ImageCheck::NotRead
        );
    }

    struct RecordingTransport {
        posted_bodies: Vec<serde_json::Value>,
    }

    impl solstone_core_generate_wire::EndpointTransport for RecordingTransport {
        fn get(
            &mut self,
            _base_url: &str,
            _path: &str,
            _credential: Option<&str>,
            _timeout: std::time::Duration,
        ) -> Result<HttpResponse, solstone_core_generate_wire::EndpointTransportError> {
            Ok(HttpResponse {
                status: 200,
                body: "{}".to_owned(),
            })
        }

        fn post_json(
            &mut self,
            _base_url: &str,
            _path: &str,
            body: &serde_json::Value,
            _credential: Option<&str>,
            _timeout: std::time::Duration,
        ) -> Result<HttpResponse, solstone_core_generate_wire::EndpointTransportError> {
            self.posted_bodies.push(body.clone());
            Ok(HttpResponse {
                status: 200,
                body: r#"{"choices":[{"message":{"content":"orange"},"finish_reason":"stop"}]}"#
                    .to_owned(),
            })
        }
    }

    #[test]
    fn request_body_transport_test() {
        use solstone_core_generate_wire::{EndpointRuntime, endpoint_generate_with};
        use solstone_core_local::ByoEndpoint;

        let temp_dir = tempfile::tempdir().expect("tempdir creates");
        let png_bytes = encode_png(Hue::Orange);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
        let request = image_request(&b64);

        let mut config = serde_json::Map::new();
        config.insert(
            "providers".to_owned(),
            serde_json::json!({
                "byo_thinking_budget": 8192,
                "local": {
                    "served_context_window": 16384,
                }
            }),
        );

        let endpoint = ByoEndpoint {
            base_url: "http://127.0.0.1:8080".to_owned(),
            served_model_id: "vision-model".to_owned(),
            credential: None,
            parallel_slots: Some(2),
            is_confidential: false,
            is_bundled: false,
        };

        let runtime = EndpointRuntime::default();
        let mut transport = RecordingTransport {
            posted_bodies: vec![],
        };

        let _ = endpoint_generate_with(
            &request,
            temp_dir.path(),
            &endpoint,
            &config,
            &runtime,
            &mut transport,
            std::time::Instant::now(),
        );

        assert_eq!(transport.posted_bodies.len(), 1);
        let body = &transport.posted_bodies[0];
        let messages = body
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let content_parts = messages[0]
            .get("content")
            .and_then(serde_json::Value::as_array)
            .unwrap();

        let image_part = &content_parts[0];
        let image_url = image_part["image_url"]["url"].as_str().unwrap();
        assert!(image_url.starts_with("data:image/png;base64,"));
        let url_b64 = &image_url["data:image/png;base64,".len()..];
        assert_eq!(url_b64, b64);
        assert!(!url_b64.starts_with("data:"));

        let text_part = &content_parts[1];
        assert_eq!(
            text_part["text"].as_str().unwrap(),
            "What color is this image? Answer with one word."
        );

        assert_eq!(body["temperature"].as_f64().unwrap(), 0.0);
        assert!(body.get("response_format").is_none());
        assert_eq!(body["max_tokens"].as_u64().unwrap(), 8704);

        // Second call with byo_thinking_budget 0 -> max_tokens is 512
        config.insert(
            "providers".to_owned(),
            serde_json::json!({
                "byo_thinking_budget": 0,
                "local": {
                    "served_context_window": 16384,
                }
            }),
        );
        let mut transport2 = RecordingTransport {
            posted_bodies: vec![],
        };
        let _ = endpoint_generate_with(
            &request,
            temp_dir.path(),
            &endpoint,
            &config,
            &runtime,
            &mut transport2,
            std::time::Instant::now(),
        );

        assert_eq!(transport2.posted_bodies.len(), 1);
        let body2 = &transport2.posted_bodies[0];
        let messages2 = body2
            .get("messages")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let content_parts2 = messages2[0]
            .get("content")
            .and_then(serde_json::Value::as_array)
            .unwrap();
        let image_url2 = content_parts2[0]["image_url"]["url"].as_str().unwrap();
        assert_eq!(image_url2, image_url);
        assert_eq!(
            content_parts2[1]["text"].as_str().unwrap(),
            "What color is this image? Answer with one word."
        );
        assert_eq!(body2["max_tokens"].as_u64().unwrap(), 512);
    }
}
