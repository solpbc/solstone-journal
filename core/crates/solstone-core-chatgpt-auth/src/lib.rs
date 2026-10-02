// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! ChatGPT authentication flow, token management, and model listing for solstone.

pub mod attempt;
pub mod authorize;
pub mod callback;
pub mod credential;
pub mod exchange;
pub mod models;
pub mod overrides;
pub mod refresh;
pub mod revoke;
pub mod store;
pub mod transport;

#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;

#[cfg(all(test, feature = "full-tests"))]
mod full_tests;

#[cfg(test)]
mod seam_tests;

#[cfg(all(test, feature = "full-tests"))]
mod full_seam_tests;

pub use attempt::{
    ChatGptStatus, SignInAttempt, SignInResult, SignOutResult, begin_sign_in,
    finish_active_sign_in, finish_sign_in, get_active_attempt, get_active_attempt_status,
    get_status, registration, set_active_attempt, sign_out,
};
pub use authorize::{
    API_RESOURCE, AuthorizeParams, DEFAULT_SCOPE, DYNAMIC_CLIENT_ID, build_authorize_params,
};
pub use callback::{
    CallbackError, CallbackRequest, FAILURE_BODY, SUCCESS_BODY, bind_loopback_listener,
    parse_pasted_callback, receive_browser_callback, respond_to_callback_stream,
};
pub use credential::{ChatGptCredential, ClosedOutcome, CredentialError};
pub use exchange::{ValidatedExchange, exchange_code_for_tokens, parse_and_validate_id_token};
pub use models::{ChatGptModel, MODELS_TIMEOUT, fetch_models};
pub use overrides::{
    API_BASE_URL_OVERRIDE_ENV, AUTH_BASE_URL_OVERRIDE_ENV, api_base_url, auth_base_url,
    is_loopback_base_url,
};
pub use refresh::{ChatGptAuthManager, EXPIRATION_MARGIN_SECS, REFRESH_TIMEOUT};
pub use revoke::{REVOKE_TIMEOUT, revoke_refresh_token};
pub use store::{
    CREDENTIAL_FILE, ChatGptSignInDoc, DEFAULT_LOCK_TIMEOUT, DIRECT_USE_SCOPE, LoadResult,
    Registration, Tokens, acquire_credential_lock, credential_path, load_credential_file, now_secs,
    rotate_unreadable_credential_file, save_credential_file,
};
#[cfg(test)]
pub use store::{with_test_access_token_lock_timeout, with_test_now_secs};
pub use transport::{ChatGptTransport, HttpResponse, TransportError, UreqTransport};

#[cfg(any(test, feature = "test-hooks"))]
pub use overrides::{set_test_api_base_url_override, set_test_auth_base_url_override};

#[cfg(any(test, feature = "test-hooks"))]
pub use test_support::{
    REFRESH_MARKERS_ENV, record_refresh_marker_if_configured, set_fail_next_grant_write,
    take_fail_next_grant_write, write_test_credential,
};
