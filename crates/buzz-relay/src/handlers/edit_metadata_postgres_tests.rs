//! Postgres-backed tests for kind:9002: the event and the channel changes it
//! names commit together, or not at all.

use buzz_db::event::EventQuery;
use nostr::{EventBuilder, Keys, Kind, Tag};
use uuid::Uuid;

use crate::handlers::group_state_postgres_tests::{
    community, create_channel, edit_channel, http_auth, state,
};
use crate::handlers::ingest::ingest_event;

/// Count stored events of `kind` in `channel_id`.
async fn count_kind(
    state: &crate::state::AppState,
    tenant: &buzz_core::TenantContext,
    channel_id: Uuid,
    kind: i32,
) -> usize {
    state
        .db
        .query_events(&EventQuery {
            kinds: Some(vec![kind]),
            channel_id: Some(channel_id),
            ..EventQuery::for_community(tenant.community())
        })
        .await
        .expect("query events")
        .len()
}

/// A change that fails inside the write stores nothing and changes nothing,
/// and the same event succeeds once the failure is gone. A test trigger makes
/// the topic UPDATE fail after the event row and the `about` change were
/// written in the same transaction. Before, the relay stored the event,
/// applied `about`, skipped `topic`, and reported success; a resend then
/// returned `duplicate:`. Mutation: store the event in its own transaction
/// → RED.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn failed_edit_stores_nothing() {
    let state = state().await;
    let tenant = community(&state, "edit-atomic-fail").await;
    let owner = Keys::generate();
    let channel_id = create_channel(&state, &tenant, &owner, &[]).await;
    let before = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    let notices_before = count_kind(&state, &tenant, channel_id, 40099).await;

    // Each test runs in its own database, so this trigger affects only it.
    for statement in [
        "CREATE FUNCTION edit_atomic_fail() RETURNS trigger LANGUAGE plpgsql AS \
         $$ BEGIN RAISE EXCEPTION 'injected topic failure'; END $$",
        "CREATE TRIGGER edit_atomic_fail BEFORE UPDATE ON channels FOR EACH ROW \
         WHEN (NEW.topic = 'fail-topic') EXECUTE FUNCTION edit_atomic_fail()",
    ] {
        sqlx::query(statement)
            .execute(state.db.pool())
            .await
            .expect("install failure trigger");
    }

    let event = EventBuilder::new(Kind::Custom(9002), "")
        .tags([
            Tag::parse(["h", &channel_id.to_string()]).unwrap(),
            Tag::parse(["about", "changed"]).unwrap(),
            Tag::parse(["topic", "fail-topic"]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .expect("sign 9002");

    for attempt in ["first send", "resend"] {
        match ingest_event(&state, &tenant, event.clone(), http_auth(&owner)).await {
            Ok(result) => assert!(
                !result.accepted,
                "{attempt}: a failed edit was reported as accepted: {:?}",
                result.message
            ),
            Err(error) => assert!(
                format!("{error:?}").contains("channel update failed"),
                "{attempt}: {error:?}"
            ),
        }
        let after = state
            .db
            .get_channel(tenant.community(), channel_id)
            .await
            .expect("channel");
        assert_eq!(
            after.description, before.description,
            "{attempt}: about is unchanged"
        );
        assert_eq!(after.topic, before.topic, "{attempt}: topic is unchanged");
        assert_eq!(
            count_kind(&state, &tenant, channel_id, 9002).await,
            0,
            "{attempt}"
        );
        assert_eq!(
            count_kind(&state, &tenant, channel_id, 40099).await,
            notices_before,
            "{attempt}: no system message announces a change that did not happen"
        );
    }

    // With the failure gone, the same signed event applies in full.
    sqlx::query("DROP TRIGGER edit_atomic_fail ON channels")
        .execute(state.db.pool())
        .await
        .expect("drop failure trigger");
    let retry = ingest_event(&state, &tenant, event, http_auth(&owner))
        .await
        .expect("retry");
    assert!(retry.accepted, "{:?}", retry.message);
    assert!(retry.message.is_empty(), "{:?}", retry.message);
    let after = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    assert_eq!(after.description.as_deref(), Some("changed"));
    assert_eq!(after.topic.as_deref(), Some("fail-topic"));
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 1);
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before + 1
    );
}

/// A resend of a stored edit is a duplicate and changes nothing: its changes
/// and system messages committed with it the first time.
/// Mutation: apply the edit again on a duplicate → RED (second notice).
#[tokio::test]
#[ignore = "requires Postgres"]
async fn resent_edit_is_a_duplicate() {
    let state = state().await;
    let tenant = community(&state, "edit-atomic-resend").await;
    let owner = Keys::generate();
    let channel_id = create_channel(&state, &tenant, &owner, &[]).await;
    let notices_before = count_kind(&state, &tenant, channel_id, 40099).await;

    let event = EventBuilder::new(Kind::Custom(9002), "")
        .tags([
            Tag::parse(["h", &channel_id.to_string()]).unwrap(),
            Tag::parse(["about", "about-1"]).unwrap(),
            Tag::parse(["topic", "topic-1"]).unwrap(),
            Tag::parse(["purpose", "purpose-1"]).unwrap(),
            Tag::parse(["ttl", "3600"]).unwrap(),
            Tag::parse(["t", "label-1"]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .expect("sign 9002");

    let first = ingest_event(&state, &tenant, event.clone(), http_auth(&owner))
        .await
        .expect("first send");
    assert!(first.accepted, "{:?}", first.message);
    assert!(first.message.is_empty(), "{:?}", first.message);

    let channel = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    assert_eq!(channel.description.as_deref(), Some("about-1"));
    assert_eq!(channel.topic.as_deref(), Some("topic-1"));
    assert_eq!(
        channel.topic_set_by.as_deref(),
        Some(owner.public_key().to_bytes().as_slice())
    );
    assert_eq!(channel.purpose.as_deref(), Some("purpose-1"));
    assert_eq!(channel.ttl_seconds, Some(3600));
    assert!(channel.ttl_deadline.is_some());
    assert_eq!(channel.labels, vec!["label-1"]);
    // topic, purpose and ttl each announce their change.
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before + 3
    );

    let resend = ingest_event(&state, &tenant, event, http_auth(&owner))
        .await
        .expect("resend");
    assert!(resend.accepted, "{:?}", resend.message);
    assert_eq!(resend.message, "duplicate:");
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 1);
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before + 3
    );
}

/// Restoring a channel that is not archived is refused before storage.
/// Before, the event was stored and reported accepted while the restore
/// failed. Mutation: drop the pre-storage check → RED.
#[tokio::test]
#[ignore = "requires Postgres"]
async fn unarchive_of_an_active_channel_is_refused() {
    let state = state().await;
    let tenant = community(&state, "edit-atomic-unarchive").await;
    let owner = Keys::generate();
    let channel_id = create_channel(&state, &tenant, &owner, &[]).await;
    let notices_before = count_kind(&state, &tenant, channel_id, 40099).await;

    let error = edit_channel(
        &state,
        &tenant,
        &owner,
        channel_id,
        &[&["archived", "false"]],
    )
    .await
    .expect_err("unarchive of an active channel");
    assert!(error.contains("channel is not archived"), "{error}");
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 0);

    // Archive then restore still works, and each step is announced.
    edit_channel(
        &state,
        &tenant,
        &owner,
        channel_id,
        &[&["archived", "true"]],
    )
    .await
    .expect("archive");
    edit_channel(
        &state,
        &tenant,
        &owner,
        channel_id,
        &[&["archived", "false"]],
    )
    .await
    .expect("unarchive");
    let channel = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    assert!(channel.archived_at.is_none());
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 2);
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before + 2
    );
}

/// Resending a committed archive or unarchive is a duplicate, not a refusal.
/// After the first send commits, the channel is already in the requested
/// state, so validation alone would refuse the identical resend (a lost
/// response would look like a failure). Mutation: drop the pre-validation
/// duplicate check in ingest → RED ("channel is not archived").
#[tokio::test]
#[ignore = "requires Postgres"]
async fn resent_archive_and_unarchive_are_duplicates() {
    let state = state().await;
    let tenant = community(&state, "edit-atomic-archive-resend").await;
    let owner = Keys::generate();
    let channel_id = create_channel(&state, &tenant, &owner, &[]).await;
    let notices_before = count_kind(&state, &tenant, channel_id, 40099).await;

    for (value, archived) in [("true", true), ("false", false)] {
        let event = EventBuilder::new(Kind::Custom(9002), "")
            .tags([
                Tag::parse(["h", &channel_id.to_string()]).unwrap(),
                Tag::parse(["archived", value]).unwrap(),
            ])
            .sign_with_keys(&owner)
            .expect("sign 9002");
        let first = ingest_event(&state, &tenant, event.clone(), http_auth(&owner))
            .await
            .expect("first send");
        assert!(first.accepted, "archived={value}: {:?}", first.message);
        assert!(
            first.message.is_empty(),
            "archived={value}: {:?}",
            first.message
        );

        let resend = ingest_event(&state, &tenant, event, http_auth(&owner))
            .await
            .expect("resend");
        assert!(resend.accepted, "archived={value}: {:?}", resend.message);
        assert_eq!(resend.message, "duplicate:", "archived={value}");

        let channel = state
            .db
            .get_channel(tenant.community(), channel_id)
            .await
            .expect("channel");
        assert_eq!(channel.archived_at.is_some(), archived, "archived={value}");
    }
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 2);
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before + 2
    );
}

/// A kind:9002 `join_role` change goes through the same transaction as the
/// other settings: it is stored, and it is announced by one
/// `join_role_changed` system message. Mutation: drop the `join_role` arm
/// from the edit → RED (the event changes nothing).
#[tokio::test]
#[ignore = "requires Postgres"]
async fn join_role_edit_is_applied_and_announced() {
    let state = state().await;
    let tenant = community(&state, "edit-atomic-join-role").await;
    let owner = Keys::generate();
    let channel_id = create_channel(&state, &tenant, &owner, &[]).await;
    let notices_before = count_kind(&state, &tenant, channel_id, 40099).await;

    let event = EventBuilder::new(Kind::Custom(9002), "")
        .tags([
            Tag::parse(["h", &channel_id.to_string()]).unwrap(),
            Tag::parse(["join_role", "guest"]).unwrap(),
        ])
        .sign_with_keys(&owner)
        .expect("sign 9002");
    let result = ingest_event(&state, &tenant, event, http_auth(&owner))
        .await
        .expect("ingest");
    assert!(result.accepted, "{:?}", result.message);

    let after = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    assert_eq!(after.join_role, "guest");
    let notices = state
        .db
        .query_events(&EventQuery {
            kinds: Some(vec![40099]),
            channel_id: Some(channel_id),
            ..EventQuery::for_community(tenant.community())
        })
        .await
        .expect("query events");
    assert_eq!(notices.len(), notices_before + 1);
    let announced = notices
        .iter()
        .filter_map(|stored| serde_json::from_str::<serde_json::Value>(&stored.event.content).ok())
        .any(|content| content["type"] == "join_role_changed" && content["join_role"] == "guest");
    assert!(announced, "one join_role_changed system message");
}
