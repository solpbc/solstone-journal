// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owner-facing Thinking copy, represented without a JSON dependency.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

pub use solstone_core_assets::Platform;

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
        label: "local",
        sub: "on your journal's computer",
        description: "model requests stay on your journal's computer.",
    },
    Lane {
        id: "confidential",
        label: "confidential processing",
        sub: "operated by sol pbc",
        description: "sol pbc runs the model on confidential GPUs.",
    },
    Lane {
        id: "byo",
        label: "your own model",
        sub: "your key, or your own endpoint",
        description: "your journal sends model requests to the provider or endpoint you choose. provider keys stay in your journal.",
    },
];

/// Availability message when the current device has no admitted hardware check.
pub const CONFIDENTIAL_NOT_ON_PLATFORM: &str =
    "confidential processing isn't available yet where your journal lives.";

pub const CONFIDENTIAL_LANE_DETAIL: ConfidentialLaneDetail = ConfidentialLaneDetail {
    heading: "confidential processing",
    sub: "operated by sol pbc",
    mechanism: "sol pbc runs the model itself on confidential GPUs in Microsoft Azure. the hardware boundary keeps the cloud host excluded from what's processed. no third-party AI provider is in the path.",
    egress: "when it's on, the thinking leaves your device. your journal itself never leaves.",
    claims: "no content is retained · no human reviews it · nothing is used to train",
    attestation: "your journal must verify the service before anything is sent. if it can't verify, it doesn't send.",
    early_access: "the services portal shows how to get it.",
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
    // Shown only while the journal lists no paired device. The link goes to
    // the network page's devices section, the one place a device is added.
    (
        "devices_invite",
        CopyValue::Object(&[
            (
                "text",
                CopyValue::String(
                    "thinking works on what goes into your journal, and no device is paired to add to it yet.",
                ),
            ),
            ("link", CopyValue::String("pair a device")),
        ]),
    ),
    (
        "active_lane_labels",
        CopyValue::Object(&[
            ("none", CopyValue::String("not thinking yet")),
            ("local", CopyValue::String("local")),
            ("confidential", CopyValue::String("confidential processing")),
            ("byo", CopyValue::String("your own model")),
        ]),
    ),
    ("lanes", CopyValue::Lanes),
    (
        "provider_labels",
        CopyValue::Object(&[
            ("anthropic", CopyValue::String("Claude")),
            ("google", CopyValue::String("Gemini")),
            ("openai", CopyValue::String("GPT")),
            ("chatgpt", CopyValue::String("ChatGPT")),
            ("local", CopyValue::String("Local")),
            // X-03: a run on the confidential lane carries "spp" as its
            // provider. With no label for it the runs view rendered the raw
            // slug; the name here matches the lane label above.
            ("spp", CopyValue::String("confidential processing")),
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
            ("more_label", CopyValue::String("how it works")),
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
                                "your journal must verify the service before anything is sent. if it can't verify, it doesn't send.",
                            ),
                        ),
                        (
                            "substrate",
                            CopyValue::String(
                                "sol pbc runs the model itself on confidential GPUs in Microsoft Azure. the hardware boundary keeps the cloud host excluded from what's processed. no third-party AI provider is in the path.",
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
                            "your audio is transcribed on the service: sent over the verified channel, processed, and not kept. on while confidential processing is in use.",
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
                            "turn it off any time. it takes effect on the next thing you say.",
                        ),
                    ),
                    (
                        "deferral",
                        CopyValue::String(
                            "transcription is waiting. nothing is sent until your journal verifies the service. your audio stays on your device and is transcribed later, after the check passes.",
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
                        CopyValue::String("confidential processing is ready · reachable {checked}"),
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
                    (
                        "not_on_platform",
                        CopyValue::String(CONFIDENTIAL_NOT_ON_PLATFORM),
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
                            "your sign-in doesn't have confidential processing. the services portal shows how to get it.",
                        ),
                    ),
                    (
                        "subscribing",
                        CopyValue::String(
                            "confidential processing isn't on yet. finish turning it on in the services portal, and your journal completes it by itself while it's still waiting.",
                        ),
                    ),
                    (
                        "journal_limit",
                        CopyValue::String(
                            "your sign-in's confidential processing already covers another journal, and it covers only one. to use it here, release the other journal in the services portal, then turn it on here again.",
                        ),
                    ),
                    (
                        "repair_needed",
                        CopyValue::String(
                            "turning it on didn't finish, so confidential processing isn't on. turn it on again to start over.",
                        ),
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
            (
                "access_ended",
                CopyValue::String(
                    "confidential processing isn't active for your sign-in anymore, so your journal isn't sending your thinking to it. the services portal shows how to get it back, and your journal picks it up again by itself.",
                ),
            ),
            (
                "credential_unknown",
                CopyValue::String(
                    "the services portal no longer recognizes this journal for confidential processing. turn it off, then on again.",
                ),
            ),
            ("portal_link", CopyValue::String("open the services portal")),
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
                            "runs right on your journal's computer. nothing leaves it to be processed",
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
                        CopyValue::String("confidential processing is ready · reachable {checked}"),
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
                    "bring your own model. sol pbc is never in the path. your key or sign-in stays in your journal.",
                ),
            ),
            ("chooser_key", CopyValue::String("a provider")),
            ("chooser_endpoint", CopyValue::String("your own endpoint")),
            ("key_heading", CopyValue::String("pick your provider")),
            (
                "key_sub",
                CopyValue::String(
                    "they all work the same in solstone. choose one you have a key for, or sign in with ChatGPT.",
                ),
            ),
            (
                "chatgpt",
                CopyValue::Object(&[
                    (
                        "lane_signed_in",
                        CopyValue::String("using ChatGPT · manage →"),
                    ),
                    (
                        "lane_signed_out",
                        CopyValue::String("signed out of ChatGPT · sign in →"),
                    ),
                    (
                        "card_body",
                        CopyValue::String(
                            "use your ChatGPT plan. you sign in with ChatGPT, so there's no key to paste.",
                        ),
                    ),
                    ("pill_signed_in", CopyValue::String("signed in")),
                    ("manage", CopyValue::String("manage ChatGPT")),
                    (
                        "reminder",
                        CopyValue::String(
                            "OpenAI will see the parts of your journal that processing uses.",
                        ),
                    ),
                    (
                        "pending_heading",
                        CopyValue::String("finish signing in with ChatGPT"),
                    ),
                    (
                        "pending_sub",
                        CopyValue::String(
                            "ChatGPT opened in a new tab. sign in there, and this page carries on by itself.",
                        ),
                    ),
                    ("pending_status", CopyValue::String("waiting for ChatGPT…")),
                    ("reopen", CopyValue::String("open the ChatGPT tab again")),
                    ("cancel", CopyValue::String("cancel")),
                    (
                        "fallback_heading",
                        CopyValue::String("not on your journal's computer?"),
                    ),
                    (
                        "fallback_explanation",
                        CopyValue::String(
                            "when you finish there, the last page says it can't be reached. that's expected. copy that page's whole address and paste it here.",
                        ),
                    ),
                    ("address_label", CopyValue::String("that page's address")),
                    ("finish", CopyValue::String("finish signing in")),
                    (
                        "strip",
                        CopyValue::String("signed in to ChatGPT as {email}"),
                    ),
                    ("strip_no_email", CopyValue::String("signed in to ChatGPT")),
                    ("sign_out", CopyValue::String("sign out")),
                    (
                        "model_heading",
                        CopyValue::String("which model should processing use?"),
                    ),
                    (
                        "model_sub",
                        CopyValue::String(
                            "these are the models your ChatGPT account offers. you can change your pick anytime.",
                        ),
                    ),
                    ("model_save", CopyValue::String("use this model")),
                    ("start_over", CopyValue::String("start over with ChatGPT")),
                    (
                        "tuning_note",
                        CopyValue::String(
                            "thinking may improve results, and it can use up your ChatGPT usage faster. processing is tuned to work well without it.",
                        ),
                    ),
                    (
                        "plan_usage_not_granted",
                        CopyValue::String(
                            "sign-in finished, but ChatGPT usage wasn't allowed for solstone. continue with ChatGPT again and allow it when ChatGPT asks.",
                        ),
                    ),
                    (
                        "usage_limit_title",
                        CopyValue::String("usage limit reached"),
                    ),
                    (
                        "usage_limit_body",
                        CopyValue::String(
                            "a ChatGPT usage limit was reached. check usage and app limits in ChatGPT settings.",
                        ),
                    ),
                    (
                        "not_eligible",
                        CopyValue::String(
                            "this ChatGPT account can't be used for processing. start over with ChatGPT to use another account, or pick another provider.",
                        ),
                    ),
                    ("pick_other", CopyValue::String("pick another provider")),
                    (
                        "revoke_unconfirmed",
                        CopyValue::String(
                            "signed out on this journal. to finish, disconnect solstone in your ChatGPT settings.",
                        ),
                    ),
                    (
                        "denied",
                        CopyValue::String(
                            "sign-in was stopped in ChatGPT. continue with ChatGPT to try again.",
                        ),
                    ),
                    (
                        "expired",
                        CopyValue::String(
                            "sign-in didn't finish in time. continue with ChatGPT to try again.",
                        ),
                    ),
                    (
                        "unfinished",
                        CopyValue::String(
                            "sign-in didn't finish. continue with ChatGPT to try again.",
                        ),
                    ),
                    (
                        "account_mismatch",
                        CopyValue::String(
                            "that's a different ChatGPT account from the one signed in before. to change accounts, start over with ChatGPT.",
                        ),
                    ),
                    (
                        "registration_refused",
                        CopyValue::String(
                            "ChatGPT no longer accepts this journal's sign-in. start over with ChatGPT.",
                        ),
                    ),
                    (
                        "superseded",
                        CopyValue::String("another sign-in finished first."),
                    ),
                    (
                        "busy",
                        CopyValue::String("your journal was busy. try again in a moment."),
                    ),
                    (
                        "storage",
                        CopyValue::String(
                            "your journal can't read its ChatGPT sign-in. start over with ChatGPT.",
                        ),
                    ),
                    (
                        "keep_failing",
                        CopyValue::String("if it keeps failing, start over with ChatGPT."),
                    ),
                    (
                        "models_failed",
                        CopyValue::String(
                            "couldn't load your ChatGPT models. reload this page to try again.",
                        ),
                    ),
                    (
                        "signed_out",
                        CopyValue::String(
                            "signed out of ChatGPT. continue with ChatGPT to sign in again.",
                        ),
                    ),
                    (
                        "model_not_found",
                        CopyValue::String(
                            "your ChatGPT account no longer offers the model you picked. pick another.",
                        ),
                    ),
                    ("model_missing", CopyValue::String("pick a model first.")),
                    (
                        "popup_blocked",
                        CopyValue::String(
                            "your browser blocked the ChatGPT tab. allow pop-ups for this page, then try again.",
                        ),
                    ),
                    (
                        "sign_in_failed",
                        CopyValue::String("couldn't start signing in. try again in a moment."),
                    ),
                    (
                        "finish_refused",
                        CopyValue::String(
                            "that address isn't from this sign-in, or it's cut off. paste the last page's whole address.",
                        ),
                    ),
                    (
                        "save_refused",
                        CopyValue::String("couldn't save that choice. try again in a moment."),
                    ),
                    (
                        "sign_out_failed",
                        CopyValue::String("couldn't sign out of ChatGPT. try again in a moment."),
                    ),
                    (
                        "status_failed",
                        CopyValue::String(
                            "couldn't check your ChatGPT sign-in. reload this page to check again.",
                        ),
                    ),
                    (
                        "poll_failed",
                        CopyValue::String("couldn't reach your journal just now. still checking…"),
                    ),
                    ("continue", CopyValue::String("Continue with ChatGPT")),
                    ("plan", CopyValue::String("Using ChatGPT plan")),
                    ("manage_usage", CopyValue::String("Manage usage")),
                    ("manage_usage_link", CopyValue::String("Manage usage ↗")),
                ]),
            ),
            (
                "openai_card",
                CopyValue::String(
                    "use an OpenAI API key. this connects to OpenAI itself. for another OpenAI-compatible provider, use {endpoint}.",
                ),
            ),
            // One thinking setting for the whole "your own model" lane. The key
            // lanes ask the provider to think; the owner's own endpoint gets no
            // thinking field, so there it only leaves room, and its words say so.
            (
                "tuning",
                CopyValue::Object(&[
                    ("label", CopyValue::String("tuning")),
                    ("default", CopyValue::String("default")),
                    ("choice", CopyValue::String("{budget} tokens")),
                    ("saved", CopyValue::String("saved")),
                    (
                        "key",
                        CopyValue::Object(&[
                            ("state", CopyValue::String("thinking budget {budget}")),
                            (
                                "check",
                                CopyValue::String("let the model think longer before it answers"),
                            ),
                            ("budget", CopyValue::String("thinking budget")),
                            (
                                "note",
                                CopyValue::String(
                                    "thinking may improve results, and it costs more with your provider. processing is tuned to work well without it.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "endpoint",
                        CopyValue::Object(&[
                            ("state", CopyValue::String("thinking room {budget}")),
                            (
                                "check",
                                CopyValue::String("leave room for the model to think"),
                            ),
                            ("budget", CopyValue::String("thinking room")),
                            (
                                "note",
                                CopyValue::String(
                                    "processing doesn't switch thinking on or off at your endpoint. if your model thinks on its own, turn this on to give it room to think. processing is tuned to work well without thinking.",
                                ),
                            ),
                        ]),
                    ),
                ]),
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
            (
                "endpoint_image_checking",
                CopyValue::String("checking whether {model} reads images…"),
            ),
            (
                "endpoint_image_not_read",
                CopyValue::String(
                    "endpoint saved, but {model} didn't name the color of a test image, so screen and photo descriptions may fail or come out wrong. if it's meant to read images, try another model, then save again.",
                ),
            ),
            (
                "endpoint_image_unchecked",
                CopyValue::String(
                    "endpoint saved. couldn't check whether {model} reads images: {reason}.",
                ),
            ),
            (
                "endpoint_reason_unreachable",
                CopyValue::String("couldn't reach the endpoint"),
            ),
            (
                "endpoint_reason_timeout",
                CopyValue::String("it didn't answer in time"),
            ),
            (
                "endpoint_reason_busy",
                CopyValue::String("your journal was busy with other work"),
            ),
            (
                "endpoint_reason_key",
                CopyValue::String("it didn't accept the key"),
            ),
            (
                "endpoint_reason_rejected",
                CopyValue::String("it rejected the request"),
            ),
            (
                "endpoint_reason_unusable",
                CopyValue::String("it sent back nothing usable"),
            ),
            (
                "endpoint_reason_other",
                CopyValue::String("something went wrong"),
            ),
            ("paste_cta", CopyValue::String("check this key →")),
            (
                "checking_key",
                CopyValue::String("checking your key with {provider}…"),
            ),
            (
                "key_ok_strip",
                CopyValue::String("your {provider} key works, checked {when}"),
            ),
            ("check_again", CopyValue::String("check again")),
            (
                "use_different_key",
                CopyValue::String("use a different key"),
            ),
            (
                "key_failed",
                CopyValue::String(
                    "this key didn't work: {reason}. paste a different key, or fix it with {provider} and check again.",
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
                CopyValue::String("couldn't reach {provider}. check your connection"),
            ),
            (
                "reason_unknown",
                CopyValue::String("{provider} couldn't be checked"),
            ),
            (
                "model_heading",
                CopyValue::String("which model should your key use?"),
            ),
            (
                "model_sub",
                CopyValue::String(
                    "type the model id exactly as {provider} lists it. your journal checks it with {provider} before saving, and you can change it anytime.",
                ),
            ),
            ("model_label", CopyValue::String("model id")),
            (
                "custom_not_found",
                CopyValue::String(
                    "{provider} doesn't offer \"{model}\" to this key. check the spelling, or try another model id.",
                ),
            ),
            ("model_save", CopyValue::String("use this model")),
            (
                "model_save_restore",
                CopyValue::String("remember this model"),
            ),
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
                CopyValue::String("your key works, but {model} didn't answer: {reason}."),
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
                    "processing will run right on your journal's computer. your {current} setup stays saved. switch back anytime.",
                ),
            ),
            (
                "to_byo_note",
                CopyValue::String("processing will use your own model. {setup} is still here."),
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
            (
                "cancelled_verdict",
                CopyValue::String("installation cancelled"),
            ),
            (
                "download_failed",
                CopyValue::String("couldn't finish the download for local thinking. try again."),
            ),
            (
                "interrupted_verdict",
                CopyValue::String("installation stopped"),
            ),
            (
                "stopped_reason",
                CopyValue::String("you can try setup again when you're ready."),
            ),
            (
                "gpu_memory_insufficient",
                CopyValue::String(
                    "the selected GPU has less than the 6 GB of GPU memory needed for local thinking.",
                ),
            ),
            (
                "gpu_memory_unknown",
                CopyValue::String("couldn't check GPU memory on your journal's computer."),
            ),
            (
                "gpu_unavailable",
                CopyValue::String("couldn't find a supported GPU for local thinking."),
            ),
            (
                "integrity_failed",
                CopyValue::String("the downloaded files couldn't be verified. try setup again."),
            ),
            (
                "package_failed",
                CopyValue::String(
                    "some files needed for local thinking are missing. update the journal and try setup again.",
                ),
            ),
            (
                "package_invalid",
                CopyValue::String(
                    "some files needed for local thinking couldn't be verified. update the journal and try setup again.",
                ),
            ),
            (
                "unsupported_failed",
                CopyValue::String("this local model isn't supported on your journal's computer."),
            ),
            (
                "disk_failed",
                CopyValue::String(
                    "there isn't enough free disk space for local thinking. free up some space and try again.",
                ),
            ),
            (
                "memory_failed",
                CopyValue::String(
                    "your journal's computer doesn't have enough memory for local thinking.",
                ),
            ),
            (
                "unidentified_failed",
                CopyValue::String(
                    "couldn't tell why local setup stopped. check the diagnostic console for details.",
                ),
            ),
            ("retry", CopyValue::String("try setup again")),
            ("install", CopyValue::String("install local model")),
            ("cancel", CopyValue::String("cancel")),
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
                                CopyValue::String(
                                    "checking the selected model and your journal's computer.",
                                ),
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
                                CopyValue::String(
                                    "it will start when your journal's computer is ready.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "unsupported",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("unavailable")),
                            (
                                "verdict",
                                CopyValue::String(
                                    "your journal's computer can't run this local model",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "local thinking needs supported hardware where your journal lives.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_memory_insufficient",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("unavailable")),
                            (
                                "verdict",
                                CopyValue::String("the selected GPU needs more memory"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "local thinking needs a compatible GPU with at least 6 GB of GPU memory.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_probe_failed",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("couldn't check")),
                            ("verdict", CopyValue::String("couldn't check GPU memory")),
                            (
                                "reason",
                                CopyValue::String(
                                    "couldn't read GPU memory on your journal's computer. try checking again in a moment.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_allocation_failed",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("needs attention")),
                            (
                                "verdict",
                                CopyValue::String("local thinking ran out of memory"),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "the GPU didn't have enough available memory to start the local model.",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "local_probe_failed",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("couldn't check")),
                            ("verdict", CopyValue::String("couldn't check local setup")),
                            (
                                "reason",
                                CopyValue::String("try checking again in a moment."),
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
                                CopyValue::String("a model is running on your journal's computer."),
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
                                    "couldn't refresh the local status check. it won't start a replacement until the check returns.",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupCatalogFacts {
    pub platform: &'static str,
    pub thinking_download_bytes: Option<u64>,
    pub speech_download_bytes: Option<u64>,
}

pub fn current_setup_facts() -> SetupCatalogFacts {
    let platform = match std::env::consts::OS {
        "macos" => "mac",
        "windows" => "windows",
        "linux" => "linux",
        other => other,
    };

    let model_assets = solstone_core_assets::resolve("local-model", None, None);
    let gguf = model_assets
        .iter()
        .find(|a| a.filename == "Qwen3.5-4B-Q4_K_M.gguf");
    let mmproj = model_assets
        .iter()
        .find(|a| a.filename == "mmproj-F16.gguf");

    let model_sum = match (gguf, mmproj) {
        (Some(g), Some(m)) => Some(g.size_bytes.saturating_add(m.size_bytes)),
        _ => None,
    };

    let thinking_download_bytes = match std::env::consts::OS {
        "windows" => model_sum,
        "macos" if std::env::consts::ARCH == "aarch64" => {
            let macos_runtime = solstone_core_assets::resolve(
                "llama-server-vulkan",
                Some(solstone_core_assets::Platform::MacosArm64),
                None,
            );
            if macos_runtime.len() == 1 {
                model_sum.map(|sum| sum.saturating_add(macos_runtime[0].size_bytes))
            } else {
                None
            }
        }
        _ => None,
    };

    let speech_download_bytes = if std::env::consts::OS == "windows" {
        None
    } else {
        let speech_assets = solstone_core_assets::resolve("parakeet-model", None, None);
        speech_assets
            .iter()
            .find(|a| a.filename == "tdt-0.6b-v3-q8_0.gguf")
            .map(|a| a.size_bytes)
    };

    SetupCatalogFacts {
        platform,
        thinking_download_bytes,
        speech_download_bytes,
    }
}

/// Shared copy uses named placeholders; measured download facts live in setup state.
/// This stays callable before a journal exists or a runtime install plan is selected.
pub fn setup_copy(facts: &SetupCatalogFacts) -> serde_json::Value {
    // A phone can be the journal itself; everywhere else it is a computer.
    let on_phone = matches!(facts.platform, "ios" | "android");
    let choices = LANES
        .into_iter()
        .map(|mut lane| {
            // Setup actions use spp; the existing Thinking page uses confidential.
            if lane.id == "confidential" {
                lane.id = "spp";
            }
            lane
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema": "solstone.setup-copy.v1",
        "heading": "your journal",
        "welcome": {
            "title": "welcome to solstone.",
            "body": "the solstone app takes in what you share with it, and all of it goes into your journal.",
            "how_it_fits": [
                "welcome to solstone.",
                "the solstone app takes in what you share with it, and all of it goes into your journal.",
                "your journal is always private, only yours."
            ]
        },
        "fits": {
            "title": "how it fits together",
            "journal_term": "your journal",
            "journal": if on_phone {
                "lives on this phone and keeps everything that goes into it."
            } else {
                "lives on your journal's computer and keeps everything that goes into it."
            },
            "app_term": "the solstone app",
            "app": "runs on each device you choose and takes in what you share with it. all of it goes into your journal.",
            "thinking_term": "thinking",
            "thinking": "helps your journal make sense of what it keeps. choose where it happens below, or later."
        },
        "login": {
            "label": "start your journal when you sign in",
            "help": "start automatically after you sign in to your journal's computer."
        },
        "folder": {
            "title": "journal folder",
            "help": "where your journal is kept on your journal's computer.",
            "change": "change folder",
            "continue": "continue"
        },
        "mark": {
            "title": "your journal mark",
            "help": "a unique visual mark created for your journal.",
            "error": "your journal mark could not be loaded. try again.",
            "permanence": "continue makes this your journal's mark for good.",
            "try_another": "try another",
            "continue": "continue"
        },
        "thinking": {
            "title": "thinking",
            "choices": choices,
            "default_line": "on your journal's computer · {size}",
            "size_unknown": "download size unavailable",
            "choose_later": "choose thinking when you're ready. your journal keeps everything until then.",
            "change": "change",
            "later": "decide later",
            "continue": "continue",
            "check_unknown": "couldn't check local setup."
        },
        "downloads": {
            "speech": "speech recognition",
            "thinking": "local thinking",
            "progress": "downloading…",
            "cancel": "cancel",
            "retry": "try again",
            "error": "download failed. try again.",
            "idle": "not started",
            "complete": "ready",
            "cancelled": "cancelled"
        },
        "footer": if on_phone {
            "your journal is kept on this phone. thinking uses the option you choose."
        } else {
            "your journal is kept on your journal's computer. thinking uses the option you choose."
        },
        "help": {
            "text": "stuck, or have a question?",
            "link": "support.solstone.app",
            "href": "https://support.solstone.app"
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadMember {
    Catalog {
        unit: &'static str,
        platform: Option<solstone_core_assets::Platform>,
        backend: Option<solstone_core_assets::Backend>,
        filename: Option<&'static str>,
    },
    Packaged,
}

pub fn sum_download_bytes(members: &[DownloadMember]) -> Option<u64> {
    if members.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    for member in members {
        match member {
            DownloadMember::Packaged => {}
            DownloadMember::Catalog {
                unit,
                platform,
                backend,
                filename,
            } => {
                let artifacts = solstone_core_assets::resolve(unit, *platform, *backend);
                let mut matched = false;
                for artifact in artifacts {
                    if filename.is_none_or(|f| artifact.filename == f) {
                        matched = true;
                        total = total.checked_add(artifact.size_bytes)?;
                    }
                }
                if !matched {
                    return None;
                }
            }
        }
    }
    Some(total)
}

pub fn speech_catalog_members(
    platform: Option<solstone_core_assets::Platform>,
) -> Vec<DownloadMember> {
    match platform {
        Some(solstone_core_assets::Platform::MacosArm64) => vec![DownloadMember::Catalog {
            unit: "parakeet-coreml",
            platform,
            backend: None,
            filename: None,
        }],
        Some(
            solstone_core_assets::Platform::LinuxX64 | solstone_core_assets::Platform::LinuxArm64,
        ) => vec![
            DownloadMember::Catalog {
                unit: "parakeet-server",
                platform,
                backend: Some(solstone_core_assets::Backend::Cpu),
                filename: None,
            },
            DownloadMember::Catalog {
                unit: "parakeet-server",
                platform,
                backend: Some(solstone_core_assets::Backend::Vulkan),
                filename: None,
            },
            DownloadMember::Catalog {
                unit: "parakeet-model",
                platform: None,
                backend: None,
                filename: Some("tdt-0.6b-v3-q8_0.gguf"),
            },
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_copy_contains_required_groups() {
        let facts = current_setup_facts();
        let copy = setup_copy(&facts);
        assert_eq!(copy["schema"], "solstone.setup-copy.v1");
        assert!(copy.get("welcome").is_some());
        assert!(copy.get("fits").is_some());
        assert!(copy.get("login").is_some());
        assert!(copy.get("folder").is_some());
        assert!(copy.get("mark").is_some());
        assert!(copy.get("thinking").is_some());
        assert!(copy.get("downloads").is_some());
        assert!(copy.get("footer").is_some());

        for key in [
            "title",
            "journal_term",
            "journal",
            "app_term",
            "app",
            "thinking_term",
            "thinking",
        ] {
            assert!(
                copy["fits"][key]
                    .as_str()
                    .is_some_and(|text| !text.is_empty())
            );
        }
        assert!(
            copy["mark"]["error"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );

        let welcome = &copy["welcome"];
        assert_eq!(welcome["title"], "welcome to solstone.");
        assert_eq!(
            welcome["body"],
            "the solstone app takes in what you share with it, and all of it goes into your journal."
        );
        let beats = welcome["how_it_fits"]
            .as_array()
            .expect("how_it_fits array");
        assert_eq!(beats.len(), 3);
        assert_eq!(beats[0], "welcome to solstone.");
        assert_eq!(
            beats[1],
            "the solstone app takes in what you share with it, and all of it goes into your journal."
        );
        assert_eq!(beats[2], "your journal is always private, only yours.");
    }

    #[test]
    fn setup_copy_is_pure_over_facts() {
        let with_sizes = SetupCatalogFacts {
            platform: "testbox",
            thinking_download_bytes: Some(123),
            speech_download_bytes: Some(456),
        };
        let without_sizes = SetupCatalogFacts {
            platform: "otherbox",
            thinking_download_bytes: None,
            speech_download_bytes: None,
        };
        let copy = setup_copy(&with_sizes);
        assert_eq!(copy, setup_copy(&without_sizes));
        assert!(
            copy["thinking"]["default_line"]
                .as_str()
                .unwrap()
                .contains("{size}")
        );
    }

    #[test]
    fn setup_choice_ids_are_action_ids_and_copy_comes_from_shared_lanes() {
        let copy = setup_copy(&current_setup_facts());
        let choices = copy["thinking"]["choices"].as_array().unwrap();
        let mut ids = choices
            .iter()
            .map(|choice| choice["id"].as_str().unwrap())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        assert_eq!(ids, ["byo", "local", "spp"]);
        for (choice, lane) in choices.iter().zip(LANES) {
            assert_eq!(choice["label"], lane.label);
            assert_eq!(choice["sub"], lane.sub);
            assert_eq!(choice["description"], lane.description);
        }
    }

    #[test]
    fn thinking_copy_payload_stays_callable_and_unchanged() {
        let payload = thinking_copy_payload();
        let value = serde_json::to_value(payload).expect("serializes");
        assert!(value.is_object());
        assert!(value.get("heading").is_some());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_facts_omit_thinking_size() {
        let facts = current_setup_facts();
        assert_eq!(facts.platform, "linux");
        assert!(facts.thinking_download_bytes.is_none());
        assert!(facts.speech_download_bytes.is_some());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_facts_omit_speech_size() {
        let facts = current_setup_facts();
        assert_eq!(facts.platform, "windows");
        assert!(facts.speech_download_bytes.is_none());
        assert!(facts.thinking_download_bytes.is_some());
    }

    #[test]
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn macos_arm64_facts_include_both_sizes() {
        let facts = current_setup_facts();
        assert_eq!(facts.platform, "mac");
        assert!(facts.thinking_download_bytes.is_some());
        assert!(facts.speech_download_bytes.is_some());
    }

    #[test]
    fn cuda_and_vulkan_selection_sum_different_runtime_and_same_model_rows() {
        use solstone_core_assets::{Platform, resolve};

        let cuda_runtime = DownloadMember::Catalog {
            unit: "llama-server-cuda",
            platform: Some(Platform::LinuxX64),
            backend: None,
            filename: None,
        };
        let vulkan_runtime = DownloadMember::Catalog {
            unit: "llama-server-vulkan",
            platform: Some(Platform::LinuxX64),
            backend: None,
            filename: Some("llama-b11669-bin-ubuntu-vulkan-x64.tar.gz"),
        };
        let model_1 = DownloadMember::Catalog {
            unit: "local-model",
            platform: None,
            backend: None,
            filename: Some("Qwen3.5-4B-Q4_K_M.gguf"),
        };
        let model_2 = DownloadMember::Catalog {
            unit: "local-model",
            platform: None,
            backend: None,
            filename: Some("mmproj-F16.gguf"),
        };

        let cuda_members = vec![cuda_runtime, model_1.clone(), model_2.clone()];
        let vulkan_members = vec![vulkan_runtime, model_1, model_2];

        let cuda_sum = sum_download_bytes(&cuda_members).expect("cuda sum resolves");
        let vulkan_sum = sum_download_bytes(&vulkan_members).expect("vulkan sum resolves");

        assert_ne!(cuda_sum, vulkan_sum);

        let expected_cuda_runtime: u64 =
            resolve("llama-server-cuda", Some(Platform::LinuxX64), None)
                .iter()
                .map(|a| a.size_bytes)
                .sum();
        let expected_vulkan_runtime: u64 =
            resolve("llama-server-vulkan", Some(Platform::LinuxX64), None)
                .iter()
                .filter(|a| a.filename == "llama-b11669-bin-ubuntu-vulkan-x64.tar.gz")
                .map(|a| a.size_bytes)
                .sum();
        let expected_models: u64 = resolve("local-model", None, None)
            .iter()
            .filter(|a| a.filename == "Qwen3.5-4B-Q4_K_M.gguf" || a.filename == "mmproj-F16.gguf")
            .map(|a| a.size_bytes)
            .sum();

        assert_eq!(cuda_sum, expected_cuda_runtime + expected_models);
        assert_eq!(vulkan_sum, expected_vulkan_runtime + expected_models);

        let mut with_packaged = cuda_members.clone();
        with_packaged.push(DownloadMember::Packaged);
        assert_eq!(sum_download_bytes(&with_packaged), Some(cuda_sum));
    }

    #[test]
    fn mac_speech_members_are_parakeet_coreml_and_sum_matches_resolve() {
        use solstone_core_assets::{Platform, resolve};

        let members = speech_catalog_members(Some(Platform::MacosArm64));
        assert_eq!(members.len(), 1);
        match &members[0] {
            DownloadMember::Catalog {
                unit,
                platform,
                backend,
                filename,
            } => {
                assert_eq!(*unit, "parakeet-coreml");
                assert_eq!(*platform, Some(Platform::MacosArm64));
                assert_eq!(*backend, None);
                assert_eq!(*filename, None);
            }
            _ => panic!("expected catalog member"),
        }

        let total = sum_download_bytes(&members).expect("sum resolves");
        let expected: u64 = resolve("parakeet-coreml", Some(Platform::MacosArm64), None)
            .iter()
            .map(|a| a.size_bytes)
            .sum();
        assert_eq!(total, expected);
    }

    #[test]
    fn missing_filename_yields_none_not_partial_sum() {
        let valid_model = DownloadMember::Catalog {
            unit: "local-model",
            platform: None,
            backend: None,
            filename: Some("Qwen3.5-4B-Q4_K_M.gguf"),
        };
        let missing_model = DownloadMember::Catalog {
            unit: "local-model",
            platform: None,
            backend: None,
            filename: Some("missing-file-does-not-exist.gguf"),
        };

        assert!(sum_download_bytes(std::slice::from_ref(&valid_model)).is_some());
        assert_eq!(sum_download_bytes(&[valid_model, missing_model]), None);
    }
}

#[cfg(test)]
mod speech_catalog_tests {
    use super::*;
    use solstone_core_assets::{Backend, Platform, resolve};

    #[test]
    fn linux_speech_total_includes_both_selected_runtimes_and_the_model() {
        for platform in [Platform::LinuxX64, Platform::LinuxArm64] {
            let selected = [Backend::Cpu, Backend::Vulkan]
                .into_iter()
                .flat_map(|backend| resolve("parakeet-server", Some(platform), Some(backend)))
                .chain(resolve("parakeet-model", None, None));
            let expected = selected.map(|row| row.size_bytes).sum::<u64>();
            assert_eq!(
                sum_download_bytes(&speech_catalog_members(Some(platform))),
                Some(expected)
            );
        }
        assert_eq!(sum_download_bytes(&speech_catalog_members(None)), None);
    }
}
