// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The Thinking view and setup share journal-window messages.

use serde::Serialize;
use serde_json::Value;
use solstone_core_window_catalog::{ENGLISH, Value as CatalogValue, render, text};

pub use solstone_core_assets::Platform;

#[derive(Clone, Serialize)]
pub struct Lane<T = String> {
    pub id: &'static str,
    pub label: T,
    pub sub: T,
    pub description: T,
}

#[derive(Clone, Serialize)]
pub struct ConfidentialLaneDetail<T = String> {
    pub heading: T,
    pub sub: T,
    pub mechanism: T,
    pub egress: T,
    pub claims: T,
    pub attestation: T,
    pub early_access: T,
}

const LANE_IDS: [Lane<&str>; 3] = [
    Lane {
        id: "local",
        label: "journal.window.thinking.lane.label.local",
        sub: "journal.window.thinking.lane.sub.on.your.journal.s.computer",
        description: "!journal.window.thinking.lane.description.model.requests.stay.on.your.journal",
    },
    Lane {
        id: "confidential",
        label: "journal.window.thinking.lane.label.confidential.processing",
        sub: "journal.window.thinking.lane.sub.operated.by.sol.pbc",
        description: "!journal.window.thinking.lane.description.sol.pbc.runs.the.model.on",
    },
    Lane {
        id: "byo",
        label: "journal.window.thinking.lane.label.your.own.model",
        sub: "journal.window.thinking.lane.sub.your.key.or.your.own.endpoint",
        description: "!journal.window.thinking.lane.description.your.journal.sends.model.requests.to",
    },
];

const CONFIDENTIAL_DETAIL_IDS: ConfidentialLaneDetail<&str> = ConfidentialLaneDetail {
    heading: "journal.window.thinking.lane.heading.confidential.processing",
    sub: "journal.window.thinking.lane.sub.operated.by.sol.pbc",
    mechanism: "!journal.window.thinking.lane.mechanism.sol.pbc.runs.the.model.itself",
    egress: "!journal.window.thinking.lane.egress.when.it.s.on.the.thinking",
    claims: "!journal.window.thinking.lane.claims.no.content.is.retained.no.human",
    attestation: "!journal.window.thinking.lane.attestation.your.journal.must.verify.the.service",
    early_access: "journal.window.thinking.lane.early.access.the.services.portal.shows.how.to",
};

pub fn lanes(locale: &str) -> [Lane; 3] {
    LANE_IDS.map(|lane| Lane {
        id: lane.id,
        label: text(locale, lane.label),
        sub: text(locale, lane.sub),
        description: text(locale, lane.description),
    })
}

pub fn confidential_lane_detail(locale: &str) -> ConfidentialLaneDetail {
    let detail = CONFIDENTIAL_DETAIL_IDS;
    ConfidentialLaneDetail {
        heading: text(locale, detail.heading),
        sub: text(locale, detail.sub),
        mechanism: text(locale, detail.mechanism),
        egress: text(locale, detail.egress),
        claims: text(locale, detail.claims),
        attestation: text(locale, detail.attestation),
        early_access: text(locale, detail.early_access),
    }
}

#[derive(Clone, Copy)]
enum CopyValue {
    String(&'static str),
    LegacyTemplate(&'static str, &'static [(&'static str, &'static str)]),
    Object(&'static [(&'static str, CopyValue)]),
    Lanes,
    ConfidentialLaneDetail,
}

impl CopyValue {
    fn localized(self, locale: &str) -> Value {
        match self {
            Self::String(value) if value.trim_start_matches('!').starts_with("journal.window.") => {
                Value::String(text(locale, value))
            }
            Self::String(value) => Value::String(value.to_owned()),
            // Existing clients can still read the former template-shaped fields.
            Self::LegacyTemplate(id, bindings) => {
                let args: Vec<_> = bindings
                    .iter()
                    .map(|&(name, placeholder)| (name, CatalogValue::String(placeholder)))
                    .collect();
                Value::String(render(locale, id, &args))
            }
            Self::Object(entries) => Value::Object(
                entries
                    .iter()
                    .map(|&(key, copy)| (key.to_owned(), copy.localized(locale)))
                    .collect(),
            ),
            Self::Lanes => serde_json::to_value(lanes(locale)).expect("Thinking lanes are JSON"),
            Self::ConfidentialLaneDetail => serde_json::to_value(confidential_lane_detail(locale))
                .expect("Thinking lane detail is JSON"),
        }
    }
}

const THINKING_COPY_PAYLOAD: CopyValue = CopyValue::Object(&[
    (
        "heading",
        CopyValue::String("journal.window.thinking.copy.thinking"),
    ),
    // Shown only while the journal lists no paired device. The link goes to
    // the network page's devices section, the one place a device is added.
    (
        "devices_invite",
        CopyValue::Object(&[
            (
                "text",
                CopyValue::String("journal.window.thinking.copy.thinking.works.on.what.goes.into"),
            ),
            (
                "link",
                CopyValue::String("journal.window.thinking.copy.pair.a.device"),
            ),
        ]),
    ),
    (
        "active_lane_labels",
        CopyValue::Object(&[
            (
                "none",
                CopyValue::String("journal.window.thinking.copy.not.thinking.yet"),
            ),
            (
                "local",
                CopyValue::String("journal.window.thinking.copy.local"),
            ),
            (
                "confidential",
                CopyValue::String("journal.window.thinking.copy.confidential.processing"),
            ),
            (
                "byo",
                CopyValue::String("journal.window.thinking.copy.your.own.model"),
            ),
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
            (
                "local",
                CopyValue::String("journal.window.thinking.copy.local.variant2"),
            ),
            // X-03: a run on the confidential lane carries "spp" as its
            // provider. With no label for it the runs view rendered the raw
            // slug; the name here matches the lane label above.
            (
                "spp",
                CopyValue::String("journal.window.thinking.copy.confidential.processing"),
            ),
        ]),
    ),
    (
        "key_labels",
        CopyValue::Object(&[
            (
                "ANTHROPIC_API_KEY",
                CopyValue::String("journal.window.thinking.copy.claude.key"),
            ),
            (
                "GOOGLE_API_KEY",
                CopyValue::String("journal.window.thinking.copy.gemini.key"),
            ),
            (
                "OPENAI_API_KEY",
                CopyValue::String("journal.window.thinking.copy.gpt.key"),
            ),
        ]),
    ),
    (
        "state_labels",
        CopyValue::Object(&[
            (
                "active",
                CopyValue::String("journal.window.thinking.copy.active"),
            ),
            (
                "available",
                CopyValue::String("journal.window.thinking.copy.available"),
            ),
            (
                "unavailable",
                CopyValue::String("journal.window.thinking.copy.not.ready"),
            ),
            (
                "loading",
                CopyValue::String("journal.window.thinking.copy.loading"),
            ),
            (
                "saved",
                CopyValue::String("journal.window.thinking.copy.saved"),
            ),
            (
                "validating",
                CopyValue::String("journal.window.thinking.copy.validating"),
            ),
            (
                "failed",
                CopyValue::String("journal.window.thinking.copy.couldn.t.finish"),
            ),
        ]),
    ),
    (
        "action_labels",
        CopyValue::Object(&[
            (
                "switch",
                CopyValue::String("journal.window.thinking.copy.use.this.lane"),
            ),
            (
                "save_key",
                CopyValue::String("journal.window.thinking.copy.save.key"),
            ),
            (
                "clear_key",
                CopyValue::String("journal.window.thinking.copy.clear.key"),
            ),
            (
                "install",
                CopyValue::String("journal.window.thinking.copy.install"),
            ),
            (
                "refresh",
                CopyValue::String("journal.window.thinking.copy.refresh"),
            ),
            (
                "check",
                CopyValue::String("journal.window.thinking.copy.check"),
            ),
        ]),
    ),
    (
        "confidential",
        CopyValue::Object(&[
            ("lane_detail", CopyValue::ConfidentialLaneDetail),
            (
                "more_label",
                CopyValue::String("journal.window.thinking.copy.how.it.works"),
            ),
            (
                "setup",
                CopyValue::Object(&[(
                    "trust_beats",
                    CopyValue::Object(&[
                        (
                            "heading",
                            CopyValue::String(
                                "journal.window.thinking.copy.confidential.processing",
                            ),
                        ),
                        (
                            "sub",
                            CopyValue::String("journal.window.thinking.copy.operated.by.sol.pbc"),
                        ),
                        (
                            "egress_audio_on",
                            CopyValue::String(
                                "!journal.window.thinking.copy.what.leaves.your.device.the.text",
                            ),
                        ),
                        (
                            "egress_audio_off",
                            CopyValue::String(
                                "!journal.window.thinking.copy.what.leaves.your.device.the.text.variant2",
                            ),
                        ),
                        (
                            "claims",
                            CopyValue::String(
                                "!journal.window.thinking.copy.no.content.is.retained.no.human",
                            ),
                        ),
                        (
                            "attestation",
                            CopyValue::String(
                                "!journal.window.thinking.copy.your.journal.must.verify.the.service",
                            ),
                        ),
                        (
                            "substrate",
                            CopyValue::String(
                                "!journal.window.thinking.copy.sol.pbc.runs.the.model.itself",
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
                        CopyValue::String(
                            "journal.window.thinking.copy.transcribe.audio.on.the.service",
                        ),
                    ),
                    (
                        "on",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.audio.is.transcribed.on.the",
                        ),
                    ),
                    (
                        "off",
                        CopyValue::String(
                            "journal.window.thinking.copy.speech.becomes.text.on.your.device",
                        ),
                    ),
                    (
                        "note",
                        CopyValue::String("journal.window.thinking.copy.turn.it.off.any.time.it"),
                    ),
                    (
                        "deferral",
                        CopyValue::String(
                            "!journal.window.thinking.copy.transcription.is.waiting.nothing.is.sent",
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
                        CopyValue::String(
                            "journal.window.thinking.copy.confidential.processing.is.available",
                        ),
                    ),
                    (
                        "verifying",
                        CopyValue::String("journal.window.thinking.copy.checking.the.hardware"),
                    ),
                    (
                        "verified",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.confidential.attestation.checked",
                            &[("checked_at", "{checked}")],
                        ),
                    ),
                    (
                        "failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.verify.the.service.nothing",
                        ),
                    ),
                    (
                        "stale",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.journal.needs.to.re.check",
                        ),
                    ),
                    (
                        "unreachable",
                        CopyValue::String(
                            "journal.window.thinking.copy.can.t.reach.confidential.processing.right",
                        ),
                    ),
                    (
                        "not_on_platform",
                        CopyValue::String("journal.window.thinking.error.confidential.platform"),
                    ),
                ]),
            ),
            (
                "operation_states",
                CopyValue::Object(&[
                    (
                        "starting",
                        CopyValue::String(
                            "journal.window.thinking.copy.opening.your.browser.to.confirm",
                        ),
                    ),
                    (
                        "waiting",
                        CopyValue::String(
                            "journal.window.thinking.copy.finish.turning.it.on.in.your",
                        ),
                    ),
                    (
                        "early_access",
                        CopyValue::String(
                            "!journal.window.thinking.copy.your.sign.in.doesn.t.have",
                        ),
                    ),
                    (
                        "subscribing",
                        CopyValue::String(
                            "!journal.window.thinking.copy.confidential.processing.isn.t.on.yet",
                        ),
                    ),
                    (
                        "journal_limit",
                        CopyValue::String(
                            "!journal.window.thinking.copy.your.sign.in.s.confidential.processing",
                        ),
                    ),
                    (
                        "repair_needed",
                        CopyValue::String(
                            "journal.window.thinking.copy.turning.it.on.didn.t.finish",
                        ),
                    ),
                ]),
            ),
            (
                "actions",
                CopyValue::Object(&[
                    (
                        "off",
                        CopyValue::String(
                            "journal.window.thinking.copy.turn.on.confidential.processing",
                        ),
                    ),
                    (
                        "enabled",
                        CopyValue::String("journal.window.thinking.copy.turn.off"),
                    ),
                    (
                        "recheck",
                        CopyValue::String("journal.window.thinking.copy.check.again"),
                    ),
                ]),
            ),
            (
                "access_ended",
                CopyValue::String(
                    "!journal.window.thinking.copy.confidential.processing.isn.t.active.for",
                ),
            ),
            (
                "credential_unknown",
                CopyValue::String(
                    "journal.window.thinking.copy.the.services.portal.no.longer.recognizes",
                ),
            ),
            (
                "portal_link",
                CopyValue::String("!journal.window.thinking.copy.open.the.services.portal"),
            ),
        ]),
    ),
    (
        "glance",
        CopyValue::Object(&[
            (
                "lane_label",
                CopyValue::String("journal.window.thinking.copy.processing.with"),
            ),
            (
                "local",
                CopyValue::Object(&[
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.a.model.on.your.device"),
                    ),
                    (
                        "detail",
                        CopyValue::String(
                            "journal.window.thinking.copy.runs.right.on.your.journal.s",
                        ),
                    ),
                ]),
            ),
            (
                "byo_key",
                CopyValue::Object(&[
                    (
                        "value",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.glance.byo.provider",
                            &[("provider_name", "{provider}")],
                        ),
                    ),
                    (
                        "detail",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.glance.byo.model",
                            &[("model_name", "{model}")],
                        ),
                    ),
                ]),
            ),
            (
                "byo_endpoint",
                CopyValue::Object(&[
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.your.own.endpoint"),
                    ),
                    (
                        "detail",
                        CopyValue::String(
                            "journal.window.thinking.copy.processing.runs.at.the.endpoint.you",
                        ),
                    ),
                ]),
            ),
            (
                "confidential_checking",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("journal.window.thinking.copy.waiting.on"),
                    ),
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.confidential.processing"),
                    ),
                    (
                        "detail",
                        CopyValue::String("journal.window.thinking.copy.checking.the.hardware"),
                    ),
                ]),
            ),
            (
                "confidential_verified",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("journal.window.thinking.copy.processing.with"),
                    ),
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.confidential.processing"),
                    ),
                    (
                        "detail",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.confidential.attestation.checked",
                            &[("checked_at", "{checked}")],
                        ),
                    ),
                ]),
            ),
            (
                "confidential_available",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("journal.window.thinking.copy.available"),
                    ),
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.confidential.processing"),
                    ),
                    (
                        "detail",
                        CopyValue::String(
                            "journal.window.thinking.copy.confidential.processing.is.available",
                        ),
                    ),
                ]),
            ),
            (
                "confidential_blocked",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("journal.window.thinking.copy.holding"),
                    ),
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.confidential.processing"),
                    ),
                    (
                        "detail",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.confidential.attestation.failed.detail.legacy",
                            &[("attestation_detail", "{message}")],
                        ),
                    ),
                ]),
            ),
            (
                "none",
                CopyValue::Object(&[
                    (
                        "value",
                        CopyValue::String("journal.window.thinking.copy.not.thinking.yet"),
                    ),
                    (
                        "detail",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.journal.is.here.choose.how",
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
                CopyValue::String("journal.window.thinking.copy.bring.your.own.model.sol.pbc"),
            ),
            (
                "chooser_key",
                CopyValue::String("journal.window.thinking.copy.a.provider"),
            ),
            (
                "chooser_endpoint",
                CopyValue::String("journal.window.thinking.copy.your.own.endpoint"),
            ),
            (
                "key_heading",
                CopyValue::String("journal.window.thinking.copy.pick.your.provider"),
            ),
            (
                "key_sub",
                CopyValue::String("journal.window.thinking.copy.they.all.work.the.same.in"),
            ),
            (
                "chatgpt",
                CopyValue::Object(&[
                    (
                        "lane_signed_in",
                        CopyValue::String("journal.window.thinking.copy.using.chatgpt.manage"),
                    ),
                    (
                        "lane_signed_out",
                        CopyValue::String(
                            "journal.window.thinking.copy.signed.out.of.chatgpt.sign.in",
                        ),
                    ),
                    (
                        "card_body",
                        CopyValue::String(
                            "journal.window.thinking.copy.use.your.chatgpt.plan.you.sign",
                        ),
                    ),
                    (
                        "pill_signed_in",
                        CopyValue::String("journal.window.thinking.copy.signed.in"),
                    ),
                    (
                        "manage",
                        CopyValue::String("journal.window.thinking.copy.manage.chatgpt"),
                    ),
                    (
                        "reminder",
                        CopyValue::String(
                            "journal.window.thinking.copy.openai.will.see.the.parts.of",
                        ),
                    ),
                    (
                        "pending_heading",
                        CopyValue::String(
                            "journal.window.thinking.copy.finish.signing.in.with.chatgpt",
                        ),
                    ),
                    (
                        "pending_sub",
                        CopyValue::String(
                            "journal.window.thinking.copy.chatgpt.opened.in.a.new.tab",
                        ),
                    ),
                    (
                        "pending_status",
                        CopyValue::String("journal.window.thinking.copy.waiting.for.chatgpt"),
                    ),
                    (
                        "reopen",
                        CopyValue::String(
                            "journal.window.thinking.copy.open.the.chatgpt.tab.again",
                        ),
                    ),
                    (
                        "cancel",
                        CopyValue::String("journal.window.thinking.copy.cancel"),
                    ),
                    (
                        "fallback_heading",
                        CopyValue::String(
                            "journal.window.thinking.copy.not.on.your.journal.s.computer",
                        ),
                    ),
                    (
                        "fallback_explanation",
                        CopyValue::String(
                            "journal.window.thinking.copy.when.you.finish.there.the.last",
                        ),
                    ),
                    (
                        "address_label",
                        CopyValue::String("journal.window.thinking.copy.that.page.s.address"),
                    ),
                    (
                        "finish",
                        CopyValue::String("journal.window.thinking.copy.finish.signing.in"),
                    ),
                    (
                        "strip",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.chatgpt.account.signed.in",
                            &[("account_email", "{email}")],
                        ),
                    ),
                    (
                        "strip_no_email",
                        CopyValue::String("journal.window.thinking.copy.signed.in.to.chatgpt"),
                    ),
                    (
                        "sign_out",
                        CopyValue::String("journal.window.thinking.copy.sign.out"),
                    ),
                    (
                        "model_heading",
                        CopyValue::String(
                            "journal.window.thinking.copy.which.model.should.processing.use",
                        ),
                    ),
                    (
                        "model_sub",
                        CopyValue::String(
                            "journal.window.thinking.copy.these.are.the.models.your.chatgpt",
                        ),
                    ),
                    (
                        "model_save",
                        CopyValue::String("journal.window.thinking.copy.use.this.model"),
                    ),
                    (
                        "start_over",
                        CopyValue::String("journal.window.thinking.copy.start.over.with.chatgpt"),
                    ),
                    (
                        "tuning_note",
                        CopyValue::String(
                            "journal.window.thinking.copy.thinking.may.improve.results.and.it",
                        ),
                    ),
                    (
                        "plan_usage_not_granted",
                        CopyValue::String(
                            "journal.window.thinking.copy.sign.in.finished.but.chatgpt.usage",
                        ),
                    ),
                    (
                        "usage_limit_title",
                        CopyValue::String("journal.window.thinking.copy.usage.limit.reached"),
                    ),
                    (
                        "usage_limit_body",
                        CopyValue::String(
                            "journal.window.thinking.copy.a.chatgpt.usage.limit.was.reached",
                        ),
                    ),
                    (
                        "not_eligible",
                        CopyValue::String(
                            "journal.window.thinking.copy.this.chatgpt.account.can.t.be",
                        ),
                    ),
                    (
                        "pick_other",
                        CopyValue::String("journal.window.thinking.copy.pick.another.provider"),
                    ),
                    (
                        "revoke_unconfirmed",
                        CopyValue::String(
                            "journal.window.thinking.copy.signed.out.on.this.journal.to",
                        ),
                    ),
                    (
                        "denied",
                        CopyValue::String(
                            "journal.window.thinking.copy.sign.in.was.stopped.in.chatgpt",
                        ),
                    ),
                    (
                        "expired",
                        CopyValue::String("journal.window.thinking.copy.sign.in.didn.t.finish.in"),
                    ),
                    (
                        "unfinished",
                        CopyValue::String(
                            "journal.window.thinking.copy.sign.in.didn.t.finish.continue",
                        ),
                    ),
                    (
                        "account_mismatch",
                        CopyValue::String(
                            "journal.window.thinking.copy.that.s.a.different.chatgpt.account",
                        ),
                    ),
                    (
                        "registration_refused",
                        CopyValue::String(
                            "journal.window.thinking.copy.chatgpt.no.longer.accepts.this.journal",
                        ),
                    ),
                    (
                        "superseded",
                        CopyValue::String(
                            "journal.window.thinking.copy.another.sign.in.finished.first",
                        ),
                    ),
                    (
                        "busy",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.journal.was.busy.try.again",
                        ),
                    ),
                    (
                        "storage",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.journal.can.t.read.its",
                        ),
                    ),
                    (
                        "keep_failing",
                        CopyValue::String(
                            "journal.window.thinking.copy.if.it.keeps.failing.start.over",
                        ),
                    ),
                    (
                        "models_failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.load.your.chatgpt.models",
                        ),
                    ),
                    (
                        "signed_out",
                        CopyValue::String(
                            "journal.window.thinking.copy.signed.out.of.chatgpt.continue.with",
                        ),
                    ),
                    (
                        "model_not_found",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.chatgpt.account.no.longer.offers",
                        ),
                    ),
                    (
                        "model_missing",
                        CopyValue::String("journal.window.thinking.copy.pick.a.model.first"),
                    ),
                    (
                        "popup_blocked",
                        CopyValue::String(
                            "journal.window.thinking.copy.your.browser.blocked.the.chatgpt.tab",
                        ),
                    ),
                    (
                        "sign_in_failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.start.signing.in.try",
                        ),
                    ),
                    (
                        "finish_refused",
                        CopyValue::String(
                            "journal.window.thinking.copy.that.address.isn.t.from.this",
                        ),
                    ),
                    (
                        "save_refused",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.save.that.choice.try",
                        ),
                    ),
                    (
                        "sign_out_failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.sign.out.of.chatgpt",
                        ),
                    ),
                    (
                        "status_failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.check.your.chatgpt.sign",
                        ),
                    ),
                    (
                        "poll_failed",
                        CopyValue::String(
                            "journal.window.thinking.copy.couldn.t.reach.your.journal.just",
                        ),
                    ),
                    (
                        "continue",
                        CopyValue::String("journal.window.thinking.copy.continue.with.chatgpt"),
                    ),
                    (
                        "plan",
                        CopyValue::String("journal.window.thinking.copy.using.chatgpt.plan"),
                    ),
                    (
                        "manage_usage",
                        CopyValue::String("journal.window.thinking.copy.manage.usage"),
                    ),
                    (
                        "manage_usage_link",
                        CopyValue::String("journal.window.thinking.copy.manage.usage.variant2"),
                    ),
                ]),
            ),
            (
                "openai_card",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.openai.endpoint.option",
                    &[("endpoint_button", "{endpoint}")],
                ),
            ),
            // One thinking setting for the whole "your own model" lane. The key
            // lanes ask the provider to think. On the owner's own endpoint, off
            // asks the model not to think and on sends nothing and leaves room,
            // and its words say so.
            (
                "tuning",
                CopyValue::Object(&[
                    (
                        "label",
                        CopyValue::String("journal.window.thinking.copy.tuning"),
                    ),
                    (
                        "default",
                        CopyValue::String("journal.window.thinking.copy.default"),
                    ),
                    (
                        "choice",
                        CopyValue::LegacyTemplate(
                            "journal.window.thinking.budget.choice.tokens",
                            &[("token_budget", "{budget}")],
                        ),
                    ),
                    (
                        "saved",
                        CopyValue::String("journal.window.thinking.copy.saved"),
                    ),
                    (
                        "key",
                        CopyValue::Object(&[
                            (
                                "state",
                                CopyValue::LegacyTemplate(
                                    "journal.window.thinking.budget.key.current",
                                    &[("token_budget", "{budget}")],
                                ),
                            ),
                            (
                                "check",
                                CopyValue::String(
                                    "journal.window.thinking.copy.let.the.model.think.longer.before",
                                ),
                            ),
                            (
                                "budget",
                                CopyValue::String("journal.window.thinking.copy.thinking.budget"),
                            ),
                            (
                                "note",
                                CopyValue::String(
                                    "journal.window.thinking.copy.thinking.may.improve.results.and.it.variant2",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "endpoint",
                        CopyValue::Object(&[
                            (
                                "state",
                                CopyValue::LegacyTemplate(
                                    "journal.window.thinking.budget.endpoint.current",
                                    &[("token_budget", "{budget}")],
                                ),
                            ),
                            (
                                "check",
                                CopyValue::String(
                                    "journal.window.thinking.copy.leave.room.for.the.model.to",
                                ),
                            ),
                            (
                                "budget",
                                CopyValue::String("journal.window.thinking.copy.thinking.room"),
                            ),
                            (
                                "note",
                                CopyValue::String(
                                    "journal.window.thinking.copy.while.this.is.off.processing.asks",
                                ),
                            ),
                        ]),
                    ),
                ]),
            ),
            (
                "get_key",
                CopyValue::String("journal.window.thinking.copy.get.a.key"),
            ),
            (
                "paste_title",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.paste.provider",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "key_hint",
                CopyValue::String("journal.window.thinking.copy.the.key.stays.in.your.journal"),
            ),
            (
                "terms",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.provider.processing.terms.legacy",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "terms_link",
                CopyValue::String("journal.window.thinking.copy.terms"),
            ),
            (
                "endpoint_heading",
                CopyValue::String(
                    "journal.window.thinking.copy.point.processing.at.your.own.endpoint",
                ),
            ),
            (
                "endpoint_sub",
                CopyValue::String(
                    "journal.window.thinking.copy.any.openai.compatible.url.your.server",
                ),
            ),
            (
                "endpoint_honesty",
                CopyValue::String(
                    "journal.window.thinking.copy.processing.checks.the.endpoint.works.before",
                ),
            ),
            (
                "endpoint_image_checking",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.endpoint.model.image.checking",
                    &[("model_name", "{model}")],
                ),
            ),
            (
                "endpoint_image_not_read",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.endpoint.model.image.not.read",
                    &[("model_name", "{model}")],
                ),
            ),
            (
                "endpoint_schema_not_followed",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.endpoint.model.schema.not.followed",
                    &[("model_name", "{model}")],
                ),
            ),
            (
                "endpoint_image_unchecked",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.endpoint.model.image.unchecked.legacy",
                    &[("model_name", "{model}"), ("failure_reason", "{reason}")],
                ),
            ),
            (
                "endpoint_reason_unreachable",
                CopyValue::String("journal.window.thinking.copy.couldn.t.reach.the.endpoint"),
            ),
            (
                "endpoint_reason_timeout",
                CopyValue::String("journal.window.thinking.copy.it.didn.t.answer.in.time"),
            ),
            (
                "endpoint_reason_busy",
                CopyValue::String("journal.window.thinking.copy.your.journal.was.busy.with.other"),
            ),
            (
                "endpoint_reason_key",
                CopyValue::String("journal.window.thinking.copy.it.didn.t.accept.the.key"),
            ),
            (
                "endpoint_reason_rejected",
                CopyValue::String("journal.window.thinking.copy.it.rejected.the.request"),
            ),
            (
                "endpoint_reason_unusable",
                CopyValue::String("journal.window.thinking.copy.it.sent.back.nothing.usable"),
            ),
            (
                "endpoint_reason_other",
                CopyValue::String("journal.window.thinking.copy.something.went.wrong"),
            ),
            (
                "paste_cta",
                CopyValue::String("journal.window.thinking.copy.check.this.key"),
            ),
            (
                "checking_key",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.checking.provider",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "key_ok_strip",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.verified.provider",
                    &[("provider_name", "{provider}"), ("checked_at", "{when}")],
                ),
            ),
            (
                "check_again",
                CopyValue::String("journal.window.thinking.copy.check.again"),
            ),
            (
                "use_different_key",
                CopyValue::String("journal.window.thinking.copy.use.a.different.key"),
            ),
            (
                "key_failed",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.failed.legacy",
                    &[
                        ("failure_reason", "{reason}"),
                        ("provider_name", "{provider}"),
                    ],
                ),
            ),
            (
                "reason_rejected",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.reason.rejected",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "reason_quota",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.reason.quota",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "reason_network",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.reason.network",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "reason_unknown",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.reason.unknown",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "model_heading",
                CopyValue::String("journal.window.thinking.copy.which.model.should.your.key.use"),
            ),
            (
                "model_sub",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.model.provider.instructions",
                    &[("provider_name", "{provider}")],
                ),
            ),
            (
                "model_label",
                CopyValue::String("journal.window.thinking.copy.model.id"),
            ),
            (
                "custom_not_found",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.model.absent",
                    &[("provider_name", "{provider}"), ("model_name", "{model}")],
                ),
            ),
            (
                "model_save",
                CopyValue::String("journal.window.thinking.copy.use.this.model"),
            ),
            (
                "model_save_restore",
                CopyValue::String("journal.window.thinking.copy.remember.this.model"),
            ),
            (
                "model_saving",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.model.checking",
                    &[("model_name", "{model}")],
                ),
            ),
            (
                "model_saved_restore",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.model.restored",
                    &[("model_name", "{label}")],
                ),
            ),
            (
                "probe_failed_save",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.keys.model.unresponsive.legacy",
                    &[("model_name", "{model}"), ("failure_reason", "{reason}")],
                ),
            ),
        ]),
    ),
    (
        "lane_switch",
        CopyValue::Object(&[
            (
                "heading",
                CopyValue::String("journal.window.thinking.copy.switch.how.processing.runs"),
            ),
            (
                "current_label",
                CopyValue::String("journal.window.thinking.copy.now"),
            ),
            (
                "target_label",
                CopyValue::String("journal.window.thinking.copy.switch.to"),
            ),
            (
                "confirm",
                CopyValue::String("journal.window.thinking.copy.switch"),
            ),
            (
                "cancel",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.lane.switch.keep.current",
                    &[("current_setup", "{current}")],
                ),
            ),
            (
                "to_local_note",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.lane.switch.local",
                    &[("current_setup", "{current}")],
                ),
            ),
            (
                "to_byo_note",
                CopyValue::LegacyTemplate(
                    "journal.window.thinking.lane.switch.byo",
                    &[("saved_setup", "{setup}")],
                ),
            ),
            (
                "setup_key",
                CopyValue::String("journal.window.thinking.copy.a.saved.key"),
            ),
            (
                "setup_endpoint",
                CopyValue::String("journal.window.thinking.copy.your.endpoint"),
            ),
        ]),
    ),
    (
        "local_install",
        CopyValue::Object(&[
            (
                "phases",
                CopyValue::Object(&[
                    (
                        "resolving",
                        CopyValue::String("journal.window.thinking.copy.resolving"),
                    ),
                    (
                        "downloading",
                        CopyValue::String("journal.window.thinking.copy.downloading"),
                    ),
                    (
                        "verifying",
                        CopyValue::String("journal.window.thinking.copy.verifying"),
                    ),
                    (
                        "installing",
                        CopyValue::String("journal.window.thinking.copy.installing"),
                    ),
                ]),
            ),
            (
                "pill_inflight",
                CopyValue::String("journal.window.thinking.copy.setting.up"),
            ),
            (
                "pill_failed",
                CopyValue::String("journal.window.thinking.copy.couldn.t.finish"),
            ),
            (
                "failed_verdict",
                CopyValue::String("journal.window.thinking.copy.local.setup.didn.t.finish"),
            ),
            (
                "failed_reason",
                CopyValue::String(
                    "journal.window.thinking.copy.local.setup.stopped.before.it.finished",
                ),
            ),
            (
                "cancelled_verdict",
                CopyValue::String("journal.window.thinking.copy.installation.cancelled"),
            ),
            (
                "download_failed",
                CopyValue::String("journal.window.thinking.copy.couldn.t.finish.the.download.for"),
            ),
            (
                "interrupted_verdict",
                CopyValue::String("journal.window.thinking.copy.installation.stopped"),
            ),
            (
                "stopped_reason",
                CopyValue::String("journal.window.thinking.copy.you.can.try.setup.again.when"),
            ),
            (
                "gpu_memory_insufficient",
                CopyValue::String("journal.window.thinking.copy.the.selected.gpu.has.less.than"),
            ),
            (
                "gpu_memory_unknown",
                CopyValue::String("journal.window.thinking.copy.couldn.t.check.gpu.memory.on"),
            ),
            (
                "gpu_unavailable",
                CopyValue::String("journal.window.thinking.copy.couldn.t.find.a.supported.gpu"),
            ),
            (
                "integrity_failed",
                CopyValue::String("journal.window.thinking.copy.the.downloaded.files.couldn.t.be"),
            ),
            (
                "package_failed",
                CopyValue::String(
                    "journal.window.thinking.copy.some.files.needed.for.local.thinking",
                ),
            ),
            (
                "package_invalid",
                CopyValue::String(
                    "journal.window.thinking.copy.some.files.needed.for.local.thinking.variant2",
                ),
            ),
            (
                "unsupported_failed",
                CopyValue::String("journal.window.thinking.copy.this.local.model.isn.t.supported"),
            ),
            (
                "disk_failed",
                CopyValue::String("journal.window.thinking.copy.there.isn.t.enough.free.disk"),
            ),
            (
                "memory_failed",
                CopyValue::String("journal.window.thinking.copy.your.journal.s.computer.doesn.t"),
            ),
            (
                "unidentified_failed",
                CopyValue::String("journal.window.thinking.copy.couldn.t.tell.why.local.setup"),
            ),
            (
                "retry",
                CopyValue::String("journal.window.thinking.copy.try.setup.again"),
            ),
            (
                "install",
                CopyValue::String("journal.window.thinking.copy.install.local.model"),
            ),
            (
                "cancel",
                CopyValue::String("journal.window.thinking.copy.cancel"),
            ),
            (
                "notice_inflight",
                CopyValue::String("journal.window.thinking.copy.local.thinking.will.stay.in.your"),
            ),
        ]),
    ),
    (
        "local_recovery",
        CopyValue::Object(&[
            (
                "retry",
                CopyValue::String("journal.window.thinking.copy.try.starting.local.again"),
            ),
            (
                "states",
                CopyValue::Object(&[
                    (
                        "checking",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.checking"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.checking.local.setup",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.checking.the.selected.model.and.your",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "starting",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.starting"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.starting.local.thinking",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.checking.the.model.before.using.it",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "recovering",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.recovering"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.is.recovering",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.starting.it.again.shortly",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "retrying",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.retrying"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.trying.local.thinking.again",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.starting.a.new.recovery.attempt",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "waiting",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.waiting"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.is.waiting",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.it.will.start.when.your.journal",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "unsupported",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.unavailable"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.your.journal.s.computer.can.t",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.needs.supported.hardware.where",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_memory_insufficient",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.unavailable"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.the.selected.gpu.needs.more.memory",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.needs.a.compatible.gpu",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_probe_failed",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.couldn.t.check"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.couldn.t.check.gpu.memory",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.couldn.t.read.gpu.memory.on",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "gpu_allocation_failed",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.needs.attention"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.ran.out.of.memory",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.the.gpu.didn.t.have.enough",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "local_probe_failed",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.couldn.t.check"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.couldn.t.check.local.setup",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.try.checking.again.in.a.moment",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "failed",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.needs.attention"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.couldn.t.start",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.the.local.model.stopped.before.it",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "changing",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.changing"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.finishing.the.local.change",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.waiting.for.current.local.work.to",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "cleanup_failed",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.needs.attention"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.couldn.t.finish.changing.state",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.couldn.t.confirm.that.local.processing",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "corrupt",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.can.t.verify"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.status.can.t.be.confirmed",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.check.again.before.changing.local.setup",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "unavailable",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.can.t.verify"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.status.can.t.be.read",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.correct.the.local.file.access.problem",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "stale",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.can.t.verify"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.status.couldn.t.be.refreshed",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.check.again.before.changing.local.setup",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "ready",
                        CopyValue::Object(&[
                            ("pill", CopyValue::String("journal.window.thinking.copy.on")),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.is.ready",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.a.model.is.running.on.your",
                                ),
                            ),
                        ]),
                    ),
                    (
                        "ready_proof_unavailable",
                        CopyValue::Object(&[
                            (
                                "pill",
                                CopyValue::String("journal.window.thinking.copy.on.needs.a.check"),
                            ),
                            (
                                "verdict",
                                CopyValue::String(
                                    "journal.window.thinking.copy.local.thinking.is.still.running",
                                ),
                            ),
                            (
                                "reason",
                                CopyValue::String(
                                    "journal.window.thinking.copy.couldn.t.refresh.the.local.status",
                                ),
                            ),
                        ]),
                    ),
                ]),
            ),
        ]),
    ),
]);

pub fn thinking_copy_payload() -> Value {
    localized_thinking_copy_payload(ENGLISH)
}

pub fn localized_thinking_copy_payload(locale: &str) -> Value {
    THINKING_COPY_PAYLOAD.localized(locale)
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
            let assets = solstone_core_assets::resolve(
                "local-model-mlx",
                Some(solstone_core_assets::Platform::MacosArm64),
                None,
            );
            (!assets.is_empty()).then(|| assets.iter().map(|row| row.size_bytes).sum())
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
    localized_setup_copy(facts, ENGLISH)
}

pub fn localized_setup_copy(facts: &SetupCatalogFacts, locale: &str) -> serde_json::Value {
    // A phone can be the journal itself; everywhere else it is a computer.
    let on_phone = matches!(facts.platform, "ios" | "android");
    let choices = lanes(locale)
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
        "heading": text(locale, "journal.setup.heading"),
        "welcome": {
            "title": text(locale, "journal.setup.welcome.title"),
            "body": text(locale, "journal.setup.welcome.body"),
            "how_it_fits": [
                text(locale, "journal.setup.welcome.title"),
                text(locale, "journal.setup.welcome.body"),
                text(locale, "journal.window.network.brandlock.line")
            ]
        },
        "fits": {
            "title": text(locale, "journal.setup.fits.title"),
            "journal_term": text(locale, "journal.setup.heading"),
            "journal": if on_phone {
                text(locale, "journal.setup.fits.journal.phone")
            } else {
                text(locale, "journal.setup.fits.journal.computer")
            },
            "app_term": text(locale, "journal.setup.fits.app.term"),
            "app": text(locale, "journal.setup.fits.app"),
            "thinking_term": text(locale, "journal.setup.fits.thinking.term"),
            "thinking": text(locale, "journal.setup.fits.thinking")
        },
        "login": {
            "label": text(locale, "journal.setup.login.label"),
            "help": text(locale, "journal.setup.login.help")
        },
        "folder": {
            "title": text(locale, "journal.setup.folder.title"),
            "help": text(locale, "journal.setup.folder.help"),
            "change": text(locale, "journal.setup.folder.change"),
            "continue": text(locale, "journal.setup.folder.continue")
        },
        "mark": {
            "title": text(locale, "journal.setup.mark.title"),
            "help": text(locale, "journal.setup.mark.help"),
            "error": text(locale, "journal.setup.mark.error"),
            "permanence": text(locale, "!journal.setup.mark.permanence"),
            "try_another": text(locale, "journal.setup.mark.try.another"),
            "continue": text(locale, "journal.setup.folder.continue")
        },
        "thinking": {
            "title": text(locale, "journal.setup.fits.thinking.term"),
            "choices": choices,
            "default_line": solstone_core_window_catalog::strings(locale, "journal.setup.thinking.default.line", [("size", "{size}")]),
            "size_unknown": text(locale, "journal.setup.thinking.size.unknown"),
            "choose_later": text(locale, "journal.setup.thinking.choose.later"),
            "change": text(locale, "journal.setup.thinking.change"),
            "later": text(locale, "journal.setup.thinking.later"),
            "continue": text(locale, "journal.setup.folder.continue"),
            "check_unknown": text(locale, "journal.setup.thinking.check.unknown")
        },
        "downloads": {
            "speech": text(locale, "journal.setup.downloads.speech"),
            "thinking": text(locale, "journal.setup.downloads.thinking"),
            "progress": text(locale, "journal.setup.downloads.progress"),
            "cancel": text(locale, "journal.setup.downloads.cancel"),
            "retry": text(locale, "journal.setup.downloads.retry"),
            "error": text(locale, "journal.setup.downloads.error"),
            "idle": text(locale, "journal.setup.downloads.idle"),
            "complete": text(locale, "journal.setup.downloads.complete"),
            "cancelled": text(locale, "journal.setup.downloads.cancelled")
        },
        "footer": if on_phone {
            text(locale, "journal.setup.footer.phone")
        } else {
            text(locale, "journal.setup.footer.computer")
        },
        "help": {
            "text": text(locale, "journal.setup.help.text"),
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
        for (choice, lane) in choices.iter().zip(lanes(ENGLISH)) {
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
            filename: None,
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
