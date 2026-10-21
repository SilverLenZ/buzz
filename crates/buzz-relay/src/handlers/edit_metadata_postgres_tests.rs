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

/// A change that fails after authorization stores nothing and changes
/// nothing. A name longer than the 255-character column passes the relay's
/// checks but fails the UPDATE. Before, the relay stored the event, applied
/// `about`, skipped `topic`, and reported success; a resend then returned
/// `duplicate:`. Mutation: store the event in its own transaction → RED.
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

    let long_name = "n".repeat(256);
    let event = EventBuilder::new(Kind::Custom(9002), "")
        .tags([
            Tag::parse(["h", &channel_id.to_string()]).unwrap(),
            Tag::parse(["about", "changed"]).unwrap(),
            Tag::parse(["name", &long_name]).unwrap(),
            Tag::parse(["topic", "changed"]).unwrap(),
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
    }

    let after = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .expect("channel");
    assert_eq!(after.name, before.name);
    assert_eq!(after.description, before.description, "about is unchanged");
    assert_eq!(after.topic, before.topic, "topic is unchanged");
    assert_eq!(count_kind(&state, &tenant, channel_id, 9002).await, 0);
    assert_eq!(
        count_kind(&state, &tenant, channel_id, 40099).await,
        notices_before,
        "no system message announces a change that did not happen"
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
