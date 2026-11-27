// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native read routes for the Settings Convey surface.

use std::path::PathBuf;

use axum::{
    Extension, Router,
    routing::{delete, get, post, put},
};
use solstone_core_convey_http::{coalesce::ResponseLocale, window_catalog::response_locale};
use solstone_core_journal_io::LockOptions;

mod activities;
mod assets;
mod config;
mod convey;
mod email;
pub use email::needs_attention as email_needs_attention;
mod facets;
mod garmin;
mod granola;
mod http;
mod icons;
mod keys;
mod logs;
mod owner_voice;
mod plaud;
mod processing;
mod request_body;
mod retention;
mod retention_executor;
mod state;
mod storage;
mod sync;
mod sync_authority;
pub use sync::{parse_sync_update, source_sync_plaud_available};
pub use sync_authority::{SourceSyncAuthority, SourceSyncDelegate, SourceSyncResponse};
mod transcribe;
mod trash;
mod vision;

pub fn routes(journal_root: PathBuf) -> Router {
    routes_with_lock_options(journal_root, LockOptions::default())
}

pub fn routes_with_lock_options(journal_root: PathBuf, config_lock_options: LockOptions) -> Router {
    let email_service = email::EmailService::live(journal_root.clone());
    routes_with_email_service(journal_root, config_lock_options, email_service)
}

pub(crate) fn routes_with_email_service(
    journal_root: PathBuf,
    config_lock_options: LockOptions,
    email_service: email::EmailService,
) -> Router {
    let config_get_root = journal_root.clone();
    let config_put_root = journal_root.clone();
    let config_post_root = journal_root.clone();
    let convey_root = journal_root.clone();
    let transcribe_root = journal_root.clone();
    let processing_root = journal_root.clone();
    let keys_root = journal_root.clone();
    let vision_get_root = journal_root.clone();
    let vision_put_root = journal_root.clone();
    let facets_root = journal_root.clone();
    let muted_facets_root = journal_root.clone();
    let facet_get_root = journal_root.clone();
    let facet_put_root = journal_root.clone();
    let facet_delete_root = journal_root.clone();
    let facet_create_root = journal_root.clone();
    let facet_rename_root = journal_root.clone();
    let facet_activities_get_root = journal_root.clone();
    let facet_activities_add_root = journal_root.clone();
    let facet_activities_update_root = journal_root.clone();
    let facet_activities_delete_root = journal_root.clone();
    let logs_root = journal_root.clone();
    let facet_logs_root = journal_root.clone();
    let storage_root = journal_root.clone();
    let owner_voice_root = journal_root.clone();
    let storage_config_root = journal_root.clone();
    let maintenance_time_root = journal_root.clone();
    let removals_root = journal_root.clone();
    let trash_list_root = journal_root.clone();
    let trash_recover_root = journal_root.clone();
    let trash_erase_root = journal_root.clone();
    let trash_empty_root = journal_root.clone();
    let trash_backups_root = journal_root.clone();
    let storage_put_root = journal_root.clone();
    let purge_root = journal_root.clone();
    let prune_logs_root = journal_root.clone();
    let granola_get_root = journal_root.clone();
    let granola_connect_root = journal_root.clone();
    let granola_disconnect_root = journal_root.clone();
    let granola_sync_root = journal_root.clone();
    let garmin_get_root = journal_root.clone();
    let garmin_connect_root = journal_root.clone();
    let garmin_disconnect_root = journal_root.clone();
    let garmin_sync_root = journal_root.clone();
    let garmin_backfill_root = journal_root.clone();
    let garmin_run_preview_root = journal_root.clone();
    let garmin_run_delete_root = journal_root.clone();
    let garmin_body_facet_get_root = journal_root.clone();
    let garmin_body_facet_post_root = journal_root.clone();
    let plaud_get_root = journal_root.clone();
    let plaud_disconnect_root = journal_root.clone();
    let plaud_begin_root = journal_root.clone();
    let plaud_finish_root = journal_root.clone();
    let plaud_sync_root = journal_root.clone();
    let sync_get_root = journal_root.clone();
    let sync_put_root = journal_root;
    Router::new()
        .route("/app/settings/", get(assets::shell))
        .route(
            "/app/settings/api/granola",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                granola::get(response_locale(locale), granola_get_root.clone())
            })
            .post(move |locale: Option<Extension<ResponseLocale>>, body| {
                granola::connect(response_locale(locale), granola_connect_root.clone(), body)
            })
            .delete(move |locale: Option<Extension<ResponseLocale>>| {
                granola::disconnect(response_locale(locale), granola_disconnect_root.clone())
            }),
        )
        .route(
            "/app/settings/api/granola/sync",
            post(move |locale: Option<Extension<ResponseLocale>>| {
                granola::sync_now(response_locale(locale), granola_sync_root.clone())
            }),
        )
        .route(
            "/app/settings/api/garmin",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                garmin::get(response_locale(locale), garmin_get_root.clone())
            })
            .post(move |locale: Option<Extension<ResponseLocale>>, body| {
                garmin::connect(response_locale(locale), garmin_connect_root.clone(), body)
            })
            .delete(move |locale: Option<Extension<ResponseLocale>>| {
                garmin::disconnect(response_locale(locale), garmin_disconnect_root.clone())
            }),
        )
        .route(
            "/app/settings/api/garmin/sync",
            post(move |locale: Option<Extension<ResponseLocale>>| {
                garmin::sync_now(response_locale(locale), garmin_sync_root.clone())
            }),
        )
        .route(
            "/app/settings/api/garmin/backfill",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                garmin::backfill(response_locale(locale), garmin_backfill_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/garmin/runs/{id}/delete",
            get(move |locale: Option<Extension<ResponseLocale>>, path| {
                garmin::run_delete_preview(
                    response_locale(locale),
                    garmin_run_preview_root.clone(),
                    path,
                )
            }),
        )
        .route(
            "/app/settings/api/garmin/runs/{id}",
            delete(
                move |locale: Option<Extension<ResponseLocale>>, path, headers, body| {
                    garmin::run_delete(
                        response_locale(locale),
                        garmin_run_delete_root.clone(),
                        path,
                        headers,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/garmin/body-facet",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                garmin::body_facet_get(response_locale(locale), garmin_body_facet_get_root.clone())
            })
            .post(move |locale: Option<Extension<ResponseLocale>>, body| {
                garmin::body_facet_post(
                    response_locale(locale),
                    garmin_body_facet_post_root.clone(),
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/plaud",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                plaud::get(response_locale(locale), plaud_get_root.clone())
            })
            .delete(move |locale: Option<Extension<ResponseLocale>>| {
                plaud::disconnect(response_locale(locale), plaud_disconnect_root.clone())
            }),
        )
        .route(
            "/app/settings/api/plaud/sign-in",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                plaud::begin_sign_in(response_locale(locale), plaud_begin_root.clone(), body)
            })
            .delete(plaud::cancel_sign_in),
        )
        .route(
            "/app/settings/api/plaud/sign-in/{attempt_id}",
            get(move |locale: Option<Extension<ResponseLocale>>, path| {
                plaud::sign_in_status(response_locale(locale), path)
            }),
        )
        .route(
            "/app/settings/api/plaud/sign-in/{attempt_id}/finish",
            post(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    plaud::finish_sign_in(
                        response_locale(locale),
                        plaud_finish_root.clone(),
                        path,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/plaud/sync",
            post(
                move |locale: Option<Extension<ResponseLocale>>,
                      delegate: Option<axum::Extension<SourceSyncDelegate>>| {
                    plaud::sync_now(response_locale(locale), plaud_sync_root.clone(), delegate)
                },
            ),
        )
        .route("/app/settings/workspace", get(assets::workspace))
        .route("/app/settings/static/settings.js", get(assets::settings_js))
        .route("/app/settings/facets/{slug}", get(assets::shell))
        .route(
            "/app/settings/api/state",
            get(|locale: Option<Extension<ResponseLocale>>| state::get(response_locale(locale))),
        )
        .route(
            "/app/settings/api/config",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                config::get(response_locale(locale), config_get_root.clone())
            })
            .put(move |locale: Option<Extension<ResponseLocale>>, body| {
                config::update(
                    response_locale(locale),
                    config_put_root.clone(),
                    config_lock_options,
                    body,
                )
            })
            .post(move |locale: Option<Extension<ResponseLocale>>, body| {
                config::update(
                    response_locale(locale),
                    config_post_root.clone(),
                    config_lock_options,
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/convey/status",
            get(move || convey::status(convey_root.clone())),
        )
        .route(
            "/app/settings/api/transcribe",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                transcribe::get(response_locale(locale), transcribe_root.clone())
            }),
        )
        .route(
            "/app/settings/api/transcribe/owner-voice",
            get(move || owner_voice::get(owner_voice_root.clone())),
        )
        .route(
            "/app/settings/api/processing",
            get(move || processing::get(processing_root.clone())),
        )
        .route(
            "/app/settings/api/validate-keys",
            get(keys::get).post(move |locale: Option<Extension<ResponseLocale>>, body| {
                keys::post(
                    response_locale(locale),
                    keys_root.clone(),
                    config_lock_options,
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/vision",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                vision::get(response_locale(locale), vision_get_root.clone())
            })
            .put(move |locale: Option<Extension<ResponseLocale>>, body| {
                vision::update(
                    response_locale(locale),
                    vision_put_root.clone(),
                    config_lock_options,
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/facets",
            get(move |query| facets::list(facets_root.clone(), query)),
        )
        .route(
            "/app/settings/api/facets/muted",
            get(move || facets::muted(muted_facets_root.clone())),
        )
        .route(
            "/app/settings/api/facet/{facet_name}",
            get(move |locale: Option<Extension<ResponseLocale>>, path| {
                facets::get_one(response_locale(locale), facet_get_root.clone(), path)
            })
            .put(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    facets::update(response_locale(locale), facet_put_root.clone(), path, body)
                },
            )
            .delete(
                move |locale: Option<Extension<ResponseLocale>>, path, headers, body| {
                    facets::delete(
                        response_locale(locale),
                        facet_delete_root.clone(),
                        path,
                        headers,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/facet",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                facets::create(response_locale(locale), facet_create_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/rename",
            post(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    facets::rename(
                        response_locale(locale),
                        facet_rename_root.clone(),
                        path,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/activities/defaults",
            get(|locale: Option<Extension<ResponseLocale>>| {
                activities::defaults(response_locale(locale))
            }),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/activities",
            get(move |locale: Option<Extension<ResponseLocale>>, path| {
                activities::for_facet(
                    response_locale(locale),
                    facet_activities_get_root.clone(),
                    path,
                )
            })
            .post(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    activities::add(
                        response_locale(locale),
                        facet_activities_add_root.clone(),
                        path,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/activities/{activity_id}",
            put(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    activities::update(
                        response_locale(locale),
                        facet_activities_update_root.clone(),
                        path,
                        body,
                    )
                },
            )
            .delete(
                move |locale: Option<Extension<ResponseLocale>>, path, body| {
                    activities::remove(
                        response_locale(locale),
                        facet_activities_delete_root.clone(),
                        path,
                        body,
                    )
                },
            ),
        )
        .route(
            "/app/settings/api/logs",
            get(move |query| logs::journal(logs_root.clone(), query)),
        )
        .route(
            "/app/settings/api/facet/{facet_name}/logs",
            get(move |path, query| logs::facet(facet_logs_root.clone(), path, query)),
        )
        .route(
            "/app/settings/api/storage/removals",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                storage::removals(response_locale(locale), removals_root.clone())
            }),
        )
        .route(
            "/app/settings/api/trash",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                trash::list(response_locale(locale), trash_list_root.clone())
            }),
        )
        .route(
            "/app/settings/api/trash/recover",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                trash::recover(response_locale(locale), trash_recover_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/trash/erase",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                trash::erase(response_locale(locale), trash_erase_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/trash/empty",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                trash::empty(response_locale(locale), trash_empty_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/trash/backups",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                trash::backups(response_locale(locale), trash_backups_root.clone())
            }),
        )
        .route(
            "/app/settings/api/storage/maintenance-time",
            axum::routing::put(move |locale: Option<Extension<ResponseLocale>>, body| {
                storage::set_maintenance_time(
                    response_locale(locale),
                    maintenance_time_root.clone(),
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/storage/config",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                storage::get_config(response_locale(locale), storage_config_root.clone())
            }),
        )
        .route(
            "/app/settings/api/storage",
            get(move |locale: Option<Extension<ResponseLocale>>| {
                storage::get(response_locale(locale), storage_root.clone())
            })
            .put(move |locale: Option<Extension<ResponseLocale>>, body| {
                retention::update(
                    response_locale(locale),
                    storage_put_root.clone(),
                    config_lock_options,
                    body,
                )
            }),
        )
        .route(
            "/app/settings/api/storage/list",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                retention::purge(response_locale(locale), purge_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/storage/prune-logs",
            post(move |locale: Option<Extension<ResponseLocale>>, body| {
                retention::prune_logs(response_locale(locale), prune_logs_root.clone(), body)
            }),
        )
        .route(
            "/app/settings/api/sync",
            get(
                move |delegate: Option<axum::Extension<SourceSyncDelegate>>| {
                    sync::get_routed(sync_get_root.clone(), delegate)
                },
            )
            .put(
                move |locale: Option<Extension<ResponseLocale>>,
                      delegate: Option<axum::Extension<SourceSyncDelegate>>,
                      body| {
                    sync::update_routed(
                        response_locale(locale),
                        sync_put_root.clone(),
                        delegate,
                        body,
                    )
                },
            ),
        )
        .route("/app/settings/api/icons", get(icons::search))
        .route("/app/settings/api/emojis", get(icons::search_emojis))
        .merge(email::routes_with_service(email_service))
}

#[cfg(test)]
mod corpus;
#[cfg(test)]
mod mutations;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod router_contracts;
#[cfg(test)]
mod settings_corpus_divergence;
#[cfg(test)]
mod test_support;
