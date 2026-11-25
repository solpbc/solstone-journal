// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::{
    ConsentSelection, ConsentStage, OAuthRuntime, RegisteredClient, checked, consent_csp,
    html_escape, html_status_with_csp, parse_redirect_uri, redirect_host_label, redirect_origin,
};
use crate::http1::HttpResponse;
use solstone_core_window_catalog::{self as catalog, Value, html, text};

// This marker only occurs in the renderer’s input. Insert escaped owner text
// or the escaped callback host in trusted emphasis markup after escaping the
// surrounding text. Neither the client nor a translation supplies HTML.
const INLINE: &str = "\u{fdd0}";

fn inline_html(locale: &str, id: &str, name: &str, escaped: &str) -> String {
    let words = catalog::strings(locale, id, [(name, INLINE)]);
    insert_inline(&words, escaped)
}

fn insert_inline(words: &str, escaped: &str) -> String {
    words
        .split(INLINE)
        .map(catalog::escape_html)
        .collect::<Vec<_>>()
        .join(escaped)
}

pub(super) fn consent_page(
    locale: &str,
    client: &RegisteredClient,
    redirect_uri: &str,
    transaction_id: &str,
    oauth: &OAuthRuntime,
    stage: ConsentStage,
    selection: &ConsentSelection,
) -> HttpResponse {
    let parsed = parse_redirect_uri(redirect_uri).ok();
    let host = parsed.as_ref().map(redirect_host_label).unwrap_or_else(|| {
        text(
            locale,
            "journal.window.shared.status.pane.visible.text.unknown",
        )
    });
    let callback_origin = parsed.as_ref().map(redirect_origin);
    let client_label = client
        .client_name
        .as_deref()
        .unwrap_or(client.client_id.as_str());
    let client = html_escape(client_label);
    let return_host = format!("<strong>{}</strong>", html_escape(&host));
    let callback_origin = callback_origin.as_deref();
    let transaction = html_escape(transaction_id);
    let facet_detail = match stage {
        ConsentStage::Code { .. } | ConsentStage::TargetedCode { .. } => {
            html(locale, "journal.window.oauth.scope.facets.after.code")
        }
        ConsentStage::Choose { .. } | ConsentStage::Confirm { .. } => {
            html(locale, "journal.window.oauth.scope.facets.detail")
        }
    };
    let choices = format!(
        r#"<fieldset><label class="check"><input type="radio" name="scope" value="whole_journal" required{whole_checked}> <strong>{scope_whole}</strong><br><span class="muted">{whole_detail}</span></label><label class="check"><input type="radio" name="scope" value="facets" required{facets_checked}> <strong>{scope_facets}</strong><br><span class="muted">{facet_detail}</span></label></fieldset>{facet_list}<fieldset><legend>{kinds}</legend>{kind_choices}</fieldset>"#,
        scope_whole = html(locale, "journal.window.oauth.scope.whole"),
        whole_detail = html(locale, "journal.window.oauth.scope.whole.detail"),
        scope_facets = html(locale, "journal.window.oauth.scope.facets"),
        kinds = html(locale, "journal.window.oauth.scope.kinds"),
        whole_checked = checked(selection.scope == Some("whole_journal")),
        facets_checked = checked(selection.scope == Some("facets")),
        facet_list = match stage {
            ConsentStage::Code { .. } | ConsentStage::TargetedCode { .. } => String::new(),
            ConsentStage::Choose { .. } | ConsentStage::Confirm { .. } => {
                facet_list(locale, oauth, selection)
            }
        },
        kind_choices = solstone_core_format::content::kinds::permission_kinds()
            .map(|kind| {
                let copy = kind.permission_copy.expect("registered permissions have copy");
                let mut row = format!(
                    r#"<label class="check{}"><input type="checkbox" name="category" value="{}"{}> <strong>{}</strong><br><span class="muted">{}</span></label>"#,
                    if kind.option_of().is_some() { " option" } else { "" },
                    kind.stored_word,
                    checked(selection.categories.iter().any(|word| word == kind.stored_word)),
                    html_escape(&copy.render_label(|id| text(locale, id))),
                    html_escape(&copy.render_description(|id| text(locale, id))),
                );
                if kind.stored_word == "places" {
                    use solstone_core_format::content::kinds::PlacesPrecision;
                    let options = [
                        (
                            PlacesPrecision::Full,
                            "journal.window.agents.permission.places.precision.full.label",
                            "journal.window.agents.permission.places.precision.full.description",
                        ),
                        (
                            PlacesPrecision::Approx,
                            "journal.window.agents.permission.places.precision.approx.label",
                            "journal.window.agents.permission.places.precision.approx.description",
                        ),
                        (
                            PlacesPrecision::NamesOnly,
                            "journal.window.agents.permission.places.precision.names.label",
                            "journal.window.agents.permission.places.precision.names.description",
                        ),
                    ]
                    .into_iter()
                    .map(|(tier, label_id, desc_id)| {
                        format!(
                            r#"<label class="check"><input type="radio" name="places_precision" value="{}"{}> <strong>{}</strong><br><span class="muted">{}</span></label>"#,
                            tier.wire_value(),
                            checked(selection.places_precision == tier),
                            html(locale, label_id),
                            html(locale, desc_id),
                        )
                    })
                    .collect::<String>();
                    row.push_str(&format!(
                        r#"<fieldset class="option"><legend class="muted">{}</legend>{options}</fieldset>"#,
                        html(
                            locale,
                            "journal.window.agents.permission.places.precision.heading"
                        )
                    ));
                }
                row
            })
            .collect::<String>(),
    );
    let code_fields = |wrong_code: bool| {
        let wrong = if wrong_code {
            format!(
                r#"<p class="error" role="alert">{}</p>"#,
                html(locale, "journal.window.oauth.wrong.code")
            )
        } else {
            String::new()
        };
        format!(
            r#"<h2>{heading}</h2>{wrong}<label>{label}<input type="text" name="pairing_code" autocomplete="one-time-code" spellcheck="false" required></label><p class="muted">{detail}</p>"#,
            heading = html(locale, "journal.window.oauth.code.heading"),
            label = html(locale, "journal.window.oauth.code.label"),
            detail = html(locale, "journal.window.oauth.code.explanation"),
        )
    };
    let body = match stage {
        ConsentStage::Code {
            wrong_code,
            incomplete,
        } => {
            let code = code_fields(wrong_code);
            let choose = if incomplete {
                format!(
                    r#"<p class="error" role="alert">{}</p>"#,
                    html(locale, "journal.window.oauth.scope.required.before.code")
                )
            } else {
                String::new()
            };
            format!(
                r#"<h1>{heading}</h1><p>{return_to}</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}"><h2>{may_see}</h2>{choose}{choices}{code}<button type="submit">{connect}</button><p class="muted">{cancel}</p></form>"#,
                return_to = inline_html(
                    locale,
                    "journal.window.oauth.connect.return.and.scope",
                    "return_host",
                    &return_host
                ),
                heading = inline_html(
                    locale,
                    "journal.window.oauth.connect.heading",
                    "client_name",
                    &client
                ),
                may_see = inline_html(
                    locale,
                    "journal.window.oauth.scope.heading",
                    "client_name",
                    &client
                ),
                connect = inline_html(
                    locale,
                    "journal.window.oauth.connect.button",
                    "client_name",
                    &client
                ),
                cancel = html(locale, "journal.window.oauth.cancel.before.code"),
            )
        }
        ConsentStage::Choose { incomplete } => {
            let notice = if incomplete {
                format!(
                    r#"<p class="error" role="alert">{}</p>"#,
                    html(locale, "journal.window.oauth.scope.required.after.code")
                )
            } else {
                String::new()
            };
            format!(
                r#"<h1>{choose_heading}</h1><p>{choose_detail}</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}">{notice}{choices}<button type="submit">{connect}</button><p class="muted">{cancel}</p></form>"#,
                choose_heading = inline_html(
                    locale,
                    "journal.window.oauth.scope.choose.heading",
                    "client_name",
                    &client
                ),
                choose_detail = inline_html(
                    locale,
                    "journal.window.oauth.scope.choose.detail",
                    "client_name",
                    &client
                ),
                connect = inline_html(
                    locale,
                    "journal.window.oauth.connect.button",
                    "client_name",
                    &client
                ),
                cancel = html(locale, "journal.window.oauth.cancel.after.code"),
            )
        }
        ConsentStage::TargetedCode { wrong_code } => {
            let code = code_fields(wrong_code);
            format!(
                r#"<h1>{heading}</h1><p>{return_to}</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}"><input type="hidden" name="reconnect" value="1">{code}<button type="submit">{next}</button><p class="muted">{cancel}</p></form>"#,
                return_to = inline_html(
                    locale,
                    "journal.window.oauth.connect.return",
                    "return_host",
                    &return_host
                ),
                heading = inline_html(
                    locale,
                    "journal.window.oauth.connect.heading",
                    "client_name",
                    &client
                ),
                cancel = html(locale, "journal.window.oauth.cancel.before.code"),
                next = html(locale, "journal.window.backup.copy.key.continue"),
            )
        }
        ConsentStage::Confirm {
            name,
            short_key,
            memory_count,
            live_holder,
        } => {
            let agent = format!("{} · {}", html_escape(&name), html_escape(&short_key));
            let effect = if memory_count == 0 {
                let id = if live_holder {
                    "journal.window.oauth.reconnect.body.live"
                } else {
                    "journal.window.oauth.reconnect.body"
                };
                inline_html(locale, id, "agent_name", &agent)
            } else {
                let id = if live_holder {
                    "journal.window.oauth.reconnect.body.live.memories"
                } else {
                    "journal.window.oauth.reconnect.body.memories"
                };
                let count = memory_count.to_string();
                let words = catalog::render(
                    locale,
                    id,
                    &[
                        ("agent_name", Value::String(INLINE)),
                        (
                            "quantity",
                            Value::Cardinal(i64::try_from(memory_count).unwrap_or(i64::MAX)),
                        ),
                        ("memory_count", Value::String(&count)),
                    ],
                );
                insert_inline(&words, &agent)
            };
            let scope_id = if live_holder {
                "journal.window.oauth.reconnect.scope.live"
            } else {
                "journal.window.oauth.reconnect.scope"
            };
            let may_see = format!(
                "<p>{}</p>",
                inline_html(locale, scope_id, "agent_name", &agent)
            );
            format!(
                r#"<h1>{heading}</h1><p>{effect}</p><form method="post" action="/authorize"><input type="hidden" name="transaction_id" value="{transaction}">{may_see}{choices}<button type="submit">{again}</button><p class="muted">{cancel}</p></form>"#,
                heading = inline_html(
                    locale,
                    "journal.window.oauth.reconnect.heading",
                    "agent_name",
                    &agent
                ),
                again = inline_html(
                    locale,
                    "journal.window.oauth.reconnect.button",
                    "agent_name",
                    &agent
                ),
                cancel = html(locale, "journal.window.oauth.cancel.reconnect"),
            )
        }
    };
    html_status_with_csp(200, "OK", &body, consent_csp(callback_origin))
}

/// The journal's facets as choices. Only a requester whose pairing code has
/// verified is ever shown this.
fn facet_list(locale: &str, oauth: &OAuthRuntime, selection: &ConsentSelection) -> String {
    let facets =
        solstone_core_facets::list_declared_facet_names(&oauth.journal_root).unwrap_or_default();
    if facets.is_empty() {
        return format!(
            r#"<p class="muted">{}</p>"#,
            html(locale, "journal.window.oauth.facets.empty")
        );
    }
    let options = facets
        .into_iter()
        .map(|facet| {
            let declaration = solstone_core_facets::read_facet_declaration(&oauth.journal_root, &facet)
                .ok()
                .flatten();
            let title = declaration
                .as_ref()
                .map(|value| value.title.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(&facet);
            let color = declaration
                .as_ref()
                .map(|value| value.color.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or("#76746b");
            let facet_checked = checked(selection.facets.contains(&facet));
            format!("<label class=\"check\"><input type=\"checkbox\" name=\"facet\" value=\"{}\"{facet_checked}> <span style=\"color:{}\">●</span> {}</label>", html_escape(&facet), html_escape(color), html_escape(title))
        })
        .collect::<String>();
    format!(
        "<fieldset><legend>{}</legend>{options}</fieldset>",
        html(locale, "journal.window.agents.visible.text.facets")
    )
}
