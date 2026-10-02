// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Serialization of concurrent credential changes: refresh against sign-out, and sign-in
//! attempts against each other and against a forget.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use solstone_core_journal_io::LockOptions;

use crate::attempt::{get_status, sign_out};
use crate::authorize::DYNAMIC_CLIENT_ID;
use crate::credential::{ChatGptCredential, ClosedOutcome};
use crate::refresh::ChatGptAuthManager;
use crate::seam_tests::{
    FakeOpenAi, Held, TEST_CLIENT, TEST_SUBJECT, begin_attempt_elsewhere, finish_pasted,
    journal_dir, read_doc, write_signed_in,
};
use crate::store::acquire_credential_lock;
use crate::test_support::write_test_credential;

const WAIT: Duration = Duration::from_secs(30);

fn lock_is_free(journal: &std::path::Path) -> bool {
    acquire_credential_lock(
        journal,
        LockOptions {
            timeout: Duration::ZERO,
            ..Default::default()
        },
    )
    .is_ok()
}

#[test]
fn sign_out_during_a_held_refresh_leaves_the_journal_signed_out() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    write_signed_in(&journal, "tok-a", "rt-a", 0);
    let fake = FakeOpenAi::new(&journal);
    let (entered, release) = fake.hold_next(Held::Refresh);

    let refresher = {
        let journal = journal.clone();
        let fake = fake.clone();
        thread::spawn(move || ChatGptAuthManager::new(journal, fake).access_token())
    };
    entered.recv_timeout(WAIT).expect("refresh grant reached");
    assert!(!lock_is_free(&journal), "a refresh holds the lock");

    let signer = {
        let journal = journal.clone();
        let fake = fake.clone();
        thread::spawn(move || sign_out(&journal, fake.as_ref(), false))
    };
    release.send(()).expect("grant released");

    assert_eq!(
        refresher.join().expect("refresh thread").as_deref(),
        Ok("tok-refreshed-1")
    );
    assert!(
        signer
            .join()
            .expect("sign-out thread")
            .expect("sign out")
            .revoked
    );

    let doc = read_doc(&journal);
    assert!(doc.tokens.is_none());
    assert_eq!(doc.sign_in_id, None);
    assert!(!get_status(&journal).expect("status").signed_in);
    let revokes = fake.revokes();
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].token, "rt-refreshed-1");
    assert!(!revokes[0].file_held_tokens);
}

#[test]
fn concurrent_first_registrations_save_one_client_paired_with_its_tokens() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    let fake = FakeOpenAi::new(&journal);
    let first = begin_attempt_elsewhere(&journal);
    let second = begin_attempt_elsewhere(&journal);
    assert_eq!(first.client_id, DYNAMIC_CLIENT_ID);
    assert_eq!(second.client_id, DYNAMIC_CLIENT_ID);

    let barrier = Arc::new(Barrier::new(2));
    let handles: Vec<_> = [(first, "oaiapp_a", "ac_a"), (second, "oaiapp_b", "ac_b")]
        .into_iter()
        .map(|(attempt, client, code)| {
            let journal = journal.clone();
            let fake = fake.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                let outcome =
                    finish_pasted(&journal, &fake, &attempt, code, Some(client), TEST_SUBJECT);
                (client, code, outcome)
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().expect("sign-in thread"))
        .collect();

    let winners: Vec<_> = results
        .iter()
        .filter(|(_, _, outcome)| *outcome == Ok(ClosedOutcome::SignedIn))
        .collect();
    let losers: Vec<_> = results
        .iter()
        .filter(|(_, _, outcome)| *outcome == Err(ClosedOutcome::Superseded))
        .collect();
    assert_eq!(winners.len(), 1, "{results:?}");
    assert_eq!(losers.len(), 1, "{results:?}");
    let (client, code, _) = winners[0];

    let doc = read_doc(&journal);
    assert_eq!(
        doc.registration.expect("registration saved").client_id,
        *client
    );
    let tokens = doc.tokens.expect("tokens saved");
    assert_eq!(tokens.access_token, format!("tok-{client}-{code}"));
    assert_eq!(tokens.refresh_token, format!("rt-{client}-{code}"));
    let exchanges = fake.exchanges();
    assert_eq!(exchanges.len(), 1);
    assert_eq!(exchanges[0]["client_id"], *client);
    assert!(fake.revokes().is_empty());
}

#[test]
fn an_attempt_begun_before_another_finished_is_superseded_at_registration() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    let fake = FakeOpenAi::new(&journal);
    let first = begin_attempt_elsewhere(&journal);
    let second = begin_attempt_elsewhere(&journal);

    assert_eq!(
        finish_pasted(
            &journal,
            &fake,
            &first,
            "ac_a",
            Some("oaiapp_a"),
            TEST_SUBJECT
        ),
        Ok(ClosedOutcome::SignedIn)
    );
    assert_eq!(
        finish_pasted(
            &journal,
            &fake,
            &second,
            "ac_b",
            Some("oaiapp_b"),
            TEST_SUBJECT
        ),
        Err(ClosedOutcome::Superseded)
    );

    assert_eq!(
        fake.exchanges().len(),
        1,
        "the later attempt never exchanges"
    );
    let doc = read_doc(&journal);
    assert_eq!(
        doc.registration.expect("registration kept").client_id,
        "oaiapp_a"
    );
    let tokens = doc.tokens.expect("tokens kept");
    assert_eq!(tokens.access_token, "tok-oaiapp_a-ac_a");
    assert_eq!(tokens.refresh_token, "rt-oaiapp_a-ac_a");
    assert!(fake.revokes().is_empty());
}

#[test]
fn forget_before_the_callback_supersedes_the_attempt() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    write_test_credential(&journal, false).expect("registered file");
    let fake = FakeOpenAi::new(&journal);
    let attempt = begin_attempt_elsewhere(&journal);
    assert_eq!(attempt.client_id, TEST_CLIENT);

    assert!(
        sign_out(&journal, fake.as_ref(), true)
            .expect("forget")
            .revoked
    );
    assert_eq!(
        finish_pasted(&journal, &fake, &attempt, "ac_a", None, TEST_SUBJECT),
        Err(ClosedOutcome::Superseded)
    );

    let doc = read_doc(&journal);
    assert!(doc.registration.is_none());
    assert!(doc.tokens.is_none());
    assert!(fake.exchanges().is_empty());
    assert!(fake.revokes().is_empty());
}

#[test]
fn forget_during_the_exchange_discards_the_result_and_revokes_it_outside_the_lock() {
    let dir = journal_dir();
    let journal = dir.path().to_path_buf();
    write_test_credential(&journal, false).expect("registered file");
    let fake = FakeOpenAi::new(&journal);
    let attempt = begin_attempt_elsewhere(&journal);
    let (entered, release) = fake.hold_next(Held::Exchange);

    let finisher = {
        let journal = journal.clone();
        let fake = fake.clone();
        thread::spawn(move || finish_pasted(&journal, &fake, &attempt, "ac_a", None, TEST_SUBJECT))
    };
    entered.recv_timeout(WAIT).expect("exchange reached");
    assert!(lock_is_free(&journal), "the exchange runs outside the lock");
    assert!(
        sign_out(&journal, fake.as_ref(), true)
            .expect("forget")
            .revoked
    );
    release.send(()).expect("exchange released");

    assert_eq!(
        finisher.join().expect("sign-in thread"),
        Err(ClosedOutcome::Superseded)
    );
    let doc = read_doc(&journal);
    assert!(doc.registration.is_none());
    assert!(doc.tokens.is_none());
    let revokes = fake.revokes();
    assert_eq!(revokes.len(), 1);
    assert_eq!(revokes[0].token, format!("rt-{TEST_CLIENT}-ac_a"));
    assert!(!revokes[0].file_held_tokens);
    assert!(revokes[0].lock_was_free);
}
