// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing Thinking copy, represented without a JSON dependency.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

#[derive(Clone, Copy, Serialize)]
pub struct Lane {
    pub id: &'static str,
    pub label: &'static str,
    pub sub: &'static str,
    pub description: &'static str,
}

#[derive(Clone, Copy, Serialize)]
pub struct ConfidentialLaneDetail {
    pub heading: &'static str,
    pub sub: &'static str,
    pub mechanism: &'static str,
    pub egress: &'static str,
    pub claims: &'static str,
    pub attestation: &'static str,
    pub early_access: &'static str,
}

pub const LANES: [Lane; 3] = [
    Lane {
        id: "local",
        label: "Local",
        sub: "on your device",
        description: "a model runs right on this computer. nothing leaves this machine to be processed.",
    },
    Lane {
        id: "confidential",
        label: "Confidential processing",
        sub: "operated by sol pbc",
        description: "sol pbc runs the model on confidential GPUs.",
    },
    Lane {
        id: "byo",
        label: "your own AI engine",
        sub: "your key, or your own endpoint",
        description: "bring a provider key (Claude, Gemini, or GPT) or point processing at your own endpoint. the key stays in your journal; sol pbc is never in the path.",
    },
];

pub const CONFIDENTIAL_LANE_DETAIL: ConfidentialLaneDetail = ConfidentialLaneDetail {
    heading: "confidential processing",
    sub: "operated by sol pbc",
    mechanism: "sol pbc runs the model itself on confidential GPUs in Microsoft Azure. the hardware boundary keeps the cloud host excluded from what's processed — no third-party AI provider is in the path.",
    egress: "when it's on, the thinking leaves your device — text, images, and (with the audio switch on, its default) your audio for transcription. your journal itself never leaves.",
    claims: "no content is retained · no human reviews it · nothing is used to train",
    attestation: "your journal must verify the service before anything is sent — if it can't verify, it doesn't send.",
    early_access: "confidential processing is coming — scouts get it first.",
};

#[derive(Clone, Copy)]
pub enum CopyValue {
    String(&'static str),
    Array(&'static [CopyValue]),
    Object(&'static [(&'static str, CopyValue)]),
    Lanes,
    ConfidentialLaneDetail,
}

impl Serialize for CopyValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::String(value) => serializer.serialize_str(value),
            Self::Array(values) => values.serialize(serializer),
            Self::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in *entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Self::Lanes => LANES.serialize(serializer),
            Self::ConfidentialLaneDetail => CONFIDENTIAL_LANE_DETAIL.serialize(serializer),
        }
    }
}

pub const THINKING_COPY_PAYLOAD: CopyValue = CopyValue::Object(&[
    ("heading", CopyValue::String("thinking")),
    (
        "active_lane_labels",
        CopyValue::Object(&[
            ("none", CopyValue::String("not thinking yet")),
            ("local", CopyValue::String("local")),
            ("confidential", CopyValue::String("confidential processing")),
            ("byo", CopyValue::String("your own AI engine")),
        ]),
    ),
    ("lanes", CopyValue::Lanes),
    (
        "provider_labels",
        CopyValue::Object(&[
            ("anthropic", CopyValue::String("Claude")),
            ("google", CopyValue::String("Gemini")),
            ("openai", CopyValue::String("GPT")),
            ("local", CopyValue::String("Local")),
        ]),
    ),
    (
        "key_labels",
        CopyValue::Object(&[
            ("ANTHROPIC_API_KEY", CopyValue::String("Claude key")),
            ("GOOGLE_API_KEY", CopyValue::String("Gemini key")),
            ("OPENAI_API_KEY", CopyValue::String("GPT key")),
        ]),
    ),
    (
        "state_labels",
        CopyValue::Object(&[
            ("active", CopyValue::String("active")),
            ("available", CopyValue::String("available")),
            ("unavailable", CopyValue::String("not ready")),
            ("loading", CopyValue::String("loading...")),
            ("saved", CopyValue::String("saved")),
            ("validating", CopyValue::String("validating...")),
            ("failed", CopyValue::String("couldn't finish")),
        ]),
    ),
    (
        "action_labels",
        CopyValue::Object(&[
            ("switch", CopyValue::String("Use This Lane")),
            ("save_key", CopyValue::String("Save Key")),
            ("clear_key", CopyValue::String("Clear Key")),
            ("install", CopyValue::String("Install")),
            ("refresh", CopyValue::String("Refresh")),
            ("check", CopyValue::String("Check")),
        ]),
    ),
    (
        "confidential",
        CopyValue::Object(&[
            ("lane_detail", CopyValue::ConfidentialLaneDetail),
            ("more_label", CopyValue::String("how it works →")),
            (
                "setup",
                CopyValue::Object(&[(
                    "trust_beats",
                    CopyValue::Object(&[
                        ("heading", CopyValue::String("confidential processing")),
                        ("sub", CopyValue::String("operated by sol pbc")),
                        (
                            "egress_audio_on",
                            CopyValue::String(
                                "what leaves your device: the text and images a model needs to work through, and your audio for transcription. your journal itself never leaves.",
                            ),
                        ),
                        (
                            "egress_audio_off",
                            CopyValue::String(
                                "what leaves your device: the text and images a model needs to work through. your audio stays on your device; speech becomes text there.",
                            ),
                        ),
                        (
                            "claims",
                            CopyValue::String(
                                "no content is retained · no human reviews it · nothing is used to train",
                            ),
                        ),
                        (
                            "attestation",
                            CopyValue::String(
                                "your journal must verify the service before anything is sent — if it can't verify, it doesn't send.",
                            ),
                        ),
                        (
                            "substrate",
                            CopyValue::String(
                                "sol pbc runs the model itself on confidential GPUs in Microsoft Azure. the hardware boundary keeps the cloud host excluded from what's processed — no third-party AI provider is in the path.",
                            ),
                        ),
                    ]),
                )]),
            ),
            (
                "audio",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("transcribe audio on the service"),
                    ),
                    (
                        "on",
                        CopyValue::String(
                            "your audio is transcribed on the service — sent over the verified channel, processed, and not kept. on while confidential processing is in use.",
                        ),
                    ),
                    (
                        "off",
                        CopyValue::String(
                            "speech becomes text on your device. your audio doesn't leave.",
                        ),
                    ),
                    (
                        "note",
                        CopyValue::String(
                            "turn it off any time — it takes effect on the next thing you say.",
                        ),
                    ),
                    (
                        "deferral",
                        CopyValue::String(
                            "transcription is waiting — nothing is sent until your journal verifies the service. your audio stays on your device and transcribes once the check passes.",
                        ),
                    ),
                ]),
            ),
            (
                "attestation_states",
                CopyValue::Object(&[
                    ("off", CopyValue::String("")),
                    (
                        "inactive",
                        CopyValue::String("confidential processing is available."),
                    ),
                    ("verifying", CopyValue::String("checking the hardware…")),
                    (
                        "verified",
                        CopyValue::String(
                            "confidential processing is ready · hardware verified {checked}",
                        ),
                    ),
                    (
                        "failed",
                        CopyValue::String("couldn't verify the service. nothing is being sent."),
                    ),
                    (
                        "stale",
                        CopyValue::String(
                            "your journal needs to re-check the service before sending.",
                        ),
                    ),
                    (
                        "unreachable",
                        CopyValue::String(
                            "can't reach confidential processing right now. nothing is being sent.",
                        ),
                    ),
                ]),
            ),
            (
                "operation_states",
                CopyValue::Object(&[
                    (
                        "starting",
                        CopyValue::String("opening your browser to confirm…"),
                    ),
                    (
                        "waiting",
                        CopyValue::String("finish turning it on in your browser"),
                    ),
                    (
                        "early_access",
                        CopyValue::String(
                            "confidential processing is coming — scouts get it first.",
                        ),
                    ),
                    (
                        "repair_needed",
                        CopyValue::String("couldn't verify the service. nothing is being sent."),
                    ),
                ]),
            ),
            (
                "actions",
                CopyValue::Object(&[
                    (
                        "off",
                        CopyValue::String("turn on confidential processing →"),
                    ),
                    ("enabled", CopyValue::String("turn off")),
                    ("recheck", CopyValue::String("check again")),
                ]),
            ),
        ]),
    ),
    (
        "glance",
        CopyValue::Object(&[
            ("lane_label", CopyValue::String("processing with")),
            (
                "local",
                CopyValue::Object(&[
                    ("value", CopyValue::String("a model on your device")),
                    (
                        "detail",
                        CopyValue::String(
                            "runs right on this computer. nothing leaves this machine to be processed",
                        ),
                    ),
                ]),
            ),
            (
                "byo_key",
                CopyValue::Object(&[
                    ("value", CopyValue::String("your own key · {provider}")),
                    (
                        "detail",
                        CopyValue::String(
                            "using {model}. a key you added, stays in your journal, never shared",
                        ),
                    ),
                ]),
            ),
            (
                "byo_endpoint",
                CopyValue::Object(&[
                    ("value", CopyValue::String("your own endpoint")),
                    (
                        "detail",
                        CopyValue::String(
                            "processing runs at the endpoint you set. your server, your rules",
                        ),
                    ),
                ]),
            ),
            (
                "confidential_checking",
                CopyValue::Object(&[
                    ("label", CopyValue::String("waiting on")),
                    ("value", CopyValue::String("confidential processing")),
                    ("detail", CopyValue::String("checking the hardware…")),
                ]),
            ),
            (
                "confidential_verified",
                CopyValue::Object(&[
                    ("label", CopyValue::String("processing with")),
                    ("value", CopyValue::String("confidential processing")),
                    (
                        "detail",
                        CopyValue::String(
                            "confidential processing is ready · hardware verified {checked}",
                        ),
                    ),
                ]),
            ),
            (
                "confidential_available",
                CopyValue::Object(&[
                    ("label", CopyValue::String("available")),
                    ("value", CopyValue::String("confidential processing")),
                    (
                        "detail",
                        CopyValue::String("confidential processing is available."),
                    ),
                ]),
            ),
            (
                "confidential_blocked",
                CopyValue::Object(&[
                    ("label", CopyValue::String("holding")),
                    ("value", CopyValue::String("confidential processing")),
                    ("detail", CopyValue::String("{message}")),
                ]),
            ),
            (
                "none",
                CopyValue::Object(&[
                    ("value", CopyValue::String("not thinking yet")),
                    (
                        "detail",
                        CopyValue::String(
                            "your journal is here. choose how processing runs below.",
                        ),
                    ),
                ]),
            ),
        ]),
    ),
    (
        "byo_setup",
        CopyValue::Object(&[
            (
                "intro",
                CopyValue::String(
                    "bring your own AI engine. sol pbc is never in the path — it stays in your journal.",
                ),
            ),
            ("chooser_key", CopyValue::String("a key")),
            ("chooser_endpoint", CopyValue::String("your own endpoint")),
            ("key_heading", CopyValue::String("pick your provider")),
            (
                "key_sub",
                CopyValue::String(
                    "all three work the same in solstone. choose the one you have a key for.",
                ),
            ),
            ("get_key", CopyValue::String("get a key ↗")),
            (
                "paste_title",
                CopyValue::String("paste your {provider} key"),
            ),
            (
                "key_hint",
                CopyValue::String(
                    "the key stays in your journal. sol pbc never sets it up or sees it. paste it once; processing uses it from here.",
                ),
            ),
            (
                "terms",
                CopyValue::String(
                    "your questions are processed by {provider}, stored only briefly for processing, and never used for training.",
                ),
            ),
            ("terms_link", CopyValue::String("terms ↗")),
            (
                "endpoint_heading",
                CopyValue::String("point processing at your own endpoint"),
            ),
            (
                "endpoint_sub",
                CopyValue::String("any OpenAI-compatible URL. your server, your rules."),
            ),
            (
                "endpoint_honesty",
                CopyValue::String(
                    "processing checks the endpoint works before relying on it. if it can't reach it, you'll see. it never quietly falls back to anyone else.",
                ),
            ),
            ("paste_cta", CopyValue::String("check this key →")),
            (
                "checking_key",
                CopyValue::String("checking your key with {provider}…"),
            ),
            (
                "key_ok_strip",
                CopyValue::String("your {provider} key works — checked {when}"),
            ),
            ("check_again", CopyValue::String("check again")),
            (
                "use_different_key",
                CopyValue::String("use a different key"),
            ),
            (
                "key_failed",
                CopyValue::String(
                    "this key didn't work — {reason}. paste a different key, or fix it with {provider} and check again.",
                ),
            ),
            (
                "reason_rejected",
                CopyValue::String("{provider} didn't accept it"),
            ),
            (
                "reason_quota",
                CopyValue::String("{provider} says it's out of quota right now"),
            ),
            (
                "reason_network",
                CopyValue::String("couldn't reach {provider} — check your connection"),
            ),
            (
                "reason_unknown",
                CopyValue::String("{provider} couldn't be checked"),
            ),
            (
                "model_heading",
                CopyValue::String("pick the model your key uses"),
            ),
            (
                "model_sub",
                CopyValue::String(
                    "three sizes from {provider} — or name one yourself. you can change this anytime.",
                ),
            ),
            (
                "tier_blurb_top",
                CopyValue::String("the most capable, for the heaviest thinking."),
            ),
            (
                "tier_blurb_mid",
                CopyValue::String("capable and quick. the middle of the range."),
            ),
            (
                "tier_blurb_lite",
                CopyValue::String(
                    "light and quick. tuned for small models, so this one does the job well.",
                ),
            ),
            ("tier_tag_suggested", CopyValue::String("suggested")),
            ("tier_tag_current", CopyValue::String("current")),
            (
                "custom_toggle",
                CopyValue::String("or name a specific model"),
            ),
            ("custom_label", CopyValue::String("model id")),
            ("custom_check", CopyValue::String("check it")),
            (
                "custom_checking",
                CopyValue::String("asking {provider} about {model}…"),
            ),
            (
                "custom_ok",
                CopyValue::String("✓ {model} answered — you can use it"),
            ),
            (
                "custom_not_found",
                CopyValue::String("{provider} doesn't offer \"{model}\" to this key."),
            ),
            ("model_save", CopyValue::String("use {label}")),
            ("model_save_restore", CopyValue::String("remember {label}")),
            (
                "model_saving",
                CopyValue::String("checking {model} with your key…"),
            ),
            (
                "model_saved_restore",
                CopyValue::String("remembered {label}. confidential processing stays on."),
            ),
            (
                "probe_failed_save",
                CopyValue::String("your key works, but {model} didn't answer — {reason}."),
            ),
        ]),
    ),
    (
        "lane_switch",
        CopyValue::Object(&[
            ("heading", CopyValue::String("switch how processing runs?")),
            ("current_label", CopyValue::String("now")),
            ("target_label", CopyValue::String("switch to")),
            ("confirm", CopyValue::String("switch")),
            ("cancel", CopyValue::String("keep using {current}")),
            (
                "to_local_note",
                CopyValue::String(
                    "processing will run right on this computer. your {current} setup stays saved. switch back anytime.",
                ),
            ),
            (
                "to_byo_note",
                CopyValue::String("processing will use your own engine. {setup} is still here."),
            ),
            ("setup_key", CopyValue::String("a saved key")),
            ("setup_endpoint", CopyValue::String("your endpoint")),
        ]),
    ),
    (
        "local_install",
        CopyValue::Object(&[
            (
                "phases",
                CopyValue::Object(&[
                    ("resolving", CopyValue::String("resolving")),
                    ("downloading", CopyValue::String("downloading")),
                    ("verifying", CopyValue::String("verifying")),
                    ("installing", CopyValue::String("installing")),
                ]),
            ),
            ("pill_inflight", CopyValue::String("setting up")),
            ("pill_failed", CopyValue::String("couldn't finish")),
            (
                "failed_verdict",
                CopyValue::String("local setup didn't finish"),
            ),
            (
                "failed_reason",
                CopyValue::String("local setup stopped before it finished."),
            ),
            ("retry", CopyValue::String("try setup again")),
            ("install", CopyValue::String("install local model")),
            (
                "notice_inflight",
                CopyValue::String("local thinking will stay in your journal once setup finishes."),
            ),
        ]),
    ),
    (
        "local_recovery",
        CopyValue::Object(&[
            ("retry", CopyValue::String("try starting local again")),
            (
                "states",
                CopyValue::Object(&[
                    (
                        "checking",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("checking")),
                            ("verdict", CopyValue::String("checking local setup")),
                            (
                                "reason",
                                CopyValue::String("checking the selected model and this computer."),
                            ),
                        ]),
                    ),
                    (
                        "starting",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("starting")),
                            ("verdict", CopyValue::String("starting local thinking")),
                            (
                                "reason",
                                CopyValue::String("checking the model before using it."),
                            ),
                        ]),
                    ),
                    (
                        "recovering",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("recovering")),
                            ("verdict", CopyValue::String("local thinking is recovering")),
                            ("reason", CopyValue::String("starting it again shortly.")),
                        ]),
                    ),
                    (
                        "retrying",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("retrying")),
                            ("verdict", CopyValue::String("trying local thinking again")),
                            (
                                "reason",
                                CopyValue::String("starting a new recovery attempt."),
                            ),
                        ]),
                    ),
                    (
                        "waiting",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("waiting")),
                            ("verdict", CopyValue::String("local thinking is waiting")),
                            (
                                "reason",
                                CopyValue::String("it will start when this computer is ready."),
                            ),
                        ]),
                    ),
                    (
                        "unsupported",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("unavailable")),
                            (
                                "verdict",
                                CopyValue::String("this computer can't run this local model"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "local thinking needs supported hardware on this computer.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "failed",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("needs attention")),
                            (
                                "verdict",
                                CopyValue::String("local thinking couldn't start"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "the local model stopped before it became ready.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "changing",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("changing")),
                            ("verdict", CopyValue::String("finishing the local change")),
                            (
                                "reason",
                                CopyValue::String("waiting for current local work to finish."),
                            ),
                        ]),
                    ),
                    (
                        "cleanup_failed",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("needs attention")),
                            (
                                "verdict",
                                CopyValue::String("local couldn't finish changing state"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "couldn't confirm that local processing stopped. it will keep checking safely.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "corrupt",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("can't verify")),
                            (
                                "verdict",
                                CopyValue::String("local status can't be confirmed"),
                            ),
                            (
                                "reason",
                                CopyValue::String("check again before changing local setup."),
                            ),
                        ]),
                    ),
                    (
                        "unavailable",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("can't verify")),
                            ("verdict", CopyValue::String("local status can't be read")),
                            (
                                "reason",
                                CopyValue::String(
                                    "correct the local file-access problem, then check again.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "stale",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("can't verify")),
                            (
                                "verdict",
                                CopyValue::String("local status couldn't be refreshed"),
                            ),
                            (
                                "reason",
                                CopyValue::String("check again before changing local setup."),
                            ),
                        ]),
                    ),
                    (
                        "ready",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("on")),
                            ("verdict", CopyValue::String("local thinking is ready")),
                            (
                                "reason",
                                CopyValue::String("a model is running on this computer."),
                            ),
                        ]),
                    ),
                    (
                        "ready_proof_unavailable",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("on, needs a check")),
                            (
                                "verdict",
                                CopyValue::String("local thinking is still running"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "couldn't refresh the local file check. it won't start a replacement until the check returns.",
                                ),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]),
    ),
]);

pub fn thinking_copy_payload() -> CopyValue {
    THINKING_COPY_PAYLOAD
}
